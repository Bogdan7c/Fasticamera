//! Вход интерфейса: касания в pt и правило масштаба пикселей.
//!
//! Оболочка отдаёт касания в пикселях surface ([`shell_contract::touch::TouchSample`]). `ui-core`
//! переводит их в pt **один раз на входе** по правилу масштаба решения 82 README плана:
//! масштаб — по ширине, ширина surface в пикселях соответствует эталонной ширине 440 pt
//! (`docs/ios-camera/layout.md`). Лишняя высота нашего экрана остаётся «сверху» — это забота
//! вёрстки (раскладка привязана к низу), а не перевода координат.

use serde::{Deserialize, Serialize};
use shell_contract::touch::{PointerId, TouchPhase, TouchSample};
use thiserror::Error;

use crate::geometry::Point;
use crate::time::MonotonicNanos;

/// Правило масштаба из конфига экрана.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PointScaleRule {
    /// Ширина эталонного экрана в pt. `[review]` 440 pt у iPhone 17 Pro Max (`layout.md` раздел 1).
    pub reference_width_pt: f64,
}

impl Default for PointScaleRule {
    fn default() -> Self {
        Self {
            reference_width_pt: 440.0,
        }
    }
}

/// Ошибка построения масштаба.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum PointScaleError {
    #[error("ширина эталонного экрана должна быть конечной и > 0 pt, получено {0}")]
    InvalidReferenceWidth(f64),
    #[error("ширина surface должна быть > 0 px, получено {0}")]
    InvalidSurfaceWidth(u32),
}

/// Масштаб «пиксели surface ↔ pt» для конкретной surface.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointScale {
    pixels_per_point: f64,
}

impl PointScale {
    /// Масштаб для surface шириной `surface_width_px` по правилу `rule`.
    /// Для нашего экрана 1080 px и 440 pt это 2,4545… px на pt.
    pub fn for_surface_width(rule: PointScaleRule, surface_width_px: u32) -> Result<Self, PointScaleError> {
        if !(rule.reference_width_pt.is_finite() && rule.reference_width_pt > 0.0) {
            return Err(PointScaleError::InvalidReferenceWidth(rule.reference_width_pt));
        }
        if surface_width_px == 0 {
            return Err(PointScaleError::InvalidSurfaceWidth(surface_width_px));
        }
        Ok(Self {
            pixels_per_point: f64::from(surface_width_px) / rule.reference_width_pt,
        })
    }

    pub fn pixels_per_point(self) -> f64 {
        self.pixels_per_point
    }

    /// Пиксели surface → pt.
    pub fn to_points(self, x_px: f32, y_px: f32) -> Point {
        Point::new(
            f64::from(x_px) / self.pixels_per_point,
            f64::from(y_px) / self.pixels_per_point,
        )
    }

    /// Длина в pt → пиксели (для рендера и тестов).
    pub fn to_pixels(self, length_pt: f64) -> f64 {
        length_pt * self.pixels_per_point
    }
}

/// Одно наблюдение одного пальца уже в pt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerSample {
    pub pointer: PointerId,
    pub phase: TouchPhase,
    pub position: Point,
    pub time: MonotonicNanos,
}

impl PointerSample {
    /// Перевод касания оболочки в pt.
    pub fn from_touch(touch: &TouchSample, scale: PointScale) -> Self {
        Self {
            pointer: touch.pointer,
            phase: touch.phase,
            position: scale.to_points(touch.position.x_px, touch.position.y_px),
            time: touch.time,
        }
    }
}

/// Всё, что двигает механику `ui-core`, в одном потоке событий.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum UiInput {
    /// Касание.
    Pointer(PointerSample),
    /// В потоке касаний дыра (`ShellEvent::TouchGap`): все незавершённые жесты сбрасываются
    /// как отменённые системой, иначе распознаватель ждал бы отрыва, которого уже не будет.
    PointerStreamGap,
    /// Новый кадр: время vsync. Нужен, чтобы таймеры жестов (долгий тап) срабатывали, даже
    /// когда палец стоит неподвижно и касаний не шлёт.
    Frame(MonotonicNanos),
}

#[cfg(test)]
mod tests {
    use shell_contract::touch::SurfacePoint;

    use super::*;

    #[test]
    fn our_screen_scale_matches_decision_82() {
        let scale = PointScale::for_surface_width(PointScaleRule::default(), 1080).expect("масштаб");
        assert!((scale.pixels_per_point() - 1080.0 / 440.0).abs() < 1e-12);
        let point = scale.to_points(1080.0, 2412.0);
        assert!((point.x - 440.0).abs() < 1e-9);
        assert!(
            (point.y - 982.666_666).abs() < 1e-3,
            "лишние ≈ 26,7 pt высоты сверху: {point:?}"
        );
        assert!((scale.to_pixels(440.0) - 1080.0).abs() < 1e-9);
    }

    #[test]
    fn invalid_rules_are_rejected() {
        let zero_rule = PointScaleRule {
            reference_width_pt: 0.0,
        };
        assert_eq!(
            PointScale::for_surface_width(zero_rule, 1080),
            Err(PointScaleError::InvalidReferenceWidth(0.0))
        );
        assert_eq!(
            PointScale::for_surface_width(PointScaleRule::default(), 0),
            Err(PointScaleError::InvalidSurfaceWidth(0))
        );
    }

    #[test]
    fn touch_is_converted_to_points_keeping_identity() {
        let scale = PointScale::for_surface_width(PointScaleRule::default(), 1080).expect("масштаб");
        let touch = TouchSample {
            pointer: PointerId::new(3),
            phase: TouchPhase::Moved,
            position: SurfacePoint {
                x_px: 540.0,
                y_px: 245.454_55,
            },
            time: MonotonicNanos::from_nanos(42),
        };
        let sample = PointerSample::from_touch(&touch, scale);
        assert_eq!(sample.pointer, PointerId::new(3));
        assert_eq!(sample.phase, TouchPhase::Moved);
        assert_eq!(sample.time, MonotonicNanos::from_nanos(42));
        assert!((sample.position.x - 220.0).abs() < 1e-9);
        assert!((sample.position.y - 100.0).abs() < 1e-4);
    }
}
