//! Секция `camera.scenarios.screen`: параметры smoke-сценария `screen` (сессия 12) — сам экран камеры iOS поверх
//! живого видоискателя, которым на телефоне «управляет» скрипт (`scripts/smoke_driver.py`: тапы, свайпы, клавиша).
//!
//! Видоискатель, режим камеры и пороги его ровности берутся из `camera.scenarios.viewfinder` (как у `glass`):
//! сценарий стартует тот же продуктовый видоискатель и судит его теми же порогами. Здесь только то, что относится
//! к экрану: сроки шагов, бюджет GPU-времени UI и **план касаний** — куда именно скрипт нажимает на экране.
//!
//! # План касаний ([`ScreenTouchPlan`])
//! Точки — доли экрана (0 — левый/верхний край, 1 — правый/нижний), как в драйвере. Rust-сценарий передаёт их
//! драйверу в поле `gesture` стадии `screen_gesture`, поэтому числа живут ТОЛЬКО здесь, а не в двух местах.
//! Все значения выведены из раскладки экрана (`camera_screen::ScreenLayout` и `WidgetLayoutConfig`) для экрана
//! OnePlus 11 (1440 × 3216 px → 440 × 982,67 pt, привязка к низу — решение 82) и помечены `[derived]` с формулой.
//! Тесты (`event_router_screen_tests.rs`) нажимают РОВНО эти точки на настоящем экране, поэтому неверное число
//! (например, после правки раскладки экрана) даёт красный функциональный тест, а не тихий промах на телефоне.
//! Раскладка экрана ещё меняется: при красном тесте «шаг промахнулся» сначала сверяйте эти числа с `ScreenGeometry`.

use serde::{Deserialize, Serialize};

/// Точка экрана в долях (оба числа в `0..=1`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScreenPoint {
    /// Доля ширины экрана слева.
    pub x: f64,
    /// Доля высоты экрана сверху.
    pub y: f64,
}

impl ScreenPoint {
    /// Точка из двух долей (проверка диапазона — в [`ScreenTouchPlan::validate`]).
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    fn is_valid(self) -> bool {
        // Через сравнение: NaN не проходит.
        (0.0..=1.0).contains(&self.x) && (0.0..=1.0).contains(&self.y)
    }
}

/// Куда в прямоугольнике КАДРА 4:3 попадают тапы фокуса: доли кадра (0 — левый/верхний край кадра, 1 — правый/нижний).
/// Сценарий выводит из них ожидаемую точку сенсора независимо от кода экрана и сверяет с точкой интереса движка.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FocusFrameTargets {
    /// `[derived]` центр кадра: (0,5; 0,5) — тот же тап, что `focus_center` (середина кадра 4:3).
    pub center: ScreenPoint,
    /// `[derived]` четверть ширины и четверть высоты кадра (см. `focus_top_left`).
    pub top_left: ScreenPoint,
    /// `[derived]` три четверти и три четверти кадра (см. `focus_bottom_right`).
    pub bottom_right: ScreenPoint,
}

impl Default for FocusFrameTargets {
    fn default() -> Self {
        Self {
            center: ScreenPoint::new(0.5, 0.5),
            top_left: ScreenPoint::new(0.25, 0.25),
            bottom_right: ScreenPoint::new(0.75, 0.75),
        }
    }
}

/// Долгое нажатие: точка и сколько держать палец.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HoldGesture {
    pub at: ScreenPoint,
    /// Сколько мс держать. `[doc]` порог долгого тапа экрана — 500 мс (`LongPressSpec::min_duration_ms`);
    /// 800 — запас на задержку `adb input swipe x y x y`.
    pub hold_ms: u64,
}

/// Свайп из точки в точку.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwipeGesture {
    pub from: ScreenPoint,
    pub to: ScreenPoint,
    /// За сколько мс провести. `[estimate]` 300 — достаточно для десятков промежуточных точек `input swipe`.
    pub duration_ms: u64,
}

/// Куда нажимает скрипт. Каждое поле — один жест, который сценарий просит у драйвера отдельной стадией.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScreenTouchPlan {
    /// Тап фокуса: центр кадра 4:3. `[derived]` x = 0,5; y = (верх + 0,5·высота кадра) / высота экрана =
    /// (183,0 + 293,3) / 982,67 = 0,4847, где кадр 4:3 = 183,0…769,67 pt (`ScreenLayout::frame_rect`).
    pub focus_center: ScreenPoint,
    /// Тап фокуса: левый верх кадра (четверть ширины, четверть высоты кадра). `[derived]` y = (183,0 + 146,7) /
    /// 982,67 = 0,3355.
    pub focus_top_left: ScreenPoint,
    /// Тап фокуса: правый низ кадра (три четверти и три четверти). `[derived]` y = (183,0 + 440,0) / 982,67 = 0,6340.
    pub focus_bottom_right: ScreenPoint,
    /// Где в кадре 4:3 лежат три тапа фокуса (для проверки МЕСТА, куда ушла точка интереса).
    pub focus_targets: FocusFrameTargets,
    /// Допуск проверки места точки по каждой оси, в долях сенсора. `[estimate]` 0,05: перепутанные оси или зеркало
    /// сдвигают точку минимум на 0,25–0,5 (тапы стоят на четвертях кадра), а округление шкал и кроп — на сотые доли.
    pub focus_point_tolerance: f64,
    /// Долгий тап — блокировка AE/AF — в центре кадра.
    pub lock_press: HoldGesture,
    /// Свайп вверх по видоискателю — открыть меню Controls. `[derived]` от y = 0,62 (внутри кадра, ниже центра)
    /// до y = 0,40: 216 pt — заметно больше порога пана (единицы pt).
    pub menu_open_swipe: SwipeGesture,
    /// Свайп вниз по шторке меню — закрыть. `[derived]` шторка 382 × 416 pt, верх на 532,6 pt (y = 0,542);
    /// от y = 0,63 (внутри шторки) до y = 0,92.
    pub menu_close_swipe: SwipeGesture,
    /// Значок EV над кадром. `[derived]` ряд значков по умолчанию [Flash, Raw, Exposure] (таймер выключен —
    /// скрыт): EV третий, центр x = (220,0 + 51,55) / 440 = 0,6172; y = (104,9 + 26,67) / 982,67 = 0,1339.
    pub ev_icon: ScreenPoint,
    /// Свайп по шкале панели Exposure. `[derived]` шкала: x = 108…332 pt (панель 400 pt без боковых отступов шкалы
    /// по 88 pt), центр по высоте = 926,6 / 982,67 = 0,9429; от x = 0,65 (286 pt) до x = 0,35 (154 pt): 132 pt — несколько
    /// шагов шкалы по 1/3 EV, жест целиком внутри шкалы.
    pub ev_slider_swipe: SwipeGesture,
    /// Кнопка сброса EV (левый верх панели). `[derived]` панель 400 × 72 pt (x = 20…420, y = 876,6…948,6); кнопка
    /// Ø 36 pt на 56 pt от края: x = (58 + 94) / 2 / 440 = 0,1727; y = 912,6 / 982,67 = 0,9287.
    pub ev_reset: ScreenPoint,
    /// Кнопка «×» панели Exposure (правый верх). `[derived]` x = (346 + 382) / 2 / 440 = 0,8273; y — как у сброса.
    pub ev_close: ScreenPoint,
    /// Тап-проба по кадру (проверяет, закрыта ли панель: при открытой панели тап только закрывает её и камере
    /// ничего не шлёт, при закрытой — наводит фокус). Верхняя половина кадра, чтобы не попасть под шторку меню.
    pub probe_tap: ScreenPoint,
    /// Горизонтальный свайп по видоискателю справа налево (смена режима «вправо»).
    pub modes_swipe_left: SwipeGesture,
    /// Горизонтальный свайп слева направо.
    pub modes_swipe_right: SwipeGesture,
}

impl Default for ScreenTouchPlan {
    fn default() -> Self {
        let swipe = |from: (f64, f64), to: (f64, f64), duration_ms: u64| SwipeGesture {
            from: ScreenPoint::new(from.0, from.1),
            to: ScreenPoint::new(to.0, to.1),
            duration_ms,
        };
        Self {
            focus_center: ScreenPoint::new(0.5, 0.4847),
            focus_top_left: ScreenPoint::new(0.25, 0.3355),
            focus_bottom_right: ScreenPoint::new(0.75, 0.6340),
            focus_targets: FocusFrameTargets::default(),
            focus_point_tolerance: 0.05,
            lock_press: HoldGesture {
                at: ScreenPoint::new(0.5, 0.4847),
                hold_ms: 800,
            },
            menu_open_swipe: swipe((0.5, 0.62), (0.5, 0.40), 300),
            menu_close_swipe: swipe((0.5, 0.63), (0.5, 0.92), 300),
            ev_icon: ScreenPoint::new(0.6172, 0.1339),
            ev_slider_swipe: swipe((0.65, 0.9429), (0.35, 0.9429), 400),
            ev_reset: ScreenPoint::new(0.1727, 0.9287),
            ev_close: ScreenPoint::new(0.8273, 0.9287),
            probe_tap: ScreenPoint::new(0.5, 0.3355),
            modes_swipe_left: swipe((0.75, 0.45), (0.25, 0.45), 250),
            modes_swipe_right: swipe((0.25, 0.45), (0.75, 0.45), 250),
        }
    }
}

impl ScreenTouchPlan {
    /// Все точки плана с понятными именами (для проверки диапазона и для тестов).
    pub fn named_points(&self) -> Vec<(&'static str, ScreenPoint)> {
        let mut points = vec![
            ("focus_center", self.focus_center),
            ("focus_top_left", self.focus_top_left),
            ("focus_bottom_right", self.focus_bottom_right),
            ("lock_press", self.lock_press.at),
            ("ev_icon", self.ev_icon),
            ("ev_reset", self.ev_reset),
            ("ev_close", self.ev_close),
            ("probe_tap", self.probe_tap),
        ];
        for (name, swipe) in self.swipes() {
            points.push((name, swipe.from));
            points.push((name, swipe.to));
        }
        points
    }

    /// Все свайпы плана.
    pub fn swipes(&self) -> [(&'static str, SwipeGesture); 5] {
        [
            ("menu_open_swipe", self.menu_open_swipe),
            ("menu_close_swipe", self.menu_close_swipe),
            ("ev_slider_swipe", self.ev_slider_swipe),
            ("modes_swipe_left", self.modes_swipe_left),
            ("modes_swipe_right", self.modes_swipe_right),
        ]
    }

    fn validate(&self) -> Result<(), ScreenScenarioConfigError> {
        for (name, point) in [
            ("focus_targets.center", self.focus_targets.center),
            ("focus_targets.top_left", self.focus_targets.top_left),
            ("focus_targets.bottom_right", self.focus_targets.bottom_right),
        ] {
            if !point.is_valid() {
                return Err(ScreenScenarioConfigError::PointOutOfRange {
                    gesture: name,
                    x: point.x,
                    y: point.y,
                });
            }
        }
        // Через сравнение: NaN не проходит. Допуск 0,5 и больше принял бы любую точку сенсора.
        let tolerance = self.focus_point_tolerance;
        if !(tolerance.is_finite() && tolerance > 0.0 && tolerance < 0.5) {
            return Err(ScreenScenarioConfigError::FocusToleranceOutOfRange { value: tolerance });
        }
        for (name, point) in self.named_points() {
            if !point.is_valid() {
                return Err(ScreenScenarioConfigError::PointOutOfRange {
                    gesture: name,
                    x: point.x,
                    y: point.y,
                });
            }
        }
        let durations = self
            .swipes()
            .map(|(name, swipe)| (name, swipe.duration_ms))
            .into_iter()
            .chain([("lock_press", self.lock_press.hold_ms)]);
        for (name, duration_ms) in durations {
            if duration_ms == 0 {
                return Err(ScreenScenarioConfigError::ZeroGestureDuration { gesture: name });
            }
        }
        Ok(())
    }
}

/// Секция `camera.scenarios.screen`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScreenScenarioConfig {
    /// Потолок длины всего сценария от «видоискатель живой», мс. Сценарий кончается, когда прошли все шаги; это
    /// только защита от зависшего скрипта (он же — длина окна замера видоискателя). `[estimate]` 2 минуты:
    /// 18 шагов × (задержка скрипта ≈ 1 с + жест ≤ 0,8 с + выдержка) укладываются с запасом.
    pub max_duration_ms: u64,
    /// Сколько ждать результата ОДНОГО шага после его стадии, мс. `[estimate]` 8 с: стадия идёт в logcat, скрипт её
    /// читает, спрашивает `wm size` и вызывает `adb` (≈ 1–2 с) и сам жест до 0,8 с.
    pub step_timeout_ms: u64,
    /// Выдержка после выполненного шага, мс: анимации (шторка, панель) доезжают, жест полностью закончен и проверка
    /// «лишнего не случилось» (камере не ушло второго намерения) идёт по установившимся счётчикам. `[estimate]` 400.
    pub step_settle_ms: u64,
    /// Сколько кадров экрана должно быть нарисовано, прежде чем начнутся касания. Касания до первого кадра экрана
    /// теряются (сцена строится на первом кадре), а пока камера не «живая» для экрана, тап отвергается.
    /// `[estimate]` 30 ≈ 0,5 с при 60 Гц.
    pub warmup_ui_frames: u64,
    /// Сколько мс ждать конца прогрева (экран увидел живую камеру и нарисовал нужные кадры). `[estimate]` 5 с.
    pub warmup_timeout_ms: u64,
    /// Доля периода кадра, которую вправе занять GPU-время UI (p95). Та же ловушка грубых ошибок, что у `glass`
    /// (решение 90): 0,5 периода.
    pub ui_gpu_budget_fraction: f64,
    /// Куда нажимает скрипт.
    pub touch: ScreenTouchPlan,
}

impl Default for ScreenScenarioConfig {
    fn default() -> Self {
        Self {
            max_duration_ms: 120_000,
            step_timeout_ms: 8_000,
            step_settle_ms: 400,
            warmup_ui_frames: 30,
            warmup_timeout_ms: 5_000,
            ui_gpu_budget_fraction: 0.5,
            touch: ScreenTouchPlan::default(),
        }
    }
}

/// Неверная секция `camera.scenarios.screen`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ScreenScenarioConfigError {
    #[error("screen.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("screen.ui_gpu_budget_fraction = {value}: нужна доля от 0 (не включая) до 1")]
    BudgetFractionOutOfRange { value: f64 },
    #[error("screen.touch.{gesture}: точка ({x}, {y}) вне экрана (доли должны лежать в 0..=1)")]
    PointOutOfRange { gesture: &'static str, x: f64, y: f64 },
    #[error("screen.touch.focus_point_tolerance = {value}: нужна доля от 0 (не включая) до 0,5 (не включая)")]
    FocusToleranceOutOfRange { value: f64 },
    #[error("screen.touch.{gesture}: длительность жеста должна быть больше нуля")]
    ZeroGestureDuration { gesture: &'static str },
}

impl ScreenScenarioConfig {
    /// Бюджет GPU-времени кадра UI при частоте показа `refresh_hz`, мс: период кадра × доля (решение 36, 90).
    pub fn ui_gpu_budget_ms(&self, refresh_hz: f64) -> f64 {
        1_000.0 / refresh_hz * self.ui_gpu_budget_fraction
    }

    pub fn validate(&self) -> Result<(), ScreenScenarioConfigError> {
        for (field, value) in [
            ("max_duration_ms", self.max_duration_ms),
            ("step_timeout_ms", self.step_timeout_ms),
            ("warmup_ui_frames", self.warmup_ui_frames),
            ("warmup_timeout_ms", self.warmup_timeout_ms),
        ] {
            if value == 0 {
                return Err(ScreenScenarioConfigError::Zero { field });
            }
        }
        // Через сравнение: NaN не проходит и отвергается.
        let fraction = self.ui_gpu_budget_fraction;
        if !(fraction.is_finite() && fraction > 0.0 && fraction <= 1.0) {
            return Err(ScreenScenarioConfigError::BudgetFractionOutOfRange { value: fraction });
        }
        self.touch.validate()
    }
}

#[cfg(test)]
#[path = "screen_scenario_config_tests.rs"]
mod tests;
