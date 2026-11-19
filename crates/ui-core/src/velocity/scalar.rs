//! Оценка скорости одной величины (координаты, расстояния, масштаба) по истории выборок.
//!
//! Алгоритм повторяет `LeastSquaresVelocityTrackerStrategy` Android (AOSP `VelocityTracker.cpp`):
//! * время откладывается назад от **последней** точки (`x = −возраст`), поэтому линейный
//!   коэффициент полинома и есть скорость в последней точке;
//! * степень полинома урезается до `n − 1` (две точки — прямая, три — парабола точно);
//! * точка с той же меткой времени заменяет предыдущую;
//! * разрыв между точками больше `assume_stopped_after` стирает историю («палец стоял, старое
//!   движение не относится к новому»), разрыв перед запросом (отрыв пальца) даёт «остановился».
//!
//! # Как учитывается «пауза перед отпусканием»
//! `behavior.md` раздел 1 требует определять паузу по **ускорению**, а не по таймеру (WWDC18 803:
//! *«we look at your finger's acceleration»*). Парабола в последней точке это делает сама:
//! если палец тормозил, производная параболы в конце близка к нулю (или меняет знак), тогда как
//! простая разность двух последних точек показала бы старую скорость. Таймер AOSP (`40 мс`) остаётся
//! дополнением для случая, когда палец остановился совсем и касаний не шлёт — выборок нет, и
//! кривая о паузе ничего не знает. Одно не заменяет другое.

use std::collections::VecDeque;

use super::config::{VelocityConfig, VelocityConfigError};
use crate::time::{MonotonicNanos, UiDuration};

/// Итог оценки скорости.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum VelocityEstimate<V> {
    /// Точек мало или кривую построить нельзя (вырожденная матрица): оценки нет.
    NotEnoughSamples,
    /// Палец стоял дольше порога «остановился»: скорость нулевая по определению.
    Stopped,
    /// Скорость измерена, единицы величины в секунду.
    Measured(V),
}

impl VelocityEstimate<f64> {
    /// Число для потребителя, которому важно только значение: «нет оценки» и «стоп» дают 0.
    pub fn or_zero(self) -> f64 {
        match self {
            Self::Measured(value) if value.is_finite() => value,
            _ => 0.0,
        }
    }
}

/// Что произошло с добавленной выборкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleOutcome {
    /// Выборка добавлена в историю.
    Added,
    /// Перед выборкой была пауза дольше порога: история стёрта, выборка начала новую.
    AddedAfterPause,
    /// Метка времени совпала с последней: последняя выборка заменена.
    ReplacedSameTime,
    /// Метка раньше последней: выборка отброшена, история не тронута.
    RejectedOutOfOrder,
    /// Значение не конечное (NaN/inf): выборка отброшена, история не тронута.
    RejectedNonFinite,
}

#[derive(Debug, Clone, Copy)]
struct Sample {
    time: MonotonicNanos,
    value: f64,
}

/// Оценщик скорости одной величины.
#[derive(Debug, Clone)]
pub struct ScalarVelocityTracker {
    config: VelocityConfig,
    samples: VecDeque<Sample>,
}

impl ScalarVelocityTracker {
    pub fn new(config: VelocityConfig) -> Result<Self, VelocityConfigError> {
        config.validate()?;
        Ok(Self {
            config,
            samples: VecDeque::new(),
        })
    }

    /// Стереть историю (новое касание).
    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// Метка времени последней выборки.
    pub fn last_sample_time(&self) -> Option<MonotonicNanos> {
        self.samples.back().map(|sample| sample.time)
    }

    pub fn add_sample(&mut self, time: MonotonicNanos, value: f64) -> SampleOutcome {
        if !value.is_finite() {
            return SampleOutcome::RejectedNonFinite;
        }
        let mut outcome = SampleOutcome::Added;
        if let Some(last) = self.samples.back().copied() {
            if time < last.time {
                return SampleOutcome::RejectedOutOfOrder;
            }
            if time == last.time {
                self.samples.pop_back();
                outcome = SampleOutcome::ReplacedSameTime;
            } else if UiDuration::between(last.time, time) > self.config.assume_stopped_after() {
                // Палец стоял: старое движение к новому не относится (как в AOSP `addMovement`).
                self.samples.clear();
                outcome = SampleOutcome::AddedAfterPause;
            }
        }
        self.samples.push_back(Sample { time, value });
        // Кольцо истории и горизонт: лишнее спереди выбрасываем.
        while self.samples.len() > self.config.history_size {
            self.samples.pop_front();
        }
        let horizon = self.config.horizon();
        while self
            .samples
            .front()
            .is_some_and(|oldest| UiDuration::between(oldest.time, time) > horizon)
        {
            self.samples.pop_front();
        }
        outcome
    }

    /// Скорость в момент `now` (обычно — метка отрыва пальца или текущего касания).
    pub fn estimate_at(&self, now: MonotonicNanos) -> VelocityEstimate<f64> {
        let Some(newest) = self.samples.back() else {
            return VelocityEstimate::NotEnoughSamples;
        };
        if UiDuration::between(newest.time, now) > self.config.assume_stopped_after() {
            return VelocityEstimate::Stopped;
        }
        match self.fit_velocity(newest.time) {
            Some(velocity) => VelocityEstimate::Measured(velocity),
            None => VelocityEstimate::NotEnoughSamples,
        }
    }

    /// Подгонка полинома по нормальным уравнениям. Время нормируется на горизонт (`u ∈ [−1, 0]`),
    /// иначе степени `u^k` для секунд порядка 0,1 дают плохо обусловленную матрицу.
    fn fit_velocity(&self, newest_time: MonotonicNanos) -> Option<f64> {
        let count = self.samples.len();
        if count < 2 {
            return None;
        }
        let degree = usize::from(self.config.polynomial_degree).min(count - 1);
        let horizon_secs = self.config.horizon().as_secs_f64();
        let size = degree + 1;
        // normal[j][k] = Σ u^(j+k); rhs[j] = Σ y·u^j.
        let mut normal = vec![vec![0.0_f64; size + 1]; size];
        for sample in &self.samples {
            let age_secs = UiDuration::between(sample.time, newest_time).as_secs_f64();
            let u = -age_secs / horizon_secs;
            let mut powers = vec![1.0_f64; 2 * degree + 1];
            for exponent in 1..powers.len() {
                powers[exponent] = powers[exponent - 1] * u;
            }
            for (row, line) in normal.iter_mut().enumerate() {
                for (column, cell) in line.iter_mut().take(size).enumerate() {
                    *cell += powers[row + column];
                }
                line[size] += sample.value * powers[row];
            }
        }
        let coefficients = solve_linear_system(normal)?;
        // y(u) = c0 + c1·u + …; dy/dt в u = 0 равна c1 / horizon.
        let velocity = coefficients.get(1)? / horizon_secs;
        velocity.is_finite().then_some(velocity)
    }
}

/// Решение системы `A·x = b` методом Гаусса с выбором главного элемента. Матрица передаётся
/// расширенной (последний столбец — правая часть). `None` — матрица вырождена.
fn solve_linear_system(mut matrix: Vec<Vec<f64>>) -> Option<Vec<f64>> {
    /// Ниже этого значения ведущий элемент считается нулём. Матрица нормирована (`|u| ≤ 1`),
    /// поэтому абсолютный порог осмыслен.
    const PIVOT_EPSILON: f64 = 1e-12;
    let size = matrix.len();
    for pivot_index in 0..size {
        let best_row = (pivot_index..size)
            .max_by(|&a, &b| matrix[a][pivot_index].abs().total_cmp(&matrix[b][pivot_index].abs()))?;
        if matrix[best_row][pivot_index].abs() < PIVOT_EPSILON {
            return None;
        }
        matrix.swap(pivot_index, best_row);
        let pivot_row = matrix[pivot_index].clone();
        for line in matrix.iter_mut().skip(pivot_index + 1) {
            let factor = line[pivot_index] / pivot_row[pivot_index];
            for (cell, pivot_cell) in line.iter_mut().zip(&pivot_row).skip(pivot_index) {
                *cell -= factor * pivot_cell;
            }
        }
    }
    let mut solution = vec![0.0_f64; size];
    for row in (0..size).rev() {
        let mut rest = matrix[row][size];
        for column in (row + 1)..size {
            rest -= matrix[row][column] * solution[column];
        }
        solution[row] = rest / matrix[row][row];
    }
    solution.iter().all(|value| value.is_finite()).then_some(solution)
}
