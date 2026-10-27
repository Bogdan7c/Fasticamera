//! Параметры кадра → байты uniform-блока `ViewfinderParameters` (std140) — чистая логика без GPU.
//!
//! # Раскладка (зеркало `shaders/viewfinder_isp.glsl`, менять только вместе)
//! Только 32-битные типы: у Adreno 740 нет `uniformAndStorageBuffer16BitAccess`. Каждая строка —
//! `vec4`/`uvec4` (16 байт), поэтому std140 не вставляет выравнивающих дыр.
//!
//! | Строка | Смещение | Тип | Содержимое |
//! |---|---|---|---|
//! | 0 | 0 | vec4 | чёрный уровень по позициям ячейки 2×2 (порядок строк, как `BLACK_LEVEL_PATTERN`) |
//! | 1 | 16 | vec4 | `white − black` по позициям, посчитано здесь в f32 той же операцией, что в эталоне |
//! | 2 | 32 | vec4 | усиление баланса белого для цвета каждой позиции |
//! | 3 | 48 | vec4 | белый уровень, уровень обрезки после баланса, порог пересвета AWB, 0 |
//! | 4–6 | 64 | vec4×3 | строки матрицы камера (после баланса) → XYZ D50, w = 0 |
//! | 7–9 | 112 | vec4×3 | строки матрицы XYZ D50 → линейный свет экрана, w = 0 |
//! | 10 | 160 | vec4 | порог сжатия охвата по каналам, w — степень `p` |
//! | 11 | 176 | vec4 | масштаб `s` по каналам, w — `1/p` (посчитано здесь, как в эталоне) |
//! | 12 | 192 | vec4 | тон: контраст `c`, `s^c`, `1/F(x_white)`, доля сохранения оттенка |
//! | 13 | 208 | uvec4 | кадрирование в пикселях буфера: left, top, width, height |
//! | 14 | 224 | uvec4 | ширина и высота выхода, поворот сенсора (четверти по часовой), шаг строки RAW10 (байт) |
//! | 15 | 240 | uvec4 | цвет позиции ячейки: 0 — R, 1 — G, 2 — B |
//! | 16 | 256 | uvec4 | столбцы и строки сетки AWB, 0, 0 |
//!
//! Все числа — ровно те f32, что использует эталон (`ToneCurve::parameters`,
//! `GamutCompression::parameters`, `PixelMatrix::rows`): шейдер ничего не пересчитывает из настроек.

use camera_contract::geometry::PixelRect;
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_model::cfa::{CfaCell, RgbChannel};
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::lens_shading::LensShadingStep;
use isp_model::params::{CURRENT_ISP_PARAMS_VERSION, IspParams};
use isp_model::statistics::AwbStatisticsSettings;
use isp_model::white_balance::SENSOR_CLIP_LEVEL;

use super::error::ViewfinderError;
use super::geometry::CheckedGeometry;

/// Размер uniform-блока: 17 строк по 16 байт.
pub const VIEWFINDER_UNIFORM_BYTES: usize = 17 * 16;

/// Позиций в ячейке CFA 2×2.
const CELL_POSITIONS: usize = 4;

/// Гамма, под которую обязаны быть посчитаны параметры для этого экрана.
pub fn gamut_for_display(color_space: DisplayColorSpace) -> DisplayGamut {
    match color_space {
        DisplayColorSpace::Srgb => DisplayGamut::Srgb,
        DisplayColorSpace::DisplayP3 => DisplayGamut::DisplayP3,
    }
}

/// Проверяет, что параметры собраны для этого прохода: та же версия модели, демозаик Preview,
/// без коррекции виньетирования картой (на OnePlus 11 её делает вендор) и гамма цели показа.
pub(crate) fn check_params(params: &IspParams, display: DisplayColorSpace) -> Result<(), ViewfinderError> {
    if params.version != CURRENT_ISP_PARAMS_VERSION {
        return Err(ViewfinderError::ParamsVersionMismatch {
            expected: CURRENT_ISP_PARAMS_VERSION,
            actual: params.version,
        });
    }
    if params.demosaic != DemosaicMethod::Preview {
        return Err(ViewfinderError::UnsupportedParams {
            what: "видоискатель делает только демозаик Preview (2×2)",
        });
    }
    if matches!(params.lens_shading, LensShadingStep::Apply(_)) {
        return Err(ViewfinderError::UnsupportedParams {
            what: "коррекция виньетирования картой в видоискателе не реализована (вендор OnePlus 11 применяет её сам)",
        });
    }
    let expected = gamut_for_display(display);
    if params.color.gamut() != expected {
        return Err(ViewfinderError::GamutMismatch {
            params: params.color.gamut(),
            display,
        });
    }
    Ok(())
}

/// Собирает байты uniform-блока (порядок и смысл — таблица модуля).
pub(crate) fn pack_uniforms(
    params: &IspParams,
    geometry: &CheckedGeometry,
    awb: &AwbStatisticsSettings,
) -> [u8; VIEWFINDER_UNIFORM_BYTES] {
    let mut writer = Std140Writer::default();
    let black = params.levels.black_pattern();
    let white = params.levels.white();
    writer.vec4(black);
    // Та же операция f32, что в `isp_model::levels::normalize_sample`: бит в бит.
    writer.vec4(black.map(|cell_black| white - cell_black));
    let gains = params.white_balance.as_array();
    writer.vec4(position_channels(params).map(|channel| gains[channel.index()]));
    writer.vec4([white, SENSOR_CLIP_LEVEL, awb.saturation_threshold, 0.0]);
    for row in params.color.camera_to_xyz_d50().rows {
        writer.vec4([row[0], row[1], row[2], 0.0]);
    }
    for row in params.color.xyz_d50_to_scene().rows {
        writer.vec4([row[0], row[1], row[2], 0.0]);
    }
    let gamut = params.gamut_compression.parameters();
    writer.vec4([gamut.threshold[0], gamut.threshold[1], gamut.threshold[2], gamut.power]);
    // Эталон считает `1.0 / power` в f32 на каждый пиксель — здесь та же операция один раз.
    writer.vec4([gamut.scale[0], gamut.scale[1], gamut.scale[2], 1.0 / gamut.power]);
    let tone = params.tone.parameters();
    writer.vec4([
        tone.contrast,
        tone.scale_pow_contrast,
        tone.white_normalization,
        tone.hue_preservation,
    ]);
    writer.uvec4(rect_words(geometry.crop));
    writer.uvec4([
        geometry.output.width,
        geometry.output.height,
        geometry.quarter_turns,
        geometry.raw10_row_stride_bytes,
    ]);
    writer.uvec4(position_channels(params).map(|channel| channel.index() as u32));
    writer.uvec4([awb.grid.columns, awb.grid.rows, 0, 0]);
    writer.finish()
}

/// Цвет каждой позиции ячейки 2×2 (порядок строк). Кадрирование начинается с чётного пикселя,
/// поэтому позиции ячейки кадрирования совпадают с позициями кадра.
fn position_channels(params: &IspParams) -> [RgbChannel; CELL_POSITIONS] {
    [(0, 0), (1, 0), (0, 1), (1, 1)].map(|(x, y)| params.pattern.channel_of_cell(CfaCell::of_pixel(x, y)))
}

fn rect_words(rect: PixelRect) -> [u32; 4] {
    [rect.left, rect.top, rect.size.width, rect.size.height]
}

/// Последовательная запись строк std140 по 16 байт.
#[derive(Default)]
struct Std140Writer {
    bytes: Vec<u8>,
}

impl Std140Writer {
    fn vec4(&mut self, values: [f32; 4]) {
        self.bytes.extend(values.iter().flat_map(|value| value.to_ne_bytes()));
    }

    fn uvec4(&mut self, values: [u32; 4]) {
        self.bytes.extend(values.iter().flat_map(|value| value.to_ne_bytes()));
    }

    /// Готовый блок. Число строк фиксировано кодом `pack_uniforms`; несовпадение — ошибка в этом
    /// модуле, которую ловит тест `layout_matches_documented_offsets` (в проде лишние байты не пишутся,
    /// недостающие остаются нулями).
    fn finish(self) -> [u8; VIEWFINDER_UNIFORM_BYTES] {
        let mut block = [0_u8; VIEWFINDER_UNIFORM_BYTES];
        let length = self.bytes.len().min(VIEWFINDER_UNIFORM_BYTES);
        block[..length].copy_from_slice(&self.bytes[..length]);
        block
    }
}

#[cfg(test)]
#[path = "uniforms_tests.rs"]
mod tests;
