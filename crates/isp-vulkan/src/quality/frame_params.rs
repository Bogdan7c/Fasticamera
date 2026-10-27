//! `IspParams` → байты uniform-блока `FrameParameters` (`shaders/quality_frame_params.glsl`, std140).
//!
//! Чистая логика без Vulkan: GPU получает ровно те f32, что эталон `isp-model` подставляет в формулы
//! (усиления, матрицы, параметры охвата и тона уже разрешены на CPU в `IspParams::resolve`).
//! Здесь же — честные отказы: параметры не той версии, не тот демозаик, виньетирование, которое GPU
//! пока не применяет.

use isp_model::cfa::BayerPattern;
use isp_model::demosaic::DemosaicMethod;
use isp_model::lens_shading::LensShadingStep;
use isp_model::params::{CURRENT_ISP_PARAMS_VERSION, IspParams};

use super::error::QualityError;

/// Размер блока, байт (8 полей vec4 + 2 массива по 3 vec4 = 12 vec4).
pub(crate) const FRAME_PARAMETERS_BYTES: u64 = 192;

/// Смещения полей блока std140, байт (порядок — как в GLSL).
mod offset {
    pub(super) const SOURCE_AREA: usize = 0;
    pub(super) const BLACK_LEVELS: usize = 16;
    pub(super) const WHITE_AND_GAINS: usize = 32;
    pub(super) const CAMERA_TO_XYZ: usize = 48;
    pub(super) const XYZ_TO_SCENE: usize = 96;
    pub(super) const GAMUT_THRESHOLD_POWER: usize = 144;
    pub(super) const GAMUT_SCALE: usize = 160;
    pub(super) const TONE: usize = 176;
}

/// Код раскладки CFA для шейдера (таблица `CHANNELS` в `quality_common.glsl`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CfaCode(u32);

impl CfaCode {
    /// Код раскладки Bayer: RGGB 0, GRBG 1, GBRG 2, BGGR 3.
    pub fn from_pattern(pattern: BayerPattern) -> Self {
        let code = if pattern == BayerPattern::RGGB {
            0
        } else if pattern == BayerPattern::GRBG {
            1
        } else if pattern == BayerPattern::GBRG {
            2
        } else {
            // `BayerPattern` строится только из четырёх констант Bayer 2×2, поэтому остаётся BGGR.
            3
        };
        Self(code)
    }

    /// Число для push-констант.
    pub(crate) fn shader_value(self) -> u32 {
        self.0
    }
}

/// Где в кадре лежит полезная область и как адресовать строки RAW10.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourceAddressing {
    /// Левый край полезной области в кадре.
    pub left: u32,
    /// Верхний край полезной области в кадре.
    pub top: u32,
    /// Шаг строки RAW10, байт (для RAW16 не используется — 0).
    pub row_stride_bytes: u32,
}

/// Проверяет, что параметры подходят качественному конвейеру.
pub(crate) fn check_supported(params: &IspParams) -> Result<(), QualityError> {
    if params.version != CURRENT_ISP_PARAMS_VERSION {
        return Err(QualityError::UnsupportedParams {
            what: format!(
                "версия параметров {:?}, шейдеры написаны под {CURRENT_ISP_PARAMS_VERSION:?}",
                params.version
            ),
        });
    }
    if params.demosaic != DemosaicMethod::Quality {
        return Err(QualityError::UnsupportedParams {
            what: format!(
                "демозаик {:?}: качественный конвейер считает только Quality",
                params.demosaic
            ),
        });
    }
    if let LensShadingStep::Apply(_) = params.lens_shading {
        return Err(QualityError::UnsupportedParams {
            what: "карта виньетирования: GPU-конвейер её пока не применяет (на OnePlus 11 это делает вендор)"
                .to_owned(),
        });
    }
    Ok(())
}

/// Собирает байты блока. Параметры должны пройти [`check_supported`].
pub(crate) fn frame_parameter_bytes(params: &IspParams, source: SourceAddressing) -> Vec<u8> {
    let mut bytes = vec![0_u8; FRAME_PARAMETERS_BYTES as usize];
    let mut put_u32 = |at: usize, values: [u32; 4]| write_words(&mut bytes, at, values.map(u32::to_ne_bytes));
    put_u32(
        offset::SOURCE_AREA,
        [source.left, source.top, source.row_stride_bytes, 0],
    );

    let gains = params.white_balance.as_array();
    let gamut = params.gamut_compression.parameters();
    let tone = params.tone.parameters();
    let camera_to_xyz = params.color.camera_to_xyz_d50().rows;
    let xyz_to_scene = params.color.xyz_d50_to_scene().rows;
    let mut floats: Vec<(usize, [f32; 4])> = vec![
        (offset::BLACK_LEVELS, params.levels.black_pattern()),
        (
            offset::WHITE_AND_GAINS,
            [params.levels.white(), gains[0], gains[1], gains[2]],
        ),
        (
            offset::GAMUT_THRESHOLD_POWER,
            [gamut.threshold[0], gamut.threshold[1], gamut.threshold[2], gamut.power],
        ),
        (
            offset::GAMUT_SCALE,
            [gamut.scale[0], gamut.scale[1], gamut.scale[2], 0.0],
        ),
        (
            offset::TONE,
            [
                tone.contrast,
                tone.scale_pow_contrast,
                tone.white_normalization,
                tone.hue_preservation,
            ],
        ),
    ];
    for (row_index, row) in camera_to_xyz.iter().enumerate() {
        floats.push((offset::CAMERA_TO_XYZ + 16 * row_index, [row[0], row[1], row[2], 0.0]));
    }
    for (row_index, row) in xyz_to_scene.iter().enumerate() {
        floats.push((offset::XYZ_TO_SCENE + 16 * row_index, [row[0], row[1], row[2], 0.0]));
    }
    for (at, values) in floats {
        write_words(&mut bytes, at, values.map(f32::to_ne_bytes));
    }
    bytes
}

/// Пишет четыре 32-битных слова подряд с `at`. Смещения — константы внутри блока, выход за край невозможен.
fn write_words(bytes: &mut [u8], at: usize, words: [[u8; 4]; 4]) {
    for (index, word) in words.iter().enumerate() {
        let start = at + 4 * index;
        if let Some(target) = bytes.get_mut(start..start + 4) {
            target.copy_from_slice(word);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quality::test_params::quality_params;

    fn float_at(bytes: &[u8], at: usize) -> f32 {
        f32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
    }

    #[test]
    fn every_field_lands_at_its_std140_offset() {
        let params = quality_params(BayerPattern::BGGR);
        let source = SourceAddressing {
            left: 8,
            top: 4,
            row_stride_bytes: 5120,
        };
        let bytes = frame_parameter_bytes(&params, source);
        assert_eq!(bytes.len() as u64, FRAME_PARAMETERS_BYTES);
        assert_eq!([u32_at(&bytes, 0), u32_at(&bytes, 4), u32_at(&bytes, 8)], [8, 4, 5120]);
        assert_eq!(float_at(&bytes, 16), params.levels.black_pattern()[0]);
        assert_eq!(float_at(&bytes, 28), params.levels.black_pattern()[3]);
        assert_eq!(float_at(&bytes, 32), params.levels.white());
        assert_eq!(float_at(&bytes, 44), params.white_balance.as_array()[2]);
        let camera_to_xyz = params.color.camera_to_xyz_d50().rows;
        assert_eq!(float_at(&bytes, 48), camera_to_xyz[0][0]);
        assert_eq!(float_at(&bytes, 48 + 16 * 2 + 8), camera_to_xyz[2][2]);
        let xyz_to_scene = params.color.xyz_d50_to_scene().rows;
        assert_eq!(float_at(&bytes, 96 + 16), xyz_to_scene[1][0]);
        let gamut = params.gamut_compression.parameters();
        assert_eq!(float_at(&bytes, 144 + 12), gamut.power);
        assert_eq!(float_at(&bytes, 160 + 4), gamut.scale[1]);
        let tone = params.tone.parameters();
        assert_eq!(float_at(&bytes, 176), tone.contrast);
        assert_eq!(float_at(&bytes, 188), tone.hue_preservation);
    }

    #[test]
    fn cfa_codes_match_shader_table() {
        assert_eq!(CfaCode::from_pattern(BayerPattern::RGGB).shader_value(), 0);
        assert_eq!(CfaCode::from_pattern(BayerPattern::GRBG).shader_value(), 1);
        assert_eq!(CfaCode::from_pattern(BayerPattern::GBRG).shader_value(), 2);
        assert_eq!(CfaCode::from_pattern(BayerPattern::BGGR).shader_value(), 3);
    }

    #[test]
    fn preview_demosaic_and_old_version_are_refused() {
        let mut params = quality_params(BayerPattern::RGGB);
        assert!(check_supported(&params).is_ok());
        params.demosaic = DemosaicMethod::Preview;
        assert!(matches!(
            check_supported(&params),
            Err(QualityError::UnsupportedParams { .. })
        ));
        let mut params = quality_params(BayerPattern::RGGB);
        params.version = isp_model::params::IspParamsVersion(0);
        assert!(matches!(
            check_supported(&params),
            Err(QualityError::UnsupportedParams { .. })
        ));
    }
}
