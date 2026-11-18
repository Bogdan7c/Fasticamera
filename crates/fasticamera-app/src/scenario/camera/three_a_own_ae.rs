//! Наш AE в сценарии `3a` (сессия 08a): откуда сценарий знает, кто ведёт экспозицию, какие признаки
//! «сошлось», «заморожено», «упёрлись в предел» он берёт и как проверяет, что вендорский AE не участвовал.
//!
//! Кто ведёт экспозицию, сценарий определяет по **факту**, а не по конфигу: `ThreeAStatus::auto_exposure`
//! равен `Some`, только пока в движке стоит наш исполнитель (вендорский отдаёт `None`). Поэтому
//! сценарию не нужен доступ к `camera.exposure_controller`, а прогон с вендорским AE идёт по-старому
//! (маркеры HAL `AE_STATE`).
//!
//! # Признак «ни одного запроса с вендорским AE»
//! Прямого журнала запросов сценарию не дают, но HAL отвечает на запрос честно: при `AE_MODE_OFF`
//! (наш план `ExposurePlan::Sensor`) в результате `AE_STATE = Inactive`, а при вендорском AE
//! (`AE_MODE_ON`) — `Searching`/`Converged`/`Locked`/`Precapture`/`FlashRequired`. Поэтому
//! каждый результат повторяющегося запроса с активным `AE_STATE` — доказательство, что в HAL ушёл
//! запрос с вендорским AE. Ограничение: журнал сценария видит каждый третий кадр (тик), а не все.

use camera_contract::three_a::{AeAfLock, AeState};
use camera_core::three_a_status::{AdjustmentReason, AutoExposureProgress, ControlParameter, ThreeAStatus};
use serde::Serialize;

use super::three_a_results::ResultLog;
use crate::auto_exposure_link::ExposureControllerKind;

/// Кто ведёт экспозицию в этом прогоне.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AeSource {
    /// Вендорский AE через HAL: маркеры — `AE_STATE`.
    Vendor,
    /// Наш AE: маркеры — состояние в статусе движка, HAL при `AE_MODE_OFF` молчит (`Inactive`).
    Own,
}

impl AeSource {
    /// Кто ведёт экспозицию по статусу движка на этом тике.
    pub fn of(status: &ThreeAStatus) -> Self {
        if status.auto_exposure.is_some() {
            Self::Own
        } else {
            Self::Vendor
        }
    }
}

impl From<ExposureControllerKind> for AeSource {
    fn from(kind: ExposureControllerKind) -> Self {
        match kind {
            ExposureControllerKind::Own => Self::Own,
            ExposureControllerKind::Vendor => Self::Vendor,
        }
    }
}

/// Блокировка нашего AE достигнута: движок принял команду (`lock == Locked`) **и** замер заморозил цель
/// (`Holding`). Один `Holding` без `lock` мог остаться от прошлого шага (устаревший статус), а
/// `LimitReached` при блокировке движок не показывает вместо `Holding`.
pub fn is_lock_reached(status: &ThreeAStatus) -> bool {
    status.lock == AeAfLock::Locked && matches!(status.auto_exposure, Some(AutoExposureProgress::Holding))
}

/// Цель нашего AE достигнута: сошлась со сценой или заморожена блокировкой.
pub fn is_converged(progress: Option<AutoExposureProgress>) -> bool {
    matches!(
        progress,
        Some(AutoExposureProgress::Settled | AutoExposureProgress::Holding)
    )
}

/// Пределов камеры не хватило: это честный итог, а не «не сошлось».
pub fn is_limit_reached(progress: Option<AutoExposureProgress>) -> bool {
    matches!(progress, Some(AutoExposureProgress::LimitReached { .. }))
}

/// Активен ли вендорский автомат в этом состоянии HAL (`Inactive` — автомат выключен, `AE_MODE_OFF`).
pub fn is_vendor_ae_active(state: AeState) -> bool {
    !matches!(state, AeState::Inactive)
}

/// Что сценарий наблюдал о нашем AE за прогон. Собирается каждый тик, итог — [`OwnAeReport`].
#[derive(Debug, Default)]
pub struct OwnAeWatch {
    /// Хоть раз видели состояние нашего AE в статусе.
    seen: bool,
    settled: bool,
    holding: bool,
    limit_reached: bool,
    /// Наибольшая по модулю нехватка пределов, EV (знак сохранён).
    worst_shortfall_ev: Option<f32>,
    /// Сколько тиков статус содержал поправку `TargetNotReached`.
    target_not_reached_ticks: u64,
    last_progress: Option<AutoExposureProgress>,
}

impl OwnAeWatch {
    /// Один тик: смотрим состояние нашего AE и поправки.
    pub fn observe(&mut self, status: &ThreeAStatus) {
        let Some(progress) = status.auto_exposure else {
            return;
        };
        self.seen = true;
        self.last_progress = Some(progress);
        match progress {
            AutoExposureProgress::Settled => self.settled = true,
            AutoExposureProgress::Holding => self.holding = true,
            AutoExposureProgress::LimitReached { shortfall_ev } => {
                self.limit_reached = true;
                let worse = self
                    .worst_shortfall_ev
                    .is_none_or(|worst| shortfall_ev.abs() > worst.abs());
                if worse {
                    self.worst_shortfall_ev = Some(shortfall_ev);
                }
            }
            AutoExposureProgress::AwaitingMeasurement
            | AutoExposureProgress::Searching
            | AutoExposureProgress::Manual => {}
        }
        let not_reached = status.adjustments.iter().any(|adjustment| {
            adjustment.parameter == ControlParameter::ExposureTarget
                && adjustment.reason == AdjustmentReason::TargetNotReached
        });
        if not_reached {
            self.target_not_reached_ticks += 1;
        }
    }

    /// Итог для отчёта; `None`, если наш AE не вёл экспозицию (прогон с вендорским).
    pub fn report(&self, results: &ResultLog) -> Option<OwnAeReport> {
        if !self.seen {
            return None;
        }
        let vendor_results: Vec<_> = results
            .repeating_after(None)
            .filter(|result| result.ae_state.is_some_and(is_vendor_ae_active))
            .collect();
        Some(OwnAeReport {
            progress_at_end: self.last_progress,
            settled_seen: self.settled,
            holding_seen: self.holding,
            limit_reached_seen: self.limit_reached,
            worst_shortfall_ev: self.worst_shortfall_ev,
            target_not_reached_ticks: self.target_not_reached_ticks,
            repeating_results_checked: results.repeating_after(None).count(),
            vendor_ae_results: vendor_results.len(),
            first_vendor_ae_frame: vendor_results.first().map(|result| result.frame_number.0),
        })
    }
}

/// Отчёт о нашем AE за прогон `3a`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OwnAeReport {
    /// Состояние на последнем тике.
    pub progress_at_end: Option<AutoExposureProgress>,
    pub settled_seen: bool,
    pub holding_seen: bool,
    /// Хоть раз пределов камеры не хватило (`LimitReached`, поправка `TargetNotReached`).
    pub limit_reached_seen: bool,
    /// Наибольшая нехватка, EV (< 0 темнее нужного, > 0 светлее).
    pub worst_shortfall_ev: Option<f32>,
    /// Тиков, где в статусе была поправка `TargetNotReached`.
    pub target_not_reached_ticks: u64,
    /// Сколько результатов повторяющегося запроса проверено на вендорский AE.
    pub repeating_results_checked: usize,
    /// Из них с активным `AE_STATE` вендора (должно быть 0).
    pub vendor_ae_results: usize,
    pub first_vendor_ae_frame: Option<i64>,
}

/// Нарушения правил нашего AE (одно правило — одно нарушение).
pub fn own_ae_violations(report: &OwnAeReport) -> Vec<String> {
    let mut violations = Vec::new();
    if report.vendor_ae_results != 0 {
        violations.push(format!(
            "наш AE: {} результатов повторяющегося запроса с активным AE_STATE вендора (первый — кадр {:?}): \
             в HAL ушёл запрос с вендорским AE",
            report.vendor_ae_results, report.first_vendor_ae_frame
        ));
    }
    if report.repeating_results_checked == 0 {
        violations
            .push("наш AE: нет результатов повторяющегося запроса, проверять «нет вендорского AE» нечем".to_owned());
    }
    // Сошёлся, заморозился или честно упёрся в предел — любой из трёх нормален; «ни одного» — AE не работал
    // (например, предложения рендера не доходят до движка).
    if !(report.settled_seen || report.holding_seen || report.limit_reached_seen) {
        violations.push(format!(
            "наш AE за весь прогон не дошёл ни до Settled, ни до Holding, ни до LimitReached (в конце: {:?})",
            report.progress_at_end
        ));
    }
    violations
}

#[cfg(test)]
#[path = "three_a_own_ae_tests.rs"]
mod tests;
