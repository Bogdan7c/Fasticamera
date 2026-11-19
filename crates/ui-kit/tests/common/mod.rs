//! Общие помощники тестов: часы, жесты, окружение.
#![allow(dead_code)]

use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_core::time::{MonotonicNanos, MotionPreference};
use ui_kit::environment::ElementEnvironment;
use ui_kit::tokens::UiKitTokens;

/// Момент в миллисекундах от условного нуля (с запасом, чтобы разности были положительны).
pub fn ms(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(1_000_000_000 + millis * 1_000_000)
}

/// Смещение пальца по горизонтали: `from_touch_down` и `from_recognition` заданы отдельно.
pub fn pan(from_touch_down_dx: f64, from_recognition_dx: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(from_touch_down_dx, 0.0),
        from_recognition: Vector::new(from_recognition_dx, 0.0),
    }
}

/// Горизонтальное смещение без гистерезиса (оба отсчёта совпадают).
pub fn pan_same(dx: f64) -> PanTranslation {
    pan(dx, dx)
}

pub fn horizontal(dx: f64) -> Vector {
    Vector::new(dx, 0.0)
}

pub const VIEWPORT_POINTS: f64 = 300.0;

pub fn env<'a>(tokens: &'a UiKitTokens, motion: MotionPreference) -> ElementEnvironment<'a> {
    ElementEnvironment {
        tokens,
        motion,
        viewport_extent_points: VIEWPORT_POINTS,
    }
}

/// Независимая формула «резинки» из `motion.md` раздел 2 (c = 0,55).
pub fn rubber_formula(overshoot: f64, dimension: f64) -> f64 {
    (1.0 - 1.0 / (overshoot * 0.55 / dimension + 1.0)) * dimension
}

use ui_kit::drive::{DragOrigin, GrowthDirection};
use ui_kit::scale::{ScaleSpec, SnapPolicy, SnapScale, TickVisibility};
use ui_kit::tokens::ScaleDensity;
use ui_kit::value::{LinearLimits, LogLimits, Logarithmic, LogarithmicSpec, QuantizedLinear, ReferenceValue};

/// Модель «EV ±2 по 1/3» — пример линейной шкалы с нулём.
pub fn exposure_model() -> QuantizedLinear {
    QuantizedLinear::new(1.0 / 3.0, ReferenceValue::At(0.0), LinearLimits { min: -2.0, max: 2.0 }).unwrap()
}

/// Шкала шагов: палец вправо — значение растёт, 12 pt на шаг, границы ±72 pt.
pub fn exposure_spec(direction: GrowthDirection, drag_origin: DragOrigin) -> ScaleSpec {
    ScaleSpec {
        direction,
        snap: SnapPolicy::EveryStep,
        density: ScaleDensity::Dense,
        ticks: TickVisibility::Visible,
        drag_origin,
    }
}

pub fn exposure_scale(value: f64, spec: ScaleSpec, tokens: &ui_kit::tokens::UiKitTokens) -> SnapScale<QuantizedLinear> {
    SnapScale::new(
        exposure_model(),
        value,
        spec,
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

/// Плавная логарифмическая модель с якорями (пример — кратность): 12 единиц на октаву.
pub fn log_model(anchors: Vec<f64>, max: f64) -> Logarithmic {
    Logarithmic::new(
        LogarithmicSpec {
            units_per_octave: 12.0,
            tick_spacing_units: 3.0,
            anchors,
        },
        LogLimits { min: 0.5, max },
    )
    .unwrap()
}

/// Плавная шкала с магнитами; 24 pt на единицу, радиус магнита 14 pt (умолчания токенов).
pub fn magnet_scale(
    value: f64,
    anchors: Vec<f64>,
    max: f64,
    tokens: &ui_kit::tokens::UiKitTokens,
) -> SnapScale<Logarithmic> {
    let spec = ScaleSpec {
        direction: GrowthDirection::FingerRight,
        snap: SnapPolicy::ContinuousWithMagnets,
        density: ScaleDensity::Regular,
        ticks: TickVisibility::Visible,
        drag_origin: DragOrigin::FromRecognition,
    };
    SnapScale::new(
        log_model(anchors, max),
        value,
        spec,
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}
