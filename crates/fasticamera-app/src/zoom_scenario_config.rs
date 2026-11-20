//! Секция `camera.scenarios.zoom`: параметры smoke-сценария `zoom` (сессия 13) — переключение объективов и зум на
//! настоящем экране камеры. Сценарий кладёт в экран действия (тап по кнопке ленты, переворот) и настоящие касания
//! (протяжка пальцем по ленте — открывает колесо), а судит по тому, что видно снаружи: события движка, статус
//! движка, показанные кадры видоискателя (`ViewfinderDiagnostics::continuity`), счётчики исполнителя эффектов.
//!
//! Числа порогов — `[estimate]`: настоящие времена переключения замеряет 13a на телефоне, и пороги тогда уточнятся.

use serde::{Deserialize, Serialize};

use crate::screen_scenario_config::ScreenPoint;

/// Протяжка пальцем по ленте зума: открывает колесо и ведёт значение. Единственное место, где сценарий водит
/// настоящим касанием: ТИКИ ХАПТИКИ рождает только колесо под пальцем (действия `ZoomWheelDragged` их не дают).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WheelDragPlan {
    /// Где палец ложится на ленту. `[derived]` центр ленты упоров на нашем экране: x = 0,5; y = 1808 px / 2412 px =
    /// 0,7496 — у golden `rest_4x3` подпись «1×» стоит в (540; 1808) px; координаты — доли экрана, поэтому верны и на
    /// 1440 × 3216 (то же отношение сторон). Функциональный тест протягивает ровно эту точку по настоящему экрану.
    pub start: ScreenPoint,
    /// На сколько доли ширины экрана палец сдвигается за один образец (отрицательное — влево). `[derived]` влево — это
    /// приближение: 5 pt на образец (5 / 440 = 0,01136), 30 образцов дают ≈ 150 pt и ведут значение с 1× к ≈ 5×
    /// без пересечения упора ширика (смена объектива в этом шаге не нужна).
    pub step_dx: f64,
    /// Сколько образцов `Moved` между `Began` и `Ended`.
    pub samples: u32,
    /// Через сколько мс после предыдущего образца кладётся следующий (метки касаний идут с этим шагом).
    /// `[estimate]` 16 — как кадр 60 Гц, в котором экран разбирает касания.
    pub sample_interval_ms: u64,
}

impl Default for WheelDragPlan {
    fn default() -> Self {
        Self {
            start: ScreenPoint::new(0.5, 0.7496),
            step_dx: -0.011_36,
            samples: 30,
            sample_interval_ms: 16,
        }
    }
}

/// Секция `camera.scenarios.zoom`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ZoomScenarioConfig {
    /// Коэффициенты, на которые по очереди тапает сценарий (ищется ближайший упор ленты). Умолчание `.6, 1, 2, 1, .6`
    /// — все переходы между объективами и 2× без смены камеры.
    pub stop_ratios: Vec<f32>,
    /// Переворот на фронтальную и обратно после тапов.
    pub flip_round_trip: bool,
    /// Протяжка по ленте для проверки хаптики; `None`-аналога нет: шаг выключается нулём образцов.
    pub wheel_drag: WheelDragPlan,
    /// Сколько ждать готовности: поток на основной камере, экран видит живую камеру, рендер отдаёт непрерывность.
    pub ready_timeout_ms: u64,
    /// Сколько ждать завершения одного шага.
    pub step_timeout_ms: u64,
    /// Сколько продолжать наблюдать после первого живого кадра новой камеры: чёрные и удержанные кадры, частота.
    pub step_settle_ms: u64,
    /// Порог «действие → событие движка `LensSwitched`», мс. `[estimate]` 1500 — телефон в 13a покажет цифру.
    pub max_switch_ms: u64,
    /// Порог «действие → первый живой кадр новой камеры на экране», мс. `[estimate]` 2000.
    pub max_first_live_frame_ms: u64,
    /// Ожидаемая частота задних камер, к/с (4:3@60).
    pub back_frame_rate: u32,
    /// Ожидаемая частота фронтальной, к/с (фронталка держит 30; решение по каталогу 00).
    pub front_frame_rate: u32,
    /// Допустимое отставание измеренной частоты от ожидаемой (доля). `[estimate]` 0,1.
    pub frame_rate_tolerance: f64,
    /// Наименьший промежуток между вибрациями, мс (решение 112: не чаще одного тика за 30 мс).
    pub min_haptic_gap_ms: u64,
    /// Сколько вибраций обязана дать протяжка по колесу, чтобы шаг считался доказательством хаптики.
    pub min_haptics_in_drag: u64,
}

impl Default for ZoomScenarioConfig {
    fn default() -> Self {
        Self {
            stop_ratios: vec![0.6, 1.0, 2.0, 1.0, 0.6],
            flip_round_trip: true,
            wheel_drag: WheelDragPlan::default(),
            ready_timeout_ms: 15_000,
            step_timeout_ms: 6_000,
            step_settle_ms: 700,
            max_switch_ms: 1_500,
            max_first_live_frame_ms: 2_000,
            back_frame_rate: 60,
            front_frame_rate: 30,
            frame_rate_tolerance: 0.1,
            min_haptic_gap_ms: 30,
            min_haptics_in_drag: 2,
        }
    }
}

/// Неверная секция `camera.scenarios.zoom`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ZoomScenarioConfigError {
    #[error("zoom.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("zoom.stop_ratios: коэффициент {value} должен быть конечным и больше нуля")]
    InvalidStopRatio { value: f32 },
    #[error("zoom.stop_ratios не должен быть пустым")]
    NoStops,
    #[error("zoom.frame_rate_tolerance = {value}: нужна доля от 0 (не включая) до 1 (не включая)")]
    ToleranceOutOfRange { value: f64 },
    #[error("zoom.wheel_drag: {reason}")]
    WheelDrag { reason: &'static str },
}

impl ZoomScenarioConfig {
    pub fn validate(&self) -> Result<(), ZoomScenarioConfigError> {
        for (field, value) in [
            ("ready_timeout_ms", self.ready_timeout_ms),
            ("step_timeout_ms", self.step_timeout_ms),
            ("step_settle_ms", self.step_settle_ms),
            ("max_switch_ms", self.max_switch_ms),
            ("max_first_live_frame_ms", self.max_first_live_frame_ms),
            ("min_haptic_gap_ms", self.min_haptic_gap_ms),
            ("min_haptics_in_drag", self.min_haptics_in_drag),
            ("back_frame_rate", u64::from(self.back_frame_rate)),
            ("front_frame_rate", u64::from(self.front_frame_rate)),
        ] {
            if value == 0 {
                return Err(ZoomScenarioConfigError::Zero { field });
            }
        }
        if self.stop_ratios.is_empty() {
            return Err(ZoomScenarioConfigError::NoStops);
        }
        for value in &self.stop_ratios {
            // Через сравнение: NaN не проходит.
            if !(value.is_finite() && *value > 0.0) {
                return Err(ZoomScenarioConfigError::InvalidStopRatio { value: *value });
            }
        }
        let tolerance = self.frame_rate_tolerance;
        if !(tolerance.is_finite() && tolerance > 0.0 && tolerance < 1.0) {
            return Err(ZoomScenarioConfigError::ToleranceOutOfRange { value: tolerance });
        }
        self.wheel_drag.validate()
    }
}

impl WheelDragPlan {
    fn validate(&self) -> Result<(), ZoomScenarioConfigError> {
        let start_valid = (0.0..=1.0).contains(&self.start.x) && (0.0..=1.0).contains(&self.start.y);
        if !start_valid {
            return Err(ZoomScenarioConfigError::WheelDrag {
                reason: "точка старта вне экрана (доли должны лежать в 0..=1)",
            });
        }
        if !self.step_dx.is_finite() || self.step_dx == 0.0 {
            return Err(ZoomScenarioConfigError::WheelDrag {
                reason: "сдвиг на образец должен быть конечным и не нулевым",
            });
        }
        // Конечная точка протяжки обязана остаться на экране, иначе образцы «уходят за край».
        let end_x = self.start.x + self.step_dx * f64::from(self.samples);
        if !(0.0..=1.0).contains(&end_x) {
            return Err(ZoomScenarioConfigError::WheelDrag {
                reason: "протяжка выходит за край экрана",
            });
        }
        if self.samples > 0 && self.sample_interval_ms == 0 {
            return Err(ZoomScenarioConfigError::WheelDrag {
                reason: "интервал образцов должен быть больше нуля",
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "zoom_scenario_config_tests.rs"]
mod tests;
