//! Где на экране лежит изображение видоискателя (чистая математика, без GPU).
//!
//! Изображение видоискателя уже прямое (ISP повернул кадр по ориентации сенсора). Компоновщик
//! принимает прямоугольник в ЛОГИЧЕСКИХ пикселях цели — как видит пользователь, — и сам учитывает
//! поворот дисплея. Здесь только: логический размер цели и прямоугольник изображения — по правилу
//! конфига («по ширине цели») или в прямоугольник, который прислал экран камеры (решение 93).

use gpu_vulkan::compositor::LogicalRect;

use crate::config::ViewfinderPlacement;
use crate::viewfinder::{EffectivePlacement, NormalizedScreenRect};

/// Размер плоскости, пиксели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaneSize {
    /// Ширина.
    pub width: u32,
    /// Высота.
    pub height: u32,
}

/// Логический (как видит пользователь) размер цели: при повороте дисплея на 90° или 270°
/// физическая картинка swapchain лежит боком — стороны меняются местами.
pub fn logical_target_size(physical: PlaneSize, display_quarter_turns: u32) -> PlaneSize {
    if display_quarter_turns % 2 == 1 {
        PlaneSize {
            width: physical.height,
            height: physical.width,
        }
    } else {
        physical
    }
}

/// Прямоугольник изображения `image` на логической цели `target`: ширина — во всю цель, высота —
/// по соотношению сторон изображения (округление к ближайшему, не меньше 1 пикселя).
///
/// Если изображение по ширине выше цели, оно прижимается к верху и снизу обрезается: координаты
/// компоновщика не бывают отрицательными, а «вписать по высоте» — уже решение раскладки (сессия 12).
/// `None` — нулевой размер изображения или цели (рисовать нечего).
pub fn viewfinder_destination(
    image: PlaneSize,
    target: PlaneSize,
    placement: ViewfinderPlacement,
) -> Option<LogicalRect> {
    if image.width == 0 || image.height == 0 || target.width == 0 || target.height == 0 {
        return None;
    }
    // u64: произведение двух u32 не переполняется.
    let scaled =
        (u64::from(image.height) * u64::from(target.width) + u64::from(image.width) / 2) / u64::from(image.width);
    let height = u32::try_from(scaled).unwrap_or(u32::MAX).max(1);
    let y = match placement {
        ViewfinderPlacement::FitWidthCentered => target.height.saturating_sub(height) / 2,
        ViewfinderPlacement::FitWidthTop => 0,
    };
    Some(LogicalRect {
        x: 0,
        y,
        width: target.width,
        height,
    })
}

/// Прямоугольник изображения `image` на логической цели `target` для действующего места: правило конфига
/// или прямоугольник из команды. Единственная точка входа раскладки — через неё идут и компоновщик, и
/// фон стекла. `None` — рисовать нечего (нулевой размер изображения, цели или прямоугольника).
pub fn resolve_destination(image: PlaneSize, target: PlaneSize, placement: EffectivePlacement) -> Option<LogicalRect> {
    match placement {
        EffectivePlacement::Configured(configured) => viewfinder_destination(image, target, configured),
        EffectivePlacement::ScreenRect(rect) => fit_into_screen_rect(image, target, rect),
    }
}

/// Вписывает изображение в прямоугольник экрана с сохранением пропорций, по центру прямоугольника:
/// сначала по ширине; если изображение относительно выше прямоугольника — по высоте. Ничего не
/// растягивается: лишнее место прямоугольника остаётся чёрным. Края прямоугольника округляются к
/// ближайшему пикселю и не выходят за цель.
fn fit_into_screen_rect(image: PlaneSize, target: PlaneSize, rect: NormalizedScreenRect) -> Option<LogicalRect> {
    if image.width == 0 || image.height == 0 || target.width == 0 || target.height == 0 {
        return None;
    }
    let left = edge_px(rect.left(), target.width);
    let top = edge_px(rect.top(), target.height);
    let right = edge_px(rect.left() + rect.width(), target.width);
    let bottom = edge_px(rect.top() + rect.height(), target.height);
    let (slot_width, slot_height) = (right.saturating_sub(left), bottom.saturating_sub(top));
    if slot_width == 0 || slot_height == 0 {
        return None;
    }
    // Сравнение пропорций без деления: image.w / image.h против slot.w / slot.h (u64 не переполняется).
    let image_is_relatively_wider =
        u64::from(image.width) * u64::from(slot_height) >= u64::from(image.height) * u64::from(slot_width);
    let (width, height) = if image_is_relatively_wider {
        (
            slot_width,
            scaled(image.height, slot_width, image.width).min(slot_height),
        )
    } else {
        (
            scaled(image.width, slot_height, image.height).min(slot_width),
            slot_height,
        )
    };
    Some(LogicalRect {
        x: left + (slot_width - width) / 2,
        y: top + (slot_height - height) / 2,
        width,
        height,
    })
}

/// Доля цели в пиксели (округление к ближайшему, не дальше размера цели).
fn edge_px(fraction: f32, extent: u32) -> u32 {
    // f32 -> u32 насыщает (NaN даёт 0); конструктор прямоугольника уже гарантирует конечные доли.
    ((f64::from(fraction) * f64::from(extent)).round() as u32).min(extent)
}

/// `value * numerator / denominator` с округлением к ближайшему, не меньше 1 пикселя.
fn scaled(value: u32, numerator: u32, denominator: u32) -> u32 {
    let product = u64::from(value) * u64::from(numerator);
    let rounded = (product + u64::from(denominator) / 2) / u64::from(denominator);
    u32::try_from(rounded).unwrap_or(u32::MAX).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn size(width: u32, height: u32) -> PlaneSize {
        PlaneSize { width, height }
    }

    #[test]
    fn portrait_phone_shows_the_4_by_3_viewfinder_full_width_and_centered() {
        // OnePlus 11 в портрете: логическая цель 1440×3216, видоискатель 4:3 стоя (1536×2048).
        let rect = viewfinder_destination(
            size(1536, 2048),
            size(1440, 3216),
            ViewfinderPlacement::FitWidthCentered,
        )
        .expect("размеры ненулевые");
        assert_eq!(
            rect,
            LogicalRect {
                x: 0,
                y: (3216 - 1920) / 2,
                width: 1440,
                height: 1920
            }
        );
        let top = viewfinder_destination(size(1536, 2048), size(1440, 3216), ViewfinderPlacement::FitWidthTop)
            .expect("размеры ненулевые");
        assert_eq!((top.y, top.height), (0, 1920));
    }

    #[test]
    fn rotated_display_swaps_the_logical_target_sides() {
        assert_eq!(logical_target_size(size(3216, 1440), 1), size(1440, 3216));
        assert_eq!(logical_target_size(size(3216, 1440), 3), size(1440, 3216));
        assert_eq!(logical_target_size(size(1440, 3216), 2), size(1440, 3216));
        assert_eq!(logical_target_size(size(1440, 3216), 0), size(1440, 3216));
    }

    #[test]
    fn image_taller_than_the_target_is_top_aligned_and_clipped() {
        let rect = viewfinder_destination(size(100, 400), size(200, 300), ViewfinderPlacement::FitWidthCentered)
            .expect("размеры ненулевые");
        assert_eq!(
            rect,
            LogicalRect {
                x: 0,
                y: 0,
                width: 200,
                height: 800
            }
        );
    }

    #[test]
    fn rounding_is_to_nearest_and_degenerate_sizes_draw_nothing() {
        // 3×2 на ширину 4: высота 2,67 → 3.
        let rect = viewfinder_destination(size(3, 2), size(4, 10), ViewfinderPlacement::FitWidthCentered)
            .expect("размеры ненулевые");
        assert_eq!((rect.height, rect.y), (3, 3));
        // Очень широкое изображение: высота не меньше пикселя.
        let thin = viewfinder_destination(size(10_000, 1), size(10, 10), ViewfinderPlacement::FitWidthTop)
            .expect("размеры ненулевые");
        assert_eq!(thin.height, 1);
        assert_eq!(
            viewfinder_destination(size(0, 4), size(4, 4), ViewfinderPlacement::FitWidthTop),
            None
        );
        assert_eq!(
            viewfinder_destination(size(4, 4), size(4, 0), ViewfinderPlacement::FitWidthTop),
            None
        );
    }

    fn screen_rect(left: f32, top: f32, width: f32, height: f32) -> EffectivePlacement {
        EffectivePlacement::ScreenRect(NormalizedScreenRect::new(left, top, width, height).expect("прямоугольник"))
    }

    #[test]
    fn matching_aspect_fills_the_screen_rect_exactly() {
        // Цель 1000×2000, прямоугольник 0.5×0.25 цели = 500×500, изображение 1:1: заполняет без полос.
        let rect = resolve_destination(size(64, 64), size(1000, 2000), screen_rect(0.25, 0.5, 0.5, 0.25));
        assert_eq!(
            rect,
            Some(LogicalRect {
                x: 250,
                y: 1000,
                width: 500,
                height: 500
            })
        );
    }

    #[test]
    fn wider_image_is_fitted_by_width_and_centered_vertically_in_the_rect() {
        // Прямоугольник 400×400, изображение 2:1 → 400×200 по центру.
        let rect = resolve_destination(size(200, 100), size(400, 800), screen_rect(0.0, 0.5, 1.0, 0.5));
        assert_eq!(
            rect,
            Some(LogicalRect {
                x: 0,
                y: 400 + 100,
                width: 400,
                height: 200
            })
        );
    }

    #[test]
    fn taller_image_is_fitted_by_height_and_centered_horizontally_in_the_rect() {
        // Прямоугольник 400×400, изображение 1:2 → 200×400 по центру (не растягивается и не обрезается).
        let rect = resolve_destination(size(100, 200), size(400, 800), screen_rect(0.0, 0.0, 1.0, 0.5));
        assert_eq!(
            rect,
            Some(LogicalRect {
                x: 100,
                y: 0,
                width: 200,
                height: 400
            })
        );
    }

    #[test]
    fn screen_rect_follows_the_target_size_so_the_fraction_survives_recreation() {
        let placement = screen_rect(0.0, 0.5, 1.0, 0.5);
        let small = resolve_destination(size(1, 1), size(200, 400), placement).expect("не пусто");
        let large = resolve_destination(size(1, 1), size(300, 600), placement).expect("не пусто");
        assert_eq!((small.x, small.y, small.width, small.height), (0, 200, 200, 200));
        assert_eq!((large.x, large.y, large.width, large.height), (0, 300, 300, 300));
    }

    #[test]
    fn degenerate_screen_rect_draws_nothing_and_never_leaves_the_target() {
        // Доля меньше полпикселя: после округления пусто.
        assert_eq!(
            resolve_destination(size(4, 4), size(100, 100), screen_rect(0.0, 0.0, 0.001, 0.5)),
            None
        );
        assert_eq!(
            resolve_destination(size(0, 4), size(100, 100), screen_rect(0.0, 0.0, 1.0, 1.0)),
            None
        );
        let full = resolve_destination(size(4, 4), size(101, 99), screen_rect(0.0, 0.0, 1.0, 1.0)).expect("не пусто");
        assert!(full.x + full.width <= 101 && full.y + full.height <= 99, "{full:?}");
    }

    #[test]
    fn configured_placement_keeps_the_pre_session_12_behaviour() {
        assert_eq!(
            resolve_destination(
                size(1536, 2048),
                size(1440, 3216),
                EffectivePlacement::Configured(ViewfinderPlacement::FitWidthTop)
            ),
            viewfinder_destination(size(1536, 2048), size(1440, 3216), ViewfinderPlacement::FitWidthTop)
        );
    }
}
