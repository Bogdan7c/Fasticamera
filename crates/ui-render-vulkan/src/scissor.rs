//! Перевод прямоугольника «где группа может дать вклад» (логические px) в scissor физической цели.
//!
//! Простыми словами: стекло рисуется большим квадом (его вершины определяют координаты, по которым шейдер
//! читает фон, и менять их нельзя: картинка сдвинется на доли бита). Чтобы не платить за пиксели, где
//! группа заведомо даёт ноль, на вызов рисования ставится scissor по узкому прямоугольнику: аппаратура
//! отбрасывает фрагменты вне него до шейдера.

use ash::vk;
use ui_draw::geometry::PxRect;

use crate::plan::BatchClip;

/// Какую часть квада стеклянной группы рисовать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GlassDrawArea {
    /// Только область возможного вклада группы (`GlassGroupItem::draw_rect_px`): боевой режим.
    Tight,
    /// Весь квад, как до сессии 16a (без scissor): нужен только тесту «старый квад против нового».
    #[cfg(feature = "test-support")]
    WholeQuad,
}

/// Scissor физической цели для логического прямоугольника при повороте цели на `quarter_turns` четвертей.
/// Границы округляются НАРУЖУ (пиксель, центр которого может оказаться внутри, остаётся), затем обрезаются
/// целью. `None` — после обрезки не осталось ни одного пикселя (рисовать нечего).
///
/// Соответствие поворотов то же, что у `logical_to_physical` в `ui_frame.glsl`.
pub(crate) fn logical_rect_to_scissor(rect: &PxRect, quarter_turns: u32, target: vk::Extent2D) -> Option<vk::Rect2D> {
    let (width, height) = (target.width as f32, target.height as f32);
    let to_physical = |x: f32, y: f32| match quarter_turns {
        1 => (y, height - x),
        2 => (width - x, height - y),
        3 => (width - y, x),
        _ => (x, y),
    };
    let (ax, ay) = to_physical(rect.origin.x, rect.origin.y);
    let (bx, by) = to_physical(rect.max_x(), rect.max_y());
    // `f32::min/max` молча отбрасывают NaN, поэтому нечисловой прямоугольник отсекаем явно.
    if ![ax, ay, bx, by].iter().all(|value| value.is_finite()) {
        return None;
    }
    let left = ax.min(bx).floor().max(0.0);
    let top = ay.min(by).floor().max(0.0);
    let right = ax.max(bx).ceil().min(width);
    let bottom = ay.max(by).ceil().min(height);
    // NaN и пустой остаток дают `false` в сравнении: рисовать нечего.
    if !(right > left && bottom > top) {
        return None;
    }
    Some(vk::Rect2D {
        offset: vk::Offset2D {
            x: left as i32,
            y: top as i32,
        },
        extent: vk::Extent2D {
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        },
    })
}

/// Что делать с вызовом рисования пакета с точки зрения обрезки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchScissor {
    /// Обрезки нет: рисуем без scissor.
    WholeTarget,
    /// Рисуем со scissor.
    Rect(vk::Rect2D),
    /// После обрезки целью не осталось ни одного пикселя: вызов рисования не нужен.
    NothingVisible,
}

impl BatchScissor {
    /// Решение для обрезки пакета при повороте цели на `quarter_turns` четвертей. Прямоугольник обрезки целый
    /// ещё до поворота (правило центра пикселя), а поворот на четверти переводит целые границы в целые,
    /// поэтому округление наружу в [`logical_rect_to_scissor`] ничего не меняет: scissor совпадает с
    /// обрезкой пиксель в пиксель.
    pub(crate) fn for_clip(clip: &BatchClip, quarter_turns: u32, target: vk::Extent2D) -> BatchScissor {
        match clip {
            BatchClip::WholeTarget => BatchScissor::WholeTarget,
            BatchClip::PixelRect(pixels) => match logical_rect_to_scissor(pixels, quarter_turns, target) {
                Some(rect) => BatchScissor::Rect(rect),
                None => BatchScissor::NothingVisible,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::panic, reason = "тесты")]

    use super::*;
    use ui_draw::geometry::{PxPoint, PxSize};

    const TARGET: vk::Extent2D = vk::Extent2D { width: 100, height: 60 };

    fn rect(x: f32, y: f32, width: f32, height: f32) -> PxRect {
        PxRect::new(PxPoint::new(x, y), PxSize::new(width, height))
    }

    fn scissor(rect: &PxRect, turns: u32) -> Option<(i32, i32, u32, u32)> {
        logical_rect_to_scissor(rect, turns, TARGET).map(|s| (s.offset.x, s.offset.y, s.extent.width, s.extent.height))
    }

    #[test]
    fn identity_rounds_outwards() {
        assert_eq!(scissor(&rect(10.2, 5.7, 20.0, 10.0), 0), Some((10, 5, 21, 11)));
    }

    #[test]
    fn rect_is_clamped_to_target() {
        assert_eq!(scissor(&rect(-30.0, -5.0, 60.0, 20.0), 0), Some((0, 0, 30, 15)));
        assert_eq!(scissor(&rect(90.0, 50.0, 40.0, 40.0), 0), Some((90, 50, 10, 10)));
    }

    #[test]
    fn rect_outside_or_nan_draws_nothing() {
        assert_eq!(scissor(&rect(200.0, 10.0, 5.0, 5.0), 0), None);
        assert_eq!(scissor(&rect(f32::NAN, 10.0, 5.0, 5.0), 0), None);
    }

    #[test]
    fn rotated_targets_map_the_corner_like_the_vertex_shader() {
        // Цель 100 × 60 физически; логический экран при поворотах на 90° — 60 × 100.
        let logical = rect(10.0, 20.0, 5.0, 8.0);
        // turns 1: physical = (y, H - x): x 20..28, y 60-15..60-10 = 45..50.
        assert_eq!(scissor(&logical, 1), Some((20, 45, 8, 5)));
        // turns 2: physical = (W - x, H - y): x 85..90, y 32..40.
        assert_eq!(scissor(&logical, 2), Some((85, 32, 5, 8)));
        // turns 3: physical = (W - y, x): x 72..80, y 10..15.
        assert_eq!(scissor(&logical, 3), Some((72, 10, 8, 5)));
    }

    #[test]
    fn integer_clip_maps_to_exactly_the_same_pixels_for_every_rotation() {
        let clip = BatchClip::PixelRect(rect(10.0, 20.0, 5.0, 8.0));
        let rect_of = |turns| match BatchScissor::for_clip(&clip, turns, TARGET) {
            BatchScissor::Rect(r) => (r.offset.x, r.offset.y, r.extent.width, r.extent.height),
            other => panic!("ожидали Rect, получили {other:?}"),
        };
        // Площадь и целые границы сохраняются: ровно 5 × 8 пикселей (или 8 × 5 при повороте на 90°).
        assert_eq!(rect_of(0), (10, 20, 5, 8));
        assert_eq!(rect_of(1), (20, 45, 8, 5));
        assert_eq!(rect_of(2), (85, 32, 5, 8));
        assert_eq!(rect_of(3), (72, 10, 8, 5));
    }

    #[test]
    fn whole_target_and_vanished_clip_are_distinguished() {
        assert_eq!(
            BatchScissor::for_clip(&BatchClip::WholeTarget, 0, TARGET),
            BatchScissor::WholeTarget
        );
        let outside = BatchClip::PixelRect(rect(500.0, 500.0, 5.0, 5.0));
        assert_eq!(
            BatchScissor::for_clip(&outside, 0, TARGET),
            BatchScissor::NothingVisible
        );
    }
}
