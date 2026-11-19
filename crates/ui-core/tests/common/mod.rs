//! Общие помощники сквозных тестов сцены и аниматора.
#![allow(dead_code)]

use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_core::geometry::{Insets, Rect};
use ui_core::id::NodeId;
use ui_core::layout::{AnchorRule, AxisPlacement, Placement, ReferenceArea};
use ui_core::scene::{SceneSettings, SurfaceGeometry, UiScene};
use ui_core::time::MonotonicNanos;

/// Ширина surface нашего экрана, пиксели (440 pt по правилу решения 82).
pub const SURFACE_WIDTH_PX: u32 = 1080;
/// Высота surface, пиксели.
pub const SURFACE_HEIGHT_PX: u32 = 2412;
/// Пикселей на pt при ширине 1080 px, считается независимо от `PointScale`.
pub const PIXELS_PER_POINT: f64 = 1080.0 / 440.0;

/// Момент в миллисекундах от условного нуля шкалы.
pub fn at_ms(millis: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(1_000_000_000 + millis * 1_000_000)
}

pub fn surface(width_px: u32) -> SurfaceGeometry {
    surface_sized(width_px, SURFACE_HEIGHT_PX)
}

pub fn surface_sized(width_px: u32, height_px: u32) -> SurfaceGeometry {
    SurfaceGeometry {
        width_px,
        height_px,
        safe_area: Insets::ZERO,
    }
}

/// Касание оболочки в ПИКСЕЛЯХ.
pub fn touch_px(pointer: i32, phase: TouchPhase, x_px: f32, y_px: f32, millis: i64) -> TouchSample {
    TouchSample {
        pointer: PointerId::new(pointer),
        phase,
        position: SurfacePoint { x_px, y_px },
        time: at_ms(millis),
    }
}

/// Касание по координатам в pt, переведённым в пиксели независимо от `PointScale`.
pub fn touch_at_pt(pointer: i32, phase: TouchPhase, x_pt: f64, y_pt: f64, millis: i64) -> TouchSample {
    touch_px(
        pointer,
        phase,
        (x_pt * PIXELS_PER_POINT) as f32,
        (y_pt * PIXELS_PER_POINT) as f32,
        millis,
    )
}

/// Привязка к левому верхнему углу родителя: рамка `rect` в координатах родителя.
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

/// Сцена с намерениями-строками и пустым деревом.
pub fn empty_scene() -> UiScene<&'static str> {
    UiScene::new(surface(SURFACE_WIDTH_PX), SceneSettings::default()).unwrap()
}

/// Добавляет в сцену ребёнка с рамкой `rect` (pt) и пересчитывает вёрстку.
pub fn add_rect(scene: &mut UiScene<&'static str>, parent: NodeId, label: &'static str, rect: Rect) -> NodeId {
    let node = scene.insert_child(parent, label).unwrap();
    scene.set_placement(node, anchored_rect(rect)).unwrap();
    scene.layout().unwrap();
    node
}
