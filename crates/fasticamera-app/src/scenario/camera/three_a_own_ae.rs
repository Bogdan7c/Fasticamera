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
//! каждый результат ЛЮБОГО происхождения (повторяющийся запрос, триггер фокуса, серия, неопознанный) с
//! активным `AE_STATE` — доказательство, что в HAL ушёл запрос с вендорским AE. Каждый новый результат
//! проверяется сразу, как появился ([`OwnAeWatch::observe_result`]), а не задним числом по журналу:
//! журнал ограничен по длине и хранит только повторяющиеся результаты, поэтому слепой зоны нет.
//! Ограничение: сценарий видит каждый третий кадр (тик), а не все.

use camera_contract::three_a::{AeAfLock, AeState, RequestOrigin};
use camera_core::three_a_status::{
    AdjustmentReason, AutoExposureProgress, ControlParameter, LastThreeAResult, ThreeACounters, ThreeAStatus,
};
use serde::Serialize;

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

/// Сколько результатов проверено на вендорский AE, по происхождению запроса.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct ResultsChecked {
    /// Повторяющийся запрос потока.
    pub repeating: usize,
    /// Разовый запрос с триггером автофокуса (тап).
    pub focus_trigger: usize,
    /// Кадры серии (брекетинг).
    pub burst: usize,
    /// Разовый запрос снимка со вспышкой (14d).
    pub flash_still: usize,
    /// Запрос не опознан.
    pub unknown: usize,
}

impl ResultsChecked {
    /// Учитывает один результат в счётчике его происхождения.
    fn count(&mut self, origin: RequestOrigin) {
        let counter = match origin {
            RequestOrigin::Repeating => &mut self.repeating,
            RequestOrigin::FocusTrigger { .. } => &mut self.focus_trigger,
            RequestOrigin::Burst { .. } => &mut self.burst,
            RequestOrigin::FlashStill { .. } => &mut self.flash_still,
            RequestOrigin::Unknown => &mut self.unknown,
        };
        *counter += 1;
    }
}

/// Диапазон сырого замера сцены за окно наблюдения: свидетельство, что сцена за окно действительно менялась.
///
/// Значения — log2 от «выдержка × ISO» (см. `AutoExposureDiagnostics::observed_scene_log2_seconds_iso`),
/// поэтому разность максимума и минимума — это EV.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ObservedSceneRange {
    extremes: Option<(f64, f64)>,
}

impl ObservedSceneRange {
    /// Один замер. Нечисловое значение (NaN, бесконечность) замером не считается: диапазон не ломаем.
    pub fn record(&mut self, log2_seconds_iso: f64) {
        if !log2_seconds_iso.is_finite() {
            return;
        }
        self.extremes = Some(match self.extremes {
            None => (log2_seconds_iso, log2_seconds_iso),
            Some((lowest, highest)) => (lowest.min(log2_seconds_iso), highest.max(log2_seconds_iso)),
        });
    }

    /// Размах замеров, EV; `None` — ни одного замера (например, вендорский AE замера сцены не даёт).
    pub fn range_ev(&self) -> Option<f64> {
        self.extremes.map(|(lowest, highest)| highest - lowest)
    }
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
    /// Проверено результатов на вендорский AE, по происхождению.
    results_checked: ResultsChecked,
    /// Из них с активным `AE_STATE` вендора, любое происхождение.
    vendor_ae_results: usize,
    /// Кадр первого такого результата (по порядку прихода).
    first_vendor_ae_frame: Option<i64>,
    /// Счётчики движка на последнем тике: учёт **каждого** результата с запуска движка, без выборки
    /// (сценарий видит лишь последний результат на тик, ~каждый третий кадр).
    engine_counters: Option<ThreeACounters>,
}

impl OwnAeWatch {
    /// Новый результат HAL (любого происхождения, каждый кадр — один раз): проверяем на вендорский AE сразу.
    pub fn observe_result(&mut self, result: &LastThreeAResult) {
        self.results_checked.count(result.origin);
        if result.ae_state.is_some_and(is_vendor_ae_active) {
            self.vendor_ae_results += 1;
            self.first_vendor_ae_frame.get_or_insert(result.frame_number.0);
        }
    }

    /// Один тик: смотрим состояние нашего AE и поправки.
    pub fn observe(&mut self, status: &ThreeAStatus) {
        self.engine_counters = Some(status.counters);
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
    pub fn report(&self) -> Option<OwnAeReport> {
        if !self.seen {
            return None;
        }
        Some(OwnAeReport {
            progress_at_end: self.last_progress,
            settled_seen: self.settled,
            holding_seen: self.holding,
            limit_reached_seen: self.limit_reached,
            worst_shortfall_ev: self.worst_shortfall_ev,
            target_not_reached_ticks: self.target_not_reached_ticks,
            results_checked: self.results_checked,
            vendor_ae_results: self.vendor_ae_results,
            first_vendor_ae_frame: self.first_vendor_ae_frame,
            engine: self.engine_counters.as_ref().map(EngineAeCount::since_engine_start),
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
    /// Сколько результатов проверено на вендорский AE, по происхождению запроса.
    pub results_checked: ResultsChecked,
    /// Из них с активным `AE_STATE` вендора, любое происхождение (должно быть 0).
    pub vendor_ae_results: usize,
    pub first_vendor_ae_frame: Option<i64>,
    /// Учёт движка по каждому результату за прогон (`None` — статуса не видели ни разу).
    pub engine: Option<EngineAeCount>,
}

/// Учёт движка **с его запуска**, а не за прогон: выбор `own / vendor` фиксирован на процесс, поэтому
/// любой результат с активным AE HAL за всю жизнь движка — нарушение. Так в учёт входят и кадры до первого
/// тика сценария (автостарт видоискателя, запуск потока), и разность не прячет перезапуск счётчиков.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct EngineAeCount {
    /// Результатов кадров любого происхождения с запуска движка.
    pub results_observed: u64,
    /// Из них с активным `AE_STATE` HAL — при нашем AE должно быть 0.
    pub results_with_hal_ae_active: u64,
    /// Предложений AE, отвергнутых как измеренных на другой камере (13b). Сценарий `3a` идёт на одной камере: 0.
    pub foreign_camera_proposals_rejected: u64,
}

impl EngineAeCount {
    /// Снимок счётчиков движка (последний тик сценария).
    pub fn since_engine_start(counters: &ThreeACounters) -> Self {
        Self {
            results_observed: counters.results_observed,
            results_with_hal_ae_active: counters.results_with_hal_ae_active,
            foreign_camera_proposals_rejected: counters.ae_foreign_camera_proposals_rejected,
        }
    }
}

/// Нарушения правил нашего AE (одно правило — одно нарушение).
pub fn own_ae_violations(report: &OwnAeReport) -> Vec<String> {
    let mut violations = Vec::new();
    if report.vendor_ae_results != 0 {
        violations.push(format!(
            "наш AE: {} результатов с активным AE_STATE вендора (первый — кадр {:?}): \
             в HAL ушёл запрос с вендорским AE",
            report.vendor_ae_results, report.first_vendor_ae_frame
        ));
    }
    match report.engine {
        Some(engine) if engine.results_with_hal_ae_active != 0 => violations.push(format!(
            "наш AE: движок с запуска насчитал {} результатов с активным AE HAL из {}: в HAL ушёл запрос с вендорским AE",
            engine.results_with_hal_ae_active, engine.results_observed
        )),
        Some(engine) if engine.results_observed == 0 => {
            violations.push("наш AE: движок не насчитал ни одного результата с запуска".to_owned())
        }
        Some(engine) if engine.foreign_camera_proposals_rejected != 0 => violations.push(format!(
            "наш AE: отвергнуто {} предложений как измеренных на другой камере: в сценарии с одной камерой должно быть 0",
            engine.foreign_camera_proposals_rejected
        )),
        Some(_) => {}
        None => violations.push("наш AE: нет счётчиков движка — учёт «каждого результата» не проверен".to_owned()),
    }
    if report.results_checked.repeating == 0 {
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
