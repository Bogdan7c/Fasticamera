//! K12: растр конического кольца — цветовой круг по списку цветовых стопов.
//!
//! У `ui-draw` нет градиентов, поэтому кольцо — картинка RGBA8, которую рендер кэширует по версии
//! ([`ui_draw::picture::UiPicture::version`]): строить её надо один раз на раскладку, а не каждый кадр.
//! Предметной области нет: стопы — просто цвета по кругу.
//!
//! # Договор пикселей
//! * Угол: 0° = вправо (+x), растёт ПРОТИВ часовой стрелки как у математической оси y вверх, то есть на
//!   картинке (строки вниз) 90° — вверх. Так же направлена ось y у `DiscPoint`;
//! * стоп `i` из `N` лежит на угле `i · 360 / N`, между соседними (по кругу, последний → первый) цвет
//!   интерполируется линейно ПО ЗАКОДИРОВАННЫМ sRGB-значениям — стопы приходят плотными, разница с линейным
//!   светом на соседних стопах незаметна;
//! * альфа ПРЯМАЯ (не предумноженная), как у `PictureEncoding::Srgb`: альфа = покрытие пикселя кольцом по
//!   расстоянию до обоих краёв (сглаживание шириной в 1 px). Цвет вне кольца не чёрный, а цвет своего угла
//!   при альфе 0: билинейная фильтрация на краю не подмешивает чёрную кайму.

use thiserror::Error;
use ui_draw::picture::{PictureEncoding, PictureError, PictureSize, UiPicture};

/// Верхняя граница диаметра: страховка от случайного гигантского буфера (4096² × 4 = 64 МБ).
const MAX_DIAMETER_PX: u32 = 4096;

/// Размеры кольца в пикселях картинки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RingGeometry {
    /// Внешний диаметр = сторона квадратной картинки.
    pub outer_diameter_px: u32,
    /// Толщина кольца (внутрь от внешнего края); не больше радиуса.
    pub thickness_px: f32,
}

/// Почему кольцо нельзя построить.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum RingPictureError {
    /// Нет ни одного цветового стопа.
    #[error("у кольца нет цветовых стопов")]
    NoStops,
    /// Диаметр нулевой.
    #[error("внешний диаметр кольца равен нулю")]
    ZeroDiameter,
    /// Диаметр больше допустимого.
    #[error("внешний диаметр кольца {0} px больше допустимых {MAX_DIAMETER_PX}")]
    DiameterTooLarge(u32),
    /// Толщина не конечна или не положительна.
    #[error("толщина кольца должна быть конечной и > 0, получено {0} px")]
    InvalidThickness(f32),
    /// Толщина больше радиуса: кольцо превратилось бы в диск с «отрицательной» дыркой.
    #[error("толщина кольца {thickness_px} px больше радиуса {radius_px} px")]
    ThicknessExceedsRadius { thickness_px: f32, radius_px: f32 },
    /// Картинку не удалось собрать (не должно случаться: размер и буфер согласованы).
    #[error("картинка кольца: {0}")]
    Picture(#[from] PictureError),
}

/// Растр кольца: квадрат `outer_diameter_px`, кольцо вписано, остальное прозрачно.
pub fn ring_picture(stops: &[[u8; 3]], geometry: RingGeometry) -> Result<UiPicture, RingPictureError> {
    if stops.is_empty() {
        return Err(RingPictureError::NoStops);
    }
    if geometry.outer_diameter_px == 0 {
        return Err(RingPictureError::ZeroDiameter);
    }
    if geometry.outer_diameter_px > MAX_DIAMETER_PX {
        return Err(RingPictureError::DiameterTooLarge(geometry.outer_diameter_px));
    }
    if !(geometry.thickness_px.is_finite() && geometry.thickness_px > 0.0) {
        return Err(RingPictureError::InvalidThickness(geometry.thickness_px));
    }
    let outer_radius = geometry.outer_diameter_px as f32 * 0.5;
    if geometry.thickness_px > outer_radius {
        return Err(RingPictureError::ThicknessExceedsRadius {
            thickness_px: geometry.thickness_px,
            radius_px: outer_radius,
        });
    }
    let inner_radius = outer_radius - geometry.thickness_px;

    let side = geometry.outer_diameter_px;
    let mut rgba = Vec::with_capacity(side as usize * side as usize * 4);
    for row in 0..side {
        for column in 0..side {
            // Центр пикселя относительно центра картинки; y переворачивается (строки идут вниз, угол — вверх).
            let dx = column as f32 + 0.5 - outer_radius;
            let dy_up = outer_radius - (row as f32 + 0.5);
            let distance = dx.hypot(dy_up);
            let color = color_at_angle(stops, dy_up.atan2(dx).to_degrees());
            let alpha = ring_coverage(distance, outer_radius, inner_radius);
            rgba.extend_from_slice(&[color[0], color[1], color[2], alpha]);
        }
    }
    Ok(UiPicture::from_rgba8(
        PictureSize::new(side, side),
        PictureEncoding::Srgb,
        rgba,
    )?)
}

/// Покрытие пикселя кольцом, 0…255: расстояние до ближайшего из двух краёв, сглаженное на 1 px.
fn ring_coverage(distance: f32, outer_radius: f32, inner_radius: f32) -> u8 {
    let depth_inside = (outer_radius - distance).min(distance - inner_radius);
    ((depth_inside + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Цвет кольца под углом `angle_degrees` (любым, в том числе отрицательным): линейная смесь двух стопов.
fn color_at_angle(stops: &[[u8; 3]], angle_degrees: f32) -> [u8; 3] {
    let turn_fraction = angle_degrees.rem_euclid(360.0) / 360.0;
    let position = turn_fraction * stops.len() as f32;
    let lower_index = (position.floor() as usize) % stops.len();
    let upper_index = (lower_index + 1) % stops.len();
    let blend = position - position.floor();
    let (lower, upper) = (stops[lower_index], stops[upper_index]);
    let channel = |index: usize| {
        let mixed = f32::from(lower[index]) + (f32::from(upper[index]) - f32::from(lower[index])) * blend;
        mixed.round().clamp(0.0, 255.0) as u8
    };
    [channel(0), channel(1), channel(2)]
}
