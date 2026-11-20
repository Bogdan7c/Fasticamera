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

/// Плавная шкала с магнитами; 24 pt на единицу, радиус магнита по умолчанию токенов (14 pt).
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

// ---------- сцена `ui-core` и поток касаний в пикселях (сквозные тесты шва) ----------

use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_core::geometry::{Insets, Rect};
use ui_core::layout::{AnchorRule, AxisPlacement, Placement, ReferenceArea};
use ui_core::scene::{SceneSettings, SurfaceGeometry, UiScene};

/// Ширина surface, пиксели: 1080 px = 440 pt (решение 82).
pub const SURFACE_WIDTH_PX: u32 = 1080;
/// Высота surface, пиксели.
pub const SURFACE_HEIGHT_PX: u32 = 2412;
/// Пикселей на pt, посчитано независимо от `PointScale` (эталон перевода).
pub const PIXELS_PER_POINT: f64 = 1080.0 / 440.0;

/// Сцена с пустым деревом на нашем экране. `I` — намерение экрана.
pub fn empty_scene<I: Clone>() -> UiScene<I> {
    UiScene::new(
        SurfaceGeometry {
            width_px: SURFACE_WIDTH_PX,
            height_px: SURFACE_HEIGHT_PX,
            safe_area: Insets::ZERO,
        },
        SceneSettings::default(),
    )
    .unwrap()
}

/// Привязка к левому верхнему углу родителя: рамка `rect` (pt) в координатах родителя.
pub fn anchored_rect(rect: Rect) -> Placement {
    Placement::Anchored(AnchorRule {
        area: ReferenceArea::Frame,
        horizontal: AxisPlacement::Start {
            inset: rect.origin.x,
            length: rect.size.width,
        },
        vertical: AxisPlacement::Start {
            inset: rect.origin.y,
            length: rect.size.height,
        },
    })
}

/// Ребёнок корня с рамкой `rect` (pt); вёрстка пересчитана (иначе хит-тест идёт по старым рамкам).
pub fn add_rect<I: Clone>(scene: &mut UiScene<I>, label: &'static str, rect: Rect) -> ui_core::id::NodeId {
    let root = scene.tree().root();
    let node = scene.insert_child(root, label).unwrap();
    scene.set_placement(node, anchored_rect(rect)).unwrap();
    scene.layout().unwrap();
    node
}

/// Пиксельная координата, как её отправит оболочка (`f32`).
pub fn to_pixels(point: f64) -> f32 {
    (point * PIXELS_PER_POINT) as f32
}

/// Что реально увидела арена: пиксель, переведённый обратно в pt независимой формулой.
pub fn seen_in_points(pixels: f32) -> f64 {
    f64::from(pixels) / PIXELS_PER_POINT
}

/// Касание оболочки в ПИКСЕЛЯХ по координатам, заданным в pt.
pub fn touch(pointer: i32, phase: TouchPhase, x_pt: f64, y_pt: f64, millis: i64) -> TouchSample {
    TouchSample {
        pointer: PointerId::new(pointer),
        phase,
        position: SurfacePoint {
            x_px: to_pixels(x_pt),
            y_px: to_pixels(y_pt),
        },
        time: ms(millis),
    }
}
