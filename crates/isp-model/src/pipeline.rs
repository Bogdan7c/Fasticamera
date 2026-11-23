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
    SceneLinearImage, ShadingCorrectedMosaic, WhiteBalancedMosaic, WorkingDisplayImage, WorkingSceneImage, XyzD50Image,
};
use crate::lens_shading::{CropInValidArea, LensShadingOutcome, LensShadingSampling, apply_lens_shading_with};
use crate::levels::normalize_levels;
use crate::look::stage::apply_look;
use crate::params::IspParams;
use crate::tone::apply_tone_curve;
use crate::tone::base::{ResolvedToneBase, WhitePolicy};
use crate::tone::working_stage::{apply_working_base, working_display_to_output, xyz_d50_to_working_scene};
use crate::transfer::encode_display;
use crate::white_balance::apply_white_balance;
use crate::white_level::{WhiteLevelField, WhiteLevelPlane, apply_white_balance_per_point};

/// Кадр не подходит к параметрам.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PipelineError {
    #[error("раскладка CFA кадра {frame:?} не совпадает с раскладкой параметров {params:?}")]
    PatternMismatch { frame: BayerPattern, params: BayerPattern },
    #[error(
        "параметры выбрали основу в рабочем пространстве (AgX, ACES, filmic): её стадии отдаёт `run_reference_working`, а не `run_reference`"
    )]
    WorkingBaseNeedsWorkingRun,
    #[error("параметры выбрали прежнюю основу тона: её стадии отдаёт `run_reference`, а не `run_reference_working`")]
    LegacyBaseNeedsLegacyRun,
    #[error("основа в рабочем пространстве выбрана, а политика белого не `PerPoint`: плоскости белого точки нет")]
    WhiteLevelsMissing,
    #[error("плоскость белого точки не совпала по размеру с изображением после демозаика")]
    WhitePlaneSizeMismatch,
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
    /// 6б. Тон, затем вид (цвет вида и таблица) и обрезка 0…1: у прежней основы `W` = цвета выхода, значения за
    /// [0, 1] после вида обрезаются. Нейтральный вид даёт выход одного тона бит в бит.
    pub display_linear: DisplayLinearImage,
    /// 7. Кривая передачи — итог.
    pub display_encoded: DisplayEncodedImage,
}

/// Общее начало обеих цепочек: от RAW до XYZ D50. Различается только обрезка пересвета (политика белого).
#[derive(Debug, Clone, PartialEq)]
pub struct FrontEndStages {
    pub normalized: CameraMosaic,
    pub lens_shading: LensShadingOutcome,
    pub shading_corrected: ShadingCorrectedMosaic,
    pub white_balanced: WhiteBalancedMosaic,
    /// Белый каждого пикселя после демозаика (только при [`WhitePolicy::PerPoint`]).
    pub white_levels: Option<WhiteLevelPlane>,
    pub camera_rgb: LinearCameraRgb,
    pub xyz_d50: XyzD50Image,
}

/// Все стадии прогона с основой в рабочем пространстве (AgX, ACES, filmic; решение 176): после XYZ — линейный
/// Rec. 2020.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingReferenceRun {
    /// Стадии от RAW до XYZ D50.
    pub front_end: FrontEndStages,
    /// Белый каждого пикселя (решение 177).
    pub white_levels: WhiteLevelPlane,
    /// 5а. Сцена в линейном Rec. 2020.
    pub working_scene: WorkingSceneImage,
    /// 6. Основа тона выбранной модели (вход, формулы модели), ещё в Rec. 2020.
    pub working_display: WorkingDisplayImage,
    /// 6а. Вид картинки (цвет вида, таблица) в Rec. 2020 после основы; нейтральный вид — копия `working_display`.
    pub working_looked: WorkingDisplayImage,
    /// 6б. Цвета выхода: матрица, мягкое сжатие охвата, обрезка 0…1.
    pub display_linear: DisplayLinearImage,
    /// 7. Кривая передачи — итог.
    pub display_encoded: DisplayEncodedImage,
}

/// Прогоняет RAW-кадр до XYZ D50 с политикой белого из параметров.
fn run_front_end(raw: &RawMosaic, params: &IspParams) -> Result<FrontEndStages, PipelineError> {
    if raw.pattern() != params.pattern {
        return Err(PipelineError::PatternMismatch {
            frame: raw.pattern(),
            params: params.pattern,
        });
    }
    let normalized = normalize_levels(raw, &params.levels);
    // Мозаика целиком — вся полезная область (начало в нуле, чётное). Видоискатель (Preview) берёт усиление по
    // ячейке, остальные демозаики — по отсчёту (см. `LensShadingSampling`).
    let sampling = LensShadingSampling::for_demosaic(params.demosaic);
    let (shading_corrected, lens_shading) = apply_lens_shading_with(&normalized, &params.lens_shading, sampling);
    let (white_balanced, white_levels) = match params.white_policy {
        WhitePolicy::SensorLevel => (apply_white_balance(&shading_corrected, &params.white_balance), None),
        WhitePolicy::PerPoint => {
            let field = WhiteLevelField::new(
                &params.lens_shading,
                &params.white_balance,
                CropInValidArea::whole(shading_corrected.size()),
                sampling,
            );
            let balanced = apply_white_balance_per_point(&shading_corrected, &params.white_balance, &field);
            let plane = WhiteLevelPlane::for_demosaic(&field, params.demosaic, shading_corrected.size());
            (balanced, Some(plane))
        }
    };
    let camera_rgb = demosaic(&white_balanced, params.demosaic);
    let xyz_d50 = camera_rgb_to_xyz_d50(&camera_rgb, params.color.camera_to_xyz_d50());
    Ok(FrontEndStages {
        normalized,
        lens_shading,
        shading_corrected,
        white_balanced,
        white_levels,
        camera_rgb,
        xyz_d50,
    })
}

/// Прогоняет RAW-кадр через все стадии эталона с ПРЕЖНЕЙ основой тона.
pub fn run_reference(raw: &RawMosaic, params: &IspParams) -> Result<ReferenceRun, PipelineError> {
    if !matches!(params.tone_base, ResolvedToneBase::Legacy) {
        return Err(PipelineError::WorkingBaseNeedsWorkingRun);
    }
    let front = run_front_end(raw, params)?;
    let scene_linear = xyz_d50_to_scene_linear(&front.xyz_d50, params.color.xyz_d50_to_scene(), params.color.gamut());
    let scene_in_gamut = compress_to_display_gamut(&scene_linear, &params.gamut_compression);
    let toned = apply_tone_curve(&scene_in_gamut, &params.tone);
    // Вид в `W` = цвета выхода; значения вне [0, 1] после цвета вида и таблицы обрезаются (у прежней основы нет
    // выходного сжатия охвата, которое их принимало бы).
    let looked = apply_look(&toned, &params.look);
    let display_linear = looked.map_into(looked.space(), |pixel| pixel.map(|channel| channel.clamp(0.0, 1.0)));
    let display_encoded = encode_display(&display_linear, params.output_profile.transfer());
    Ok(ReferenceRun {
        normalized: front.normalized,
        lens_shading: front.lens_shading,
        shading_corrected: front.shading_corrected,
        white_balanced: front.white_balanced,
        camera_rgb: front.camera_rgb,
        xyz_d50: front.xyz_d50,
        scene_linear,
        scene_in_gamut,
        display_linear,
        display_encoded,
    })
}

/// Прогоняет RAW-кадр через все стадии эталона с основой в рабочем пространстве (AgX, ACES или filmic —
/// выбранную параметрами; цепочка для всех моделей одна).
pub fn run_reference_working(raw: &RawMosaic, params: &IspParams) -> Result<WorkingReferenceRun, PipelineError> {
    let Some(base) = params.tone_base.working_stage() else {
        return Err(PipelineError::LegacyBaseNeedsLegacyRun);
    };
    let mut front_end = run_front_end(raw, params)?;
    let white_levels = front_end.white_levels.take().ok_or(PipelineError::WhiteLevelsMissing)?;
    let working_scene = xyz_d50_to_working_scene(&front_end.xyz_d50, base);
    let working_display =
        apply_working_base(&working_scene, &white_levels, base).ok_or(PipelineError::WhitePlaneSizeMismatch)?;
    // Вид картинки: цвет вида и таблица в Rec. 2020 после основы; значения вне [0, 1] принимает выходное сжатие.
    let working_looked = apply_look(&working_display, &params.look);
    let display_linear = working_display_to_output(&working_looked, base);
    let display_encoded = encode_display(&display_linear, params.output_profile.transfer());
    Ok(WorkingReferenceRun {
        front_end,
        white_levels,
        working_scene,
        working_display,
        working_looked,
        display_linear,
        display_encoded,
    })
}

/// Итог эталона для любой основы: закодированное изображение выхода.
pub fn run_reference_encoded(raw: &RawMosaic, params: &IspParams) -> Result<DisplayEncodedImage, PipelineError> {
    match params.tone_base.working_stage() {
        None => run_reference(raw, params).map(|run| run.display_encoded),
        Some(_) => run_reference_working(raw, params).map(|run| run.display_encoded),
    }
}
