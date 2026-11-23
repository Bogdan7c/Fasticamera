//! Эталонный конвейер целиком: RAW → сигнал экрана, со всеми промежуточными стадиями.
//!
//! Промежуточные изображения сохраняются, потому что GPU-ISP сверяется с эталоном
//! стадия за стадией: расхождение сразу показывает, какая стадия виновата.

use crate::cfa::BayerPattern;
use crate::color::stage::{camera_rgb_to_xyz_d50, xyz_d50_to_scene_linear};
use crate::demosaic::demosaic;
use crate::gamut::compress_to_display_gamut;
use crate::image::{
    CameraMosaic, DisplayEncodedImage, DisplayLinearImage, LinearCameraRgb, RawMosaic, SceneInGamutImage,
    SceneLinearImage, ShadingCorrectedMosaic, WhiteBalancedMosaic, XyzD50Image,
};
use crate::lens_shading::{LensShadingOutcome, LensShadingSampling, apply_lens_shading_with};
use crate::levels::normalize_levels;
use crate::params::IspParams;
use crate::tone::apply_tone_curve;
use crate::transfer::encode_display;
use crate::white_balance::apply_white_balance;

/// Кадр не подходит к параметрам.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PipelineError {
    #[error("раскладка CFA кадра {frame:?} не совпадает с раскладкой параметров {params:?}")]
    PatternMismatch { frame: BayerPattern, params: BayerPattern },
}

/// Все стадии одного прогона эталона.
#[derive(Debug, Clone, PartialEq)]
pub struct ReferenceRun {
    /// 1. Уровни.
    pub normalized: CameraMosaic,
    /// 2. Виньетирование по нашей калибровке (или копия стадии 1 с причиной пропуска).
    ///
    /// Именно эта мозаика — вход статистики AWB (`statistics::compute_awb_statistics`).
    pub lens_shading: LensShadingOutcome,
    pub shading_corrected: ShadingCorrectedMosaic,
    /// 3. Баланс белого и обрезка пересвета.
    pub white_balanced: WhiteBalancedMosaic,
    /// 4. Демозаик.
    pub camera_rgb: LinearCameraRgb,
    /// 5. Цвет: XYZ D50 и сцена в основных цветах экрана.
    pub xyz_d50: XyzD50Image,
    pub scene_linear: SceneLinearImage,
    /// 6а. Сжатие охвата.
    pub scene_in_gamut: SceneInGamutImage,
    /// 6б. Тон.
    pub display_linear: DisplayLinearImage,
    /// 7. Кривая передачи — итог.
    pub display_encoded: DisplayEncodedImage,
}

/// Прогоняет RAW-кадр через все стадии эталона.
pub fn run_reference(raw: &RawMosaic, params: &IspParams) -> Result<ReferenceRun, PipelineError> {
    if raw.pattern() != params.pattern {
        return Err(PipelineError::PatternMismatch {
            frame: raw.pattern(),
            params: params.pattern,
        });
    }
    let normalized = normalize_levels(raw, &params.levels);
    // Мозаика целиком — вся полезная область (начало в нуле, чётное). Видоискатель (Preview) берёт усиление по
    // ячейке, остальные демозаики — по отсчёту (см. `LensShadingSampling`).
    let (shading_corrected, lens_shading) = apply_lens_shading_with(
        &normalized,
        &params.lens_shading,
        LensShadingSampling::for_demosaic(params.demosaic),
    );
    let white_balanced = apply_white_balance(&shading_corrected, &params.white_balance);
    let camera_rgb = demosaic(&white_balanced, params.demosaic);
    let xyz_d50 = camera_rgb_to_xyz_d50(&camera_rgb, params.color.camera_to_xyz_d50());
    let scene_linear = xyz_d50_to_scene_linear(&xyz_d50, params.color.xyz_d50_to_scene(), params.color.gamut());
    let scene_in_gamut = compress_to_display_gamut(&scene_linear, &params.gamut_compression);
    let display_linear = apply_tone_curve(&scene_in_gamut, &params.tone);
    let display_encoded = encode_display(&display_linear);
    Ok(ReferenceRun {
        normalized,
        lens_shading,
        shading_corrected,
        white_balanced,
        camera_rgb,
        xyz_d50,
        scene_linear,
        scene_in_gamut,
        display_linear,
        display_encoded,
    })
}
