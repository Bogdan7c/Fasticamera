//! Журнал результатов HAL для сценария `3a`.
//!
//! Движок публикует в статусе только ПОСЛЕДНИЙ результат (`ThreeAStatus::last_result`), а сценарий
//! опрашивается раз в тик (50 мс ≈ 3 кадра). Поэтому журнал:
//! * дедуплицирует по номеру кадра (один и тот же результат на нескольких тиках — один раз);
//! * хранит только результаты повторяющегося запроса (`RequestOrigin::Repeating`): кадры триггера фокуса и
//!   серии имеют свою экспозицию и в проверки экспозиции/EV/блокировки не годятся;
//! * ограничен по длине — долгий прогон не растит память.
//!
//! Журнал видит каждый ТРЕТИЙ кадр: «задержка в кадрах» и «кадров до сходимости» в отчёте имеют
//! точность до тика, и это сказано в отчёте (`sampled_every_tick`).

use std::collections::VecDeque;

use camera_contract::three_a::RequestOrigin;
use camera_core::three_a_status::LastThreeAResult;

/// Сколько результатов хранит журнал: 4096 тиков ≈ 3,4 минуты, дольше сценарий не идёт.
pub const RESULT_LOG_CAPACITY: usize = 4_096;

/// Журнал результатов повторяющегося запроса, новые в конце.
#[derive(Debug, Default)]
pub struct ResultLog {
    results: VecDeque<LastThreeAResult>,
    /// Номер последнего кадра любого происхождения: по нему отсекаем повторы одного снимка.
    last_seen_frame: Option<i64>,
}

impl ResultLog {
    /// Принимает последний результат движка; возвращает, новый ли он.
    pub fn observe(&mut self, last: Option<&LastThreeAResult>) -> bool {
        let Some(result) = last else {
            return false;
        };
        if self.last_seen_frame == Some(result.frame_number.0) {
            return false;
        }
        self.last_seen_frame = Some(result.frame_number.0);
        if result.origin == RequestOrigin::Repeating {
            if self.results.len() == RESULT_LOG_CAPACITY {
                self.results.pop_front();
            }
            self.results.push_back(*result);
        }
        true
    }

    /// Номер последнего увиденного кадра (любого происхождения) — точка отсчёта «после команды».
    pub fn latest_frame(&self) -> Option<i64> {
        self.last_seen_frame
    }

    /// Последний результат повторяющегося запроса.
    pub fn latest_repeating(&self) -> Option<&LastThreeAResult> {
        self.results.back()
    }

    /// Результаты повторяющегося запроса строго после кадра `frame` (`None` — все), старые первыми.
    pub fn repeating_after(&self, frame: Option<i64>) -> impl Iterator<Item = &LastThreeAResult> {
        self.results
            .iter()
            .filter(move |result| frame.is_none_or(|boundary| result.frame_number.0 > boundary))
    }
}

/// Произведение «выдержка × ISO», нс·ISO: световая энергия кадра, постоянная при блокировке AE и
/// удваивающаяся при +1 EV. `None`, если HAL не отдал одно из чисел.
pub fn exposure_product(result: &LastThreeAResult) -> Option<f64> {
    let exposure = result.exposure_time?;
    let iso = result.sensitivity_iso?;
    Some(exposure.as_nanos() as f64 * f64::from(iso))
}

/// Медиана (пустой набор — `None`). NaN не бывает: входы — конечные произведения.
pub fn median(mut values: Vec<f64>) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

/// Относительное расхождение `value` и `target`; `None` при нулевой цели.
pub fn relative_gap(value: f64, target: f64) -> Option<f64> {
    (target != 0.0).then(|| ((value - target) / target).abs())
}

/// Критерий «произведение «выдержка × ISO» перестало меняться».
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrozenCriteria {
    /// Допустимое относительное расхождение с итоговым значением, доля (тот же допуск, что у постоянства).
    pub tolerance_fraction: f64,
    /// Сколько последних результатов подряд должны уложиться в допуск, чтобы считать произведение замершим:
    /// один последний результат «замершим» был бы всегда.
    pub min_tail_samples: usize,
}

/// Через сколько кадров после `marker` (кадра команды) произведение перестало меняться до конца ряда.
///
/// `products` — пары «номер кадра, произведение» в порядке кадров. Ищем самый длинный хвост ряда, где каждое
/// значение в допуске от ПОСЛЕДНЕГО; его начало и есть момент замирания. `None`, если хвост короче
/// `min_tail_samples` (произведение менялось до самого конца) или ряд пуст.
pub fn product_frozen_after_frames(marker: i64, products: &[(i64, f64)], criteria: FrozenCriteria) -> Option<i64> {
    let (_, reference) = *products.last()?;
    let mut tail_start = products.len();
    for (index, (_, product)) in products.iter().enumerate().rev() {
        let within = relative_gap(*product, reference).is_some_and(|gap| gap <= criteria.tolerance_fraction);
        if !within {
            break;
        }
        tail_start = index;
    }
    let tail_len = products.len() - tail_start;
    if tail_len < criteria.min_tail_samples.max(1) {
        return None;
    }
    products.get(tail_start).map(|(frame, _)| frame - marker)
}

#[cfg(test)]
#[path = "three_a_results_tests.rs"]
mod tests;
