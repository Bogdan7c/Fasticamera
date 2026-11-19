//! Мягкая тень под фигурой: общая формула для `ShapeItem` и стекла (шейдеры `ui_shape`, `ui_glass`).

use crate::geometry::PxPoint;
use crate::shape::coverage;

/// Точка, в которой считается расстояние до формы для тени: тень — та же форма, сдвинутая вниз
/// на `offset_px`, поэтому пиксель экрана смотрит на форму в точке выше на это смещение.
pub fn shadow_probe_point(screen_point: PxPoint, offset_px: f32) -> PxPoint {
    PxPoint::new(screen_point.x, screen_point.y - offset_px)
}

/// Непрозрачность тени 0..1 (до умножения на её альфу) по расстоянию `shifted_distance_px` до
/// сдвинутой формы (внутри < 0).
///
/// При `radius_px > 0` край мягкий: smoothstep шириной `2·radius` вокруг границы (±radius), на самой
/// границе 0,5. При `radius_px ≤ 0` — резкая копия формы с обычным сглаживанием края шириной
/// `aa_width_px`.
pub fn shadow_falloff(shifted_distance_px: f32, radius_px: f32, aa_width_px: f32) -> f32 {
    if radius_px > 0.0 {
        let t = (0.5 - shifted_distance_px / (2.0 * radius_px)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    } else {
        coverage(shifted_distance_px, aa_width_px)
    }
}
