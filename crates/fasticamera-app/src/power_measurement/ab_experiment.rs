//! A/B-замер вклада стадии в ток телефона (решение 45): «ток со стадией − ток без неё».
//!
//! ## Расписание: ABBA (порядок в паре чередуется)
//!
//! Окна идут подряд по `window`; пара `p` — окна `2p` и `2p + 1`. Чётная пара — «A, затем B»,
//! нечётная — «B, затем A» (A — стадия включена, B — выключена): `AB BA AB BA …`.
//!
//! Почему не `ABAB`: ток медленно ползёт сам по себе (нагрев, падение напряжения батареи,
//! фоновая работа). Пусть дрейф — `d` мА за окно. При `ABAB` каждая пара даёт `Δ − d`: смещение
//! одинаковое во всех парах, поэтому в разбросе его НЕ видно — вывод уверенно неверный. При `ABBA`
//! пары дают попеременно `Δ − d` и `Δ + d`: в среднем дрейф сокращается (при чётном числе пар —
//! точно, при нечётном остаётся `d / N`), а сам дрейф честно попадает в разброс. Цена — нужно
//! помнить порядок пары, это делает [`PairOrder`].
//!
//! ## Вывод
//!
//! Δ пары = среднее A − среднее B; итог — среднее Δ по парам и разброс `max Δ − min Δ`. Если
//! `|среднее| ≤ разброс`, вклад «не различим» ([`Contribution::Indistinguishable`]) — без числа.
//! Любое окно на внешнем питании, неустановленный или противоречивый знак тока, слишком много
//! пропусков — отказ мерить ([`Contribution::Rejected`]), не догадка.
//!
//! ## Как сценарий ведёт замер
//!
//! На каждом тике маршрутизатора сценарий забирает показания (`PowerSampling::drain_samples`),
//! читает `CLOCK_MONOTONIC` (`clock_offset::monotonic_now`) и вызывает
//! [`AbPowerExperiment::on_tick`]. Ответ [`AbStep::Measure`] говорит, должна ли стадия сейчас быть
//! включена: сценарий применяет это (идемпотентно) к ISP. [`AbStep::Finished`] — замер окончен,
//! отчёт в [`AbPowerExperiment::report`]. Переключение стадии отстаёт от границы окна на тик —
//! это поглощает разогрев.

use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use shell_contract::power::{
    BatterySample, DischargeSign, DischargeSignEvidence, PlausibleCurrentRange, detect_discharge_sign,
};

use super::config::{AbSettings, PowerSettings, SignPolicy};
use super::window::{PowerWindowAccumulator, WindowMeasurement, WindowPush, WindowRejection, WindowSpan, offset};
use shell_contract::power::PowerSamplingPeriod;

/// Включена ли измеряемая стадия.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StageSetting {
    /// Окно A: стадия включена.
    On,
    /// Окно B: стадия выключена.
    Off,
}

/// Порядок окон в паре.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PairOrder {
    AThenB,
    BThenA,
}

impl PairOrder {
    fn of_pair(pair: usize) -> Self {
        if pair.is_multiple_of(2) {
            PairOrder::AThenB
        } else {
            PairOrder::BThenA
        }
    }
}

/// Какое окно идёт.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WindowPosition {
    /// Номер окна с нуля.
    pub index: usize,
    /// Номер пары с нуля.
    pub pair: usize,
    /// Идёт разогрев (показания в среднее не идут).
    pub warming_up: bool,
}

/// Что делать сценарию на этом тике.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbStep {
    /// Замер идёт: стадия должна быть в положении `stage`.
    Measure {
        stage: StageSetting,
        window: WindowPosition,
    },
    /// Замер окончен (вывод или отказ) — см. [`AbPowerExperiment::report`].
    Finished,
}

/// Почему A/B-замер отказался давать число.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AbRejection {
    /// Окно не годится (внешнее питание, нет тока, дыры).
    Window {
        index: usize,
        stage: StageSetting,
        rejection: WindowRejection,
    },
    /// Знак тока разряда не установлен или противоречит установленному — угадывать нельзя.
    DischargeSignUndetermined {
        index: usize,
        evidence: DischargeSignEvidence,
    },
}

/// Вклад стадии.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum Contribution {
    /// Вклад больше разброса повторов: число осмысленно.
    Measured { mean_delta_ma: f64, spread_ma: f64 },
    /// Вклад не больше разброса: «не различим», числа нет.
    Indistinguishable { spread_ma: f64 },
    /// Мерить нельзя.
    Rejected { reason: AbRejection },
}

/// Одна пара A/B.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct PairDelta {
    pub pair: usize,
    pub order: PairOrder,
    pub stage_on_ma: f64,
    pub stage_off_ma: f64,
    /// `stage_on_ma − stage_off_ma`.
    pub delta_ma: f64,
}

/// Закрытое годное окно.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WindowReport {
    pub index: usize,
    pub stage: StageSetting,
    pub measurement: WindowMeasurement,
}

/// Отчёт замера.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AbPowerReport {
    pub contribution: Contribution,
    /// Знак, с которым считались токи (`None` — не установлен).
    pub discharge_sign: Option<DischargeSign>,
    pub pairs: Vec<PairDelta>,
    pub windows: Vec<WindowReport>,
    /// Показания раньше начала замера (из кольца до старта) — отброшены.
    pub samples_before_start: u64,
    /// Показания для уже закрытого окна (пришли позже `close_grace`) — отброшены.
    pub late_samples: u64,
}

/// Открытое окно и его номер.
struct OpenWindow {
    index: usize,
    accumulator: PowerWindowAccumulator,
}

/// A/B-замер: расписание окон, накопление, знак тока, вывод.
pub struct AbPowerExperiment {
    settings: AbSettings,
    period: PowerSamplingPeriod,
    sign_policy: SignPolicy,
    max_missing_fraction: f64,
    /// Правдоподобный модуль среднего тока окна (проверка единиц `CURRENT_NOW`).
    current_range: PlausibleCurrentRange,
    started_at: MonotonicNanos,
    open: Option<OpenWindow>,
    sign: Option<DischargeSign>,
    windows: Vec<WindowReport>,
    samples_before_start: u64,
    late_samples: u64,
    report: Option<AbPowerReport>,
}

impl AbPowerExperiment {
    /// Замер, начинающийся в `started_at` (первое окно — A, стадия включена).
    pub fn new(settings: &PowerSettings, started_at: MonotonicNanos) -> Self {
        let sign = match settings.sign {
            SignPolicy::Fixed(sign) => Some(sign),
            SignPolicy::Detect { .. } => None,
        };
        let mut experiment = Self {
            settings: settings.ab,
            period: settings.sampling.period,
            sign_policy: settings.sign,
            max_missing_fraction: settings.max_missing_fraction,
            current_range: settings.current_range,
            started_at,
            open: None,
            sign,
            windows: Vec::new(),
            samples_before_start: 0,
            late_samples: 0,
            report: None,
        };
        experiment.open = Some(experiment.open_window(0));
        experiment
    }

    /// Всего окон: две на пару.
    pub fn total_windows(&self) -> usize {
        usize::try_from(self.settings.pairs)
            .unwrap_or(usize::MAX)
            .saturating_mul(2)
    }

    /// Положение стадии в окне `index` по схеме ABBA.
    pub fn stage_of(index: usize) -> StageSetting {
        let first_in_pair = index.is_multiple_of(2);
        match (PairOrder::of_pair(index / 2), first_in_pair) {
            (PairOrder::AThenB, true) | (PairOrder::BThenA, false) => StageSetting::On,
            (PairOrder::AThenB, false) | (PairOrder::BThenA, true) => StageSetting::Off,
        }
    }

    /// Отчёт, когда замер окончен.
    pub fn report(&self) -> Option<&AbPowerReport> {
        self.report.as_ref()
    }

    /// Тик сценария: свежие показания и текущий момент `CLOCK_MONOTONIC`.
    pub fn on_tick(&mut self, now: MonotonicNanos, samples: &[BatterySample]) -> AbStep {
        for sample in samples {
            if self.report.is_some() {
                break;
            }
            self.route(*sample);
        }
        self.close_due_windows(now);
        if self.report.is_some() {
            return AbStep::Finished;
        }
        let index = self.window_index(now).min(self.total_windows().saturating_sub(1));
        let span = self.span_of(index);
        AbStep::Measure {
            stage: Self::stage_of(index),
            window: WindowPosition {
                index,
                pair: index / 2,
                warming_up: span.is_warmup(now),
            },
        }
    }

    /// Окно, в которое попадает момент (может быть за последним).
    fn window_index(&self, at: MonotonicNanos) -> usize {
        let elapsed = at.as_nanos().saturating_sub(self.started_at.as_nanos()).max(0);
        let window = i64::try_from(self.settings.window.as_nanos())
            .unwrap_or(i64::MAX)
            .max(1);
        usize::try_from(elapsed / window).unwrap_or(usize::MAX)
    }

    fn span_of(&self, index: usize) -> WindowSpan {
        let windows_before = u32::try_from(index).unwrap_or(u32::MAX);
        let start = offset(self.started_at, self.settings.window.saturating_mul(windows_before));
        WindowSpan::new(start, self.settings.warmup, self.settings.window)
    }

    fn open_window(&self, index: usize) -> OpenWindow {
        OpenWindow {
            index,
            accumulator: PowerWindowAccumulator::new(self.span_of(index), self.period),
        }
    }

    /// Кладёт показание в его окно; более ранние открытые окна при этом закрываются.
    fn route(&mut self, sample: BatterySample) {
        if sample.at < self.started_at {
            self.samples_before_start += 1;
            return;
        }
        let index = self.window_index(sample.at);
        while self.open.as_ref().is_some_and(|open| open.index < index) && self.report.is_none() {
            self.close_open_window();
        }
        let Some(open) = self.open.as_mut() else {
            // Все окна закрыты — показание после конца замера; на вывод не влияет.
            return;
        };
        if open.index > index {
            self.late_samples += 1;
            return;
        }
        let stage = Self::stage_of(open.index);
        let window_index = open.index;
        if let WindowPush::Rejected(rejection) = open.accumulator.push(sample) {
            self.reject(AbRejection::Window {
                index: window_index,
                stage,
                rejection,
            });
        }
    }

    /// Закрывает окна, чей конец (с запасом на запоздавшие показания) уже прошёл.
    fn close_due_windows(&mut self, now: MonotonicNanos) {
        while self.report.is_none() {
            let Some(open) = self.open.as_ref() else { return };
            let close_at = offset(open.accumulator.span().end(), self.settings.close_grace);
            if now < close_at {
                return;
            }
            self.close_open_window();
        }
    }

    /// Закрывает открытое окно: знак тока, итог окна, следующее окно или вывод.
    fn close_open_window(&mut self) {
        let Some(open) = self.open.take() else { return };
        let stage = Self::stage_of(open.index);
        if let Err(rejection) = open.accumulator.check_complete(self.max_missing_fraction) {
            return self.reject(AbRejection::Window {
                index: open.index,
                stage,
                rejection,
            });
        }
        let sign = match self.confirm_sign(&open) {
            Ok(sign) => sign,
            Err(evidence) => {
                return self.reject(AbRejection::DischargeSignUndetermined {
                    index: open.index,
                    evidence,
                });
            }
        };
        match open
            .accumulator
            .finish(sign, self.max_missing_fraction, self.current_range)
        {
            Ok(measurement) => self.windows.push(WindowReport {
                index: open.index,
                stage,
                measurement,
            }),
            Err(rejection) => {
                return self.reject(AbRejection::Window {
                    index: open.index,
                    stage,
                    rejection,
                });
            }
        }
        let next = open.index + 1;
        if next < self.total_windows() {
            self.open = Some(self.open_window(next));
        } else {
            self.conclude();
        }
    }

    /// Знак тока для окна. При автоопределении КАЖДОЕ окно обязано подтвердить знак первого:
    /// противоречие (вендор шумит около нуля, телефон начал заряжаться без смены статуса) — отказ.
    fn confirm_sign(&mut self, open: &OpenWindow) -> Result<DischargeSign, DischargeSignEvidence> {
        let rule = match self.sign_policy {
            // Знак задан конфигом: подтверждать нечего.
            SignPolicy::Fixed(sign) => return Ok(sign),
            SignPolicy::Detect { rule } => rule,
        };
        let evidence = detect_discharge_sign(open.accumulator.measured_samples(), rule);
        match (evidence, self.sign) {
            (DischargeSignEvidence::Detected { sign, .. }, None) => {
                self.sign = Some(sign);
                Ok(sign)
            }
            (DischargeSignEvidence::Detected { sign, .. }, Some(established)) if sign == established => Ok(sign),
            (evidence, _) => Err(evidence),
        }
    }

    fn reject(&mut self, reason: AbRejection) {
        self.open = None;
        self.report = Some(self.build_report(Contribution::Rejected { reason }, Vec::new()));
    }

    /// Все окна закрыты и годны: Δ по парам, среднее, разброс, вывод.
    fn conclude(&mut self) {
        let pairs: Vec<PairDelta> = self
            .windows
            .chunks_exact(2)
            .enumerate()
            .map(|(pair, windows)| pair_delta(pair, &windows[0], &windows[1]))
            .collect();
        let contribution = judge(&pairs);
        self.report = Some(self.build_report(contribution, pairs));
    }

    fn build_report(&self, contribution: Contribution, pairs: Vec<PairDelta>) -> AbPowerReport {
        AbPowerReport {
            contribution,
            discharge_sign: self.sign,
            pairs,
            windows: self.windows.clone(),
            samples_before_start: self.samples_before_start,
            late_samples: self.late_samples,
        }
    }
}

/// Δ пары из двух соседних окон (какое из них A — по схеме, а не по положению).
fn pair_delta(pair: usize, first: &WindowReport, second: &WindowReport) -> PairDelta {
    let (on, off) = if first.stage == StageSetting::On {
        (first, second)
    } else {
        (second, first)
    };
    let stage_on_ma = on.measurement.mean_discharge_ma;
    let stage_off_ma = off.measurement.mean_discharge_ma;
    PairDelta {
        pair,
        order: PairOrder::of_pair(pair),
        stage_on_ma,
        stage_off_ma,
        delta_ma: stage_on_ma - stage_off_ma,
    }
}

/// Вывод по парам: «не различим», если `|среднее Δ| ≤ разброс` (размах Δ между парами).
pub fn judge(pairs: &[PairDelta]) -> Contribution {
    let deltas = pairs.iter().map(|pair| pair.delta_ma);
    let (min, max) = deltas
        .clone()
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), delta| {
            (low.min(delta), high.max(delta))
        });
    let spread_ma = if pairs.is_empty() { 0.0 } else { max - min };
    let mean_delta_ma = if pairs.is_empty() {
        0.0
    } else {
        // Пар единицы — перевод в f64 точен.
        deltas.sum::<f64>() / pairs.len() as f64
    };
    if mean_delta_ma.abs() > spread_ma {
        Contribution::Measured {
            mean_delta_ma,
            spread_ma,
        }
    } else {
        Contribution::Indistinguishable { spread_ma }
    }
}

#[cfg(test)]
#[path = "ab_experiment_tests.rs"]
mod tests;
