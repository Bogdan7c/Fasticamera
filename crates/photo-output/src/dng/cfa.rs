//! Цветной фильтр кадра для DNG: узор CFA с учётом начала полезной области, уровни чёрного и шум по позициям.
//!
//! Всё, что в паспорте задано «по позициям квадрата 2×2 в порядке строк», относится к левому верхнему
//! углу БУФЕРА сенсора. DNG же описывает узор от левого верхнего угла ЗАПИСАННОГО изображения
//! (у нас — полезная область `valid_area`). Если начало полезной области нечётное, квадрат 2×2
//! сдвигается: перестановка одна и та же для узора CFA и для `BlackLevel`.

use camera_contract::frame_passport::NoiseProfile;
use camera_contract::geometry::PixelRect;
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern};

/// Цвет канала CFA в кодировке DNG (`CFAPattern` / `CFAPlaneColor`, DNG 1.7, раздел 1): 0 — R, 1 — G, 2 — B.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CfaColor {
    Red = 0,
    Green = 1,
    Blue = 2,
}

/// Узор 2×2 в порядке строк.
pub type CfaQuad = [CfaColor; 4];

/// Узор буфера по раскладке паспорта. `None` — мозаика не Bayer (DNG пишется только для Bayer).
pub fn buffer_origin_quad(mosaic: MosaicPattern) -> Option<CfaQuad> {
    use CfaColor::{Blue as B, Green as G, Red as R};
    if mosaic.kind != MosaicKind::Bayer {
        return None;
    }
    match mosaic.layout {
        CfaLayout::Rggb => Some([R, G, G, B]),
        CfaLayout::Grbg => Some([G, R, B, G]),
        CfaLayout::Gbrg => Some([G, B, R, G]),
        CfaLayout::Bggr => Some([B, G, G, R]),
        CfaLayout::Rgb | CfaLayout::Mono | CfaLayout::Nir => None,
    }
}

/// Перестановка позиций 2×2 для сдвига начала изображения на `(left, top)` пикселей буфера.
/// Возвращает индексы: позиция `i` записанного изображения берётся из позиции `result[i]` буфера.
pub fn shifted_positions(valid_area: PixelRect) -> [usize; 4] {
    let column_shift = (valid_area.left % 2) as usize;
    let row_shift = (valid_area.top % 2) as usize;
    let mut positions = [0; 4];
    for row in 0..2 {
        for column in 0..2 {
            positions[row * 2 + column] = ((row + row_shift) % 2) * 2 + (column + column_shift) % 2;
        }
    }
    positions
}

/// Переставляет любые четыре значения «по позициям 2×2» по `shifted_positions`.
pub fn apply_shift<T: Copy>(values: [T; 4], positions: [usize; 4]) -> [T; 4] {
    positions.map(|source| values[source])
}

/// Профиль шума DNG по цветовым плоскостям R, G, B (порядок `CFAPlaneColor` = 0, 1, 2).
///
/// HAL отдаёт по паре (S, O) на каждую из четырёх позиций CFA (порядок как у `BLACK_LEVEL_PATTERN`,
/// позиции буфера), а DNG допускает 1 пару или по паре на цветовую плоскость (раздел «NoiseProfile»).
/// Решение: три пары — R, G, B; для зелёной плоскости берём среднее двух зелёных позиций, потому что
/// в DNG обе зелёные позиции — одна плоскость и одной модели шума достаточно (у реальных сенсоров
/// Gr и Gb отличаются на доли процента). Возвращает `None`, если паспорт неполон или значения
/// недопустимы (S ≤ 0, O < 0, не число).
pub fn noise_profile_per_plane(profile: &NoiseProfile, buffer_quad: CfaQuad) -> Option<[f64; 6]> {
    let mut scale_sum = [0.0f64; 3];
    let mut offset_sum = [0.0f64; 3];
    let mut samples = [0u32; 3];
    for (coefficients, color) in profile.0.iter().zip(buffer_quad) {
        let plane = color as usize;
        scale_sum[plane] += coefficients.scale;
        offset_sum[plane] += coefficients.offset;
        samples[plane] += 1;
    }
    let mut pairs = [0.0f64; 6];
    for plane in 0..3 {
        if samples[plane] == 0 {
            return None;
        }
        let scale = scale_sum[plane] / f64::from(samples[plane]);
        let offset = offset_sum[plane] / f64::from(samples[plane]);
        // Спецификация: S > 0, O ≥ 0 (и конечные числа).
        if !(scale.is_finite() && offset.is_finite() && scale > 0.0 && offset >= 0.0) {
            return None;
        }
        pairs[plane * 2] = scale;
        pairs[plane * 2 + 1] = offset;
    }
    Some(pairs)
}
