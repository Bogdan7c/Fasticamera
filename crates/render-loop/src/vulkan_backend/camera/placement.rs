//! Где на экране лежит изображение видоискателя (чистая математика, без GPU).
//!
//! Изображение видоискателя уже прямое (ISP повернул кадр по ориентации сенсора). Компоновщик
//! принимает прямоугольник в ЛОГИЧЕСКИХ пикселях цели — как видит пользователь, — и сам учитывает
//! поворот дисплея. Здесь только: логический размер цели и прямоугольник «по ширине цели».
//! Раскладку экрана целиком будет решать сессия 12.

use gpu_vulkan::compositor::LogicalRect;

use crate::config::ViewfinderPlacement;

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
}
