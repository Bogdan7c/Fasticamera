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

/// Бесшовные шаги 13b: настоящее долгое нажатие, колесо и щипок поверх тёплой второй камеры.
///
/// Пальцы ведутся по НАБЛЮДЕНИЮ (экран показывает значение → двигаем дальше), а не по сетке заранее
/// посчитанных точек: сколько пикселей колеса на единицу зума, знает только экран. Поэтому здесь шаги и
/// пределы, а не координаты цели. Целевые коэффициенты берутся из каталога зума в момент шага.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SeamlessPlan {
    /// Включать ли бесшовные шаги в план (после протяжки, перед завершающим тапом).
    pub enabled: bool,
    /// Сколько ждать, пока колесо откроется и тёплая станет готова после нажатия, мс. `[estimate]` 5000.
    pub ready_timeout_ms: u64,
    /// Сколько ждать, пока после «не нужно» колесо уйдёт и все тёплые закроются, мс. Обязано быть больше
    /// покоя колеса (≈ 2,15 с) плюс паузы движка (1,5 с). `[estimate]` 9000.
    pub release_timeout_ms: u64,
    /// Окно замера частоты обеих камер, пока тёплая готова, мс. `[estimate]` 1000.
    pub fps_window_ms: u64,
    /// Интервал между образцами пальцев, мс. `[estimate]` 16 — кадр 60 Гц.
    pub sample_interval_ms: u64,
    /// Предел образцов движения к цели; больше — провал «палец не довёл значение». `[estimate]` 160.
    pub max_drag_samples: u32,
    /// Сколько неподвижных образцов после достижения цели («продолжаем немного»). `[estimate]` 12.
    pub dwell_samples: u32,
    /// Сдвиг пальца по колесу за образец, доли ширины экрана. `[estimate]` 0,0045 (≈ 2 pt).
    pub wheel_step_dx: f64,
    /// Центр щипка (доли экрана): середина кадра видоискателя, не лента и не колесо.
    pub pinch_center: ScreenPoint,
    /// Половина начального расстояния между пальцами, доли ширины. `[estimate]` 0,15.
    pub pinch_start_half_width: f64,
    /// Относительное изменение расстояния между пальцами за образец. `[estimate]` 0,012.
    pub pinch_step: f64,
    /// Насколько (в тысячных коэффициента) экран может отличаться от отметки объектива в момент смены вверх по шкале.
    /// С готовой тёплой смена идёт ровно на отметке (решение 128); допуск — шаг щипка (1,2 %) и задержка события.
    /// Гистерезис селектора (5 %) заведомо не проходит. `[derived]` 30.
    pub switch_ratio_tolerance_milli: u32,
    /// Сколько мс колебаться около отметки главной камеры при замере дребезга (0 — шаг выключен). `[estimate]` 2000.
    pub hover_ms: u64,
    /// Полуширина колебаний около отметки, тысячные коэффициента: ±2 % — типичный дрожащий палец. `[estimate]` 20.
    pub hover_band_milli: u32,
    /// Наименьшая частота каждой задней камеры, пока стримят обе, к/с (решение владельца: 55 из 60).
    pub min_back_fps: f64,
    /// Сколько кадров можно пропустить на стыке «последний живой кадр старой камеры → первый живой новой»
    /// (0 — новый кадр строго на следующем месте). Считается целыми кадрами (`SwitchGap::frames_missed`), а не дробными
    /// периодами: дрожание времени показа не должно решать вердикт. `[estimate]` 1.
    pub max_frames_missed_at_switch: u32,
    /// Тап `.6 ↔ 1` с скрытыми кадрами больше этого — заметка, не провал (цель ≈ 2 после решения 118).
    pub tap_hidden_frames_note_above: u64,
}

impl Default for SeamlessPlan {
    fn default() -> Self {
        Self {
            enabled: true,
            ready_timeout_ms: 5_000,
            release_timeout_ms: 9_000,
            fps_window_ms: 1_000,
            sample_interval_ms: 16,
            max_drag_samples: 160,
            dwell_samples: 12,
            wheel_step_dx: 0.004_5,
            pinch_center: ScreenPoint::new(0.5, 0.4),
            pinch_start_half_width: 0.15,
            pinch_step: 0.012,
            switch_ratio_tolerance_milli: 30,
            hover_ms: 2_000,
            hover_band_milli: 20,
            min_back_fps: 55.0,
            max_frames_missed_at_switch: 1,
            tap_hidden_frames_note_above: 4,
        }
    }
}

impl SeamlessPlan {
    fn validate(&self) -> Result<(), ZoomScenarioConfigError> {
        for (reason, value) in [
            ("ready_timeout_ms должно быть больше нуля", self.ready_timeout_ms),
            ("release_timeout_ms должно быть больше нуля", self.release_timeout_ms),
            ("fps_window_ms должно быть больше нуля", self.fps_window_ms),
            ("sample_interval_ms должно быть больше нуля", self.sample_interval_ms),
            (
                "max_drag_samples должно быть больше нуля",
                u64::from(self.max_drag_samples),
            ),
        ] {
            if value == 0 {
                return Err(ZoomScenarioConfigError::Seamless { reason });
            }
        }
        let finite_positive = |value: f64| value.is_finite() && value > 0.0;
        if !finite_positive(self.wheel_step_dx) || self.wheel_step_dx >= 0.1 {
            return Err(ZoomScenarioConfigError::Seamless {
                reason: "wheel_step_dx должен быть конечным, больше 0 и меньше 0,1",
            });
        }
        if !(finite_positive(self.pinch_step) && self.pinch_step < 0.5) {
            return Err(ZoomScenarioConfigError::Seamless {
                reason: "pinch_step должен быть конечным, больше 0 и меньше 0,5",
            });
        }
        if !(finite_positive(self.pinch_start_half_width) && self.pinch_start_half_width < 0.5) {
            return Err(ZoomScenarioConfigError::Seamless {
                reason: "pinch_start_half_width должен быть в (0; 0,5)",
            });
        }
        let center_valid = (0.0..=1.0).contains(&self.pinch_center.x) && (0.0..=1.0).contains(&self.pinch_center.y);
        if !center_valid {
            return Err(ZoomScenarioConfigError::Seamless {
                reason: "центр щипка вне экрана",
            });
        }
        if self.hover_ms > 0 && self.hover_band_milli == 0 {
            return Err(ZoomScenarioConfigError::Seamless {
                reason: "hover_band_milli должно быть больше нуля, пока включён замер дребезга (hover_ms > 0)",
            });
        }
        if self.switch_ratio_tolerance_milli == 0 {
            return Err(ZoomScenarioConfigError::Seamless {
                reason: "switch_ratio_tolerance_milli должно быть больше нуля",
            });
        }
        if !finite_positive(self.min_back_fps) {
            return Err(ZoomScenarioConfigError::Seamless {
                reason: "min_back_fps должно быть конечным и больше нуля",
            });
        }
        Ok(())
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
    /// Бесшовные шаги тёплой камеры (13b).
    pub seamless: SeamlessPlan,
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
            seamless: SeamlessPlan::default(),
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
    #[error("zoom.seamless: {reason}")]
    Seamless { reason: &'static str },
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
        self.wheel_drag.validate()?;
        self.seamless.validate()
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
