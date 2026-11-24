//! «Параметры как у продукта» для стенда насыщения: сенсор, вид и основа тона, карта теней, факты экрана.
//!
//! Стенд меряет цену работ, поэтому входы работ должны быть теми, на которых работает продукт: нейтральные
//! параметры и пустая карта теней дали бы другой шейдер и другую цену. Всё берётся у владельцев:
//! * сенсор — из каталога камер движка через тот же `build_stream_passport`, что у бэкенда камеры, и вшитый
//!   профиль карты теней устройства (`lens_shading_profile`); камера для этого НЕ стримит. Нет каталога или
//!   RAW — задокументированный запасной набор ([`SensorSource::Fallback`]) с пометкой в результате;
//! * вид и основа тона — активный рецепт библиотеки пресетов (`LookHandle`) поверх конфига рендера
//!   (`viewfinder.tone`, `viewfinder.look_model`, `viewfinder.lens_shading`);
//! * факты камеры и вида для экрана — из статуса движка и библиотеки, как у источника экрана.
//!
//! # Отступления от продукта (каждое видно в результате)
//! * нейтраль камеры — фиксированный D65 по профилю сенсора: без потока нет оценки белого от AWB, а цена шейдера
//!   от значения нейтрали не зависит;
//! * охват экрана — Display P3 (экран OnePlus 11), для видоискателя и для качественного конвейера (V4, снимок);
//! * подтяжка AE — единица (как до первого кадра AE).

use std::sync::Arc;

use camera_contract::capabilities::{CameraCapabilities, LensShading, RawSupport, ReferenceIlluminant};
use camera_contract::catalog::CameraCatalog;
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::lens_shading::LensShadingCalibrations;
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, ResolvedStream, TargetFrameRate};
use camera_core::status::CameraStatus;
use camera_screen::viewfinder_map::{MirrorMode, StreamGeometry};
use camera_screen::{CameraFacts, CameraPresence, ScreenConfig};
use isp_model::cfa::BayerPattern;
use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::{CameraNeutral, DngCalibration, DngColorProfile};
use isp_model::color::matrix::Matrix3;
use isp_model::color::temperature::calibration_illuminant_temperature;
use isp_model::image::DisplayGamut;
use isp_model::levels::NormalizationLevels;
use isp_model::look::LookControls;
use isp_model::params::IspParams;
use isp_model::sensor::SensorDescription;
use isp_vulkan::saturation::{ProductParamsRecipe, SaturationWorkError};
use look_contract::{LookFacts, LookSettings, PresetId, ToneBaseModel};
use render_loop::config::{RenderLoopConfig, ViewfinderToneConfig};
use serde::Serialize;
use ui_core::time::MotionPreference;

use super::catalog_wait::describe_catalog;
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_facts::{camera_facts_from_status, facts_for_absent_engine};
use crate::look_facts::{LookFactsCatalog, LookFactsSource};
use crate::look_service::LookHandle;

/// Роль объектива, чьё описание сенсора берёт стенд: основная камера, как у видоискателя продукта.
const SENSOR_LENS_ROLE: LensRole = LensRole::Wide;
/// Частота режима видоискателя (паспорт требует её; для описания сенсора не важна).
const VIEWFINDER_FRAME_RATE: u32 = 60;
/// Размер RAW-буфера основной камеры OnePlus 11 (факты сессии 00): запасной набор и запасная геометрия.
const FALLBACK_RAW_SIZE: PixelSize = PixelSize::new(4096, 3072);
/// Чёрный уровень запасного набора (RAW10 основной камеры, факты 00).
const FALLBACK_BLACK_LEVEL: f32 = 64.0;
/// Белый уровень запасного набора (RAW10).
const FALLBACK_WHITE_LEVEL: f32 = 1023.0;
/// Нейтраль запасного набора: реальный кадр «лампа, серая карта» основной камеры (фикстура `real-main-lamp-gray-tile`).
const FALLBACK_NEUTRAL: [f64; 3] = [0.515625, 1.0, 0.4990234375];

/// Откуда взято описание сенсора: это пишется в результат, чтобы цифру можно было сопоставить с телефоном.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum SensorSource {
    /// Настоящее описание камеры телефона (каталог движка + профиль карты теней устройства).
    Engine {
        camera: String,
        /// Есть ли у описания наша карта теней объектива (как у продукта).
        lens_shading_calibrated: bool,
    },
    /// Запасной набор из фикстуры основной камеры OnePlus 11; причина — почему не взят настоящий.
    Fallback { reason: String },
}

/// Какой вид и основа тона действовали при сборке входов.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LookSummary {
    /// Имя активного пресета (`none` — библиотеки пресетов в сборке нет).
    pub preset: String,
    pub tone_base: ToneBaseModel,
}

/// Для какого конвейера нужны параметры.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PipelineKind {
    /// Видоискатель и приборы (демозаик `Preview`).
    Viewfinder,
    /// Снимок и стенд V4 (демозаик `Quality`).
    Quality,
}

/// Всё, что работам стенда нужно от продукта.
#[derive(Debug, Clone)]
pub struct ProductInputs {
    pub sensor: SensorDescription,
    pub neutral: CameraNeutral,
    pub sensor_source: SensorSource,
    pub look: LookSummary,
    /// Рецепт видоискателя (основа, ручки тона, вид, режим карты теней продукта).
    recipe: ProductParamsRecipe,
    /// Секции моделей основы тона из конфига продукта: из них `tone_base_for` строит основу ЛЮБОЙ модели
    /// (опыты стенда над моделями, которые не выбраны видом).
    tone_config: ViewfinderToneConfig,
    /// Факты камеры для экрана: объективы, вспышка, геометрия потока (камера «живая»: экран строится как при потоке).
    pub camera_facts: CameraFacts,
    /// Факты вида для экрана калибровки (`None` — библиотеки пресетов в сборке нет).
    pub look_facts: Option<LookFacts>,
    pub screen_config: ScreenConfig,
    pub motion: MotionPreference,
}

impl ProductInputs {
    /// Параметры ISP для конвейера: тот же рецепт продукта, демозаик по виду конвейера.
    pub fn params(&self, pipeline: PipelineKind) -> Result<IspParams, SaturationWorkError> {
        self.resolve_recipe(pipeline, self.recipe.clone())
    }

    /// Параметры ISP для конвейера, но с основой тона ДРУГОЙ модели: секция модели берётся из конфига продукта тем же
    /// `tone_base_for`, что у видоискателя и снимка; ручки, вид и всё остальное — как у [`Self::params`].
    pub fn params_with_tone_model(
        &self,
        pipeline: PipelineKind,
        model: ToneBaseModel,
    ) -> Result<IspParams, SaturationWorkError> {
        let recipe = ProductParamsRecipe {
            tone_base: self.tone_config.tone_base_for(model),
            ..self.recipe.clone()
        };
        self.resolve_recipe(pipeline, recipe)
    }

    /// Рецепт → параметры: демозаик выбирается по виду конвейера.
    fn resolve_recipe(
        &self,
        pipeline: PipelineKind,
        recipe: ProductParamsRecipe,
    ) -> Result<IspParams, SaturationWorkError> {
        let recipe = match pipeline {
            PipelineKind::Viewfinder => recipe,
            PipelineKind::Quality => ProductParamsRecipe {
                demosaic: ProductParamsRecipe::quality(recipe.gamut).demosaic,
                ..recipe
            },
        };
        recipe.resolve(&self.sensor, self.neutral)
    }
}

/// Источники входов (ссылки на сервисы приложения).
pub struct InputSources<'sources> {
    /// Снимок статуса движка до закрытия камеры; `None` — движка нет.
    pub engine_status: Option<&'sources CameraStatus>,
    pub lens_shading: &'sources LensShadingCalibrations,
    pub render_config: &'sources RenderLoopConfig,
    pub look: Option<&'sources LookHandle>,
    pub screen: &'sources CameraScreenConfig,
}

/// Собирает входы стенда. Не падает: чего нет, то заменяется запасным с пометкой причины.
pub fn gather_inputs(sources: &InputSources<'_>) -> ProductInputs {
    let (sensor, sensor_source, stream_geometry) = match sensor_from_engine(sources) {
        Ok(built) => built,
        Err(reason) => (fallback_sensor(), SensorSource::Fallback { reason }, None),
    };
    let neutral = neutral_of(&sensor);
    let (recipe, look, look_facts) = look_of(sources);
    ProductInputs {
        camera_facts: live_camera_facts(sources.engine_status, stream_geometry),
        sensor,
        neutral,
        sensor_source,
        look,
        recipe,
        tone_config: sources.render_config.viewfinder.tone,
        look_facts,
        screen_config: sources.screen.screen.clone(),
        motion: sources.screen.motion,
    }
}

/// Описание сенсора основной камеры из каталога движка и геометрия потока (для фактов экрана).
fn sensor_from_engine(
    sources: &InputSources<'_>,
) -> Result<(SensorDescription, SensorSource, Option<StreamGeometry>), String> {
    let status = sources.engine_status.ok_or("движок камеры недоступен")?;
    sensor_from_catalog(&status.catalog, sources.lens_shading)
}

/// Описание сенсора основной камеры из каталога и калибровок карты теней (камера не стримит).
pub fn sensor_from_catalog(
    catalog: &CameraCatalog,
    lens_shading: &LensShadingCalibrations,
) -> Result<(SensorDescription, SensorSource, Option<StreamGeometry>), String> {
    let descriptor = catalog
        .resolve(&CameraTarget::Role(SENSOR_LENS_ROLE))
        .map_err(|error| {
            format!(
                "камеры основной роли нет в каталоге: {error}; {}",
                describe_catalog(catalog)
            )
        })?;
    let RawSupport::Supported(_) = &descriptor.capabilities.raw else {
        return Err("основная камера без RAW".to_owned());
    };
    let size = raw_buffer_size(&descriptor.capabilities);
    let frame_rate = TargetFrameRate::new(VIEWFINDER_FRAME_RATE).map_err(|error| error.to_string())?;
    let stream = ResolvedStream {
        camera: descriptor.id.clone(),
        raw_format: RawFormat::Raw10,
        buffer_size: size,
        readout: ReadoutAspect::FourByThree,
        frame_rate,
    };
    // Тот же сборщик паспорта, что у бэкенда камеры: правило «где настоящие данные» живёт в одном месте.
    let passport = camera_android::stream_passport::build_stream_passport(
        &stream,
        &descriptor.capabilities,
        FrameRateMechanism::HalDefault,
        lens_shading,
    )
    .map_err(|error| format!("паспорт потока: {error}"))?;
    let sensor = SensorDescription::from_stream_passport(&passport, &passport.color_calibration)
        .map_err(|error| format!("описание сенсора: {error}"))?;
    let geometry = StreamGeometry {
        valid_area: passport.valid_area,
        active_array_size: descriptor.capabilities.active_array.size,
        sensor_orientation: passport.sensor_orientation,
        mirror: MirrorMode::None,
    };
    let source = SensorSource::Engine {
        camera: descriptor.id.to_string(),
        lens_shading_calibrated: sensor.lens_shading_map().is_some(),
    };
    Ok((sensor, source, Some(geometry)))
}

/// Размер RAW10-буфера камеры по её карте потоков; нет записи — размер основной камеры OnePlus 11.
fn raw_buffer_size(capabilities: &CameraCapabilities) -> PixelSize {
    capabilities
        .raw_streams
        .iter()
        .find(|option| option.format == RawFormat::Raw10)
        .map_or(FALLBACK_RAW_SIZE, |option| option.size)
}

/// Нейтраль камеры для D65 по её DNG-профилю; профиль не дал — нейтраль запасного набора.
fn neutral_of(sensor: &SensorDescription) -> CameraNeutral {
    sensor
        .color_profile()
        .neutral_for_white(Chromaticity::D65)
        .unwrap_or_else(|error| {
            tracing::warn!(target: "fasticamera::saturation", %error, "нейтраль для D65 не посчитана: запасная");
            fallback_neutral()
        })
}

fn fallback_neutral() -> CameraNeutral {
    // Константа проверена тестом `fallback_set_resolves_isp_params`: значения конечные и положительные.
    CameraNeutral::new(FALLBACK_NEUTRAL).unwrap_or_else(|_| unreachable!("запасная нейтраль положительна и конечна"))
}

/// Запасной набор: основная камера OnePlus 11 (BGGR, RAW10 64/1023, две калибровки света D65 и A из фикстуры
/// `real-main-lamp-gray-tile`). Цена шейдера от значений матриц не зависит, но путь тот же, что у продукта.
pub fn fallback_sensor() -> SensorDescription {
    let calibration = |illuminant: ReferenceIlluminant, color: [f64; 9], calibration: [f64; 9]| {
        let temperature = calibration_illuminant_temperature(illuminant)
            .unwrap_or_else(|_| unreachable!("D65 и A есть в таблице эталонных светов"));
        DngCalibration {
            temperature,
            color_matrix: matrix(color),
            forward_matrix: Some(matrix(FALLBACK_FORWARD_MATRIX)),
            camera_calibration: matrix(calibration),
        }
    };
    let profile = DngColorProfile::new(
        calibration(
            ReferenceIlluminant::D65,
            [
                0.9848, -0.3684, -0.1011, -0.2682, 1.1295, 0.1152, 0.0588, 0.0403, 0.5348,
            ],
            [1.0078125, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0078125],
        ),
        Some(calibration(
            ReferenceIlluminant::StandardA,
            [1.9084, -1.3277, 0.1113, -0.2221, 1.1645, 0.2236, 0.141, -0.0884, 0.8508],
            [0.9921875, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.9765625],
        )),
        [1.0, 1.0, 1.0],
    )
    .unwrap_or_else(|_| unreachable!("запасной профиль собирается: константы проверены тестом"));
    let levels = NormalizationLevels::new([FALLBACK_BLACK_LEVEL; 4], FALLBACK_WHITE_LEVEL)
        .unwrap_or_else(|_| unreachable!("уровни 64/1023 допустимы"));
    SensorDescription::new(BayerPattern::BGGR, levels, profile, LensShading::AppliedByVendor)
}

/// ForwardMatrix запасного набора (одна на оба света, как в фикстуре).
const FALLBACK_FORWARD_MATRIX: [f64; 9] = [
    0.4375, 0.3828125, 0.140625, 0.21875, 0.71875, 0.0625, 0.015625, 0.09375, 0.7109375,
];

fn matrix(values: [f64; 9]) -> Matrix3 {
    Matrix3::from_rows([
        [values[0], values[1], values[2]],
        [values[3], values[4], values[5]],
        [values[6], values[7], values[8]],
    ])
}

/// Рецепт продукта, краткая запись вида и факты вида для экрана.
fn look_of(sources: &InputSources<'_>) -> (ProductParamsRecipe, LookSummary, Option<LookFacts>) {
    let viewfinder = &sources.render_config.viewfinder;
    let (settings, base, preset_name, look_facts) = match sources.look {
        Some(handle) => {
            let state = handle.view_state();
            let name = preset_display_name(&state.active.preset, &state.presets);
            let facts = LookFactsSource::new(
                handle.clone(),
                LookFactsCatalog::from_render_config(sources.render_config, &sources.screen.band_swatch)
                    .with_wheel_ring_style(&sources.screen.wheel_ring),
            )
            .current_facts(None);
            (
                state.active.recipe.settings,
                state.active.recipe.base,
                name,
                Some(facts),
            )
        }
        None => (
            LookSettings::NEUTRAL,
            viewfinder.tone.base.into(),
            "none".to_owned(),
            None,
        ),
    };
    let recipe = ProductParamsRecipe {
        tone: viewfinder.tone.curve_settings(),
        tone_base: viewfinder.tone.tone_base_for(base),
        lens_shading: viewfinder.lens_shading.control(),
        look: LookControls::new(settings, Arc::new(viewfinder.look_model.clone())),
        ..ProductParamsRecipe::viewfinder()
    };
    (
        recipe,
        LookSummary {
            preset: preset_name,
            tone_base: base,
        },
        look_facts,
    )
}

fn preset_display_name(active: &PresetId, presets: &[look_contract::PresetEntry]) -> String {
    presets
        .iter()
        .find(|entry| entry.id == *active)
        .map_or_else(|| format!("{active:?}"), |entry| entry.name.as_str().to_owned())
}

/// Факты камеры для экрана: из статуса движка, но «живая» камера (экран строится как при идущем потоке:
/// раскладка и значки те же, что у владельца, а не экран «камера недоступна»).
fn live_camera_facts(status: Option<&CameraStatus>, geometry: Option<StreamGeometry>) -> CameraFacts {
    let mut facts = status.map_or_else(facts_for_absent_engine, camera_facts_from_status);
    facts.presence = CameraPresence::Live;
    if facts.stream.is_none() {
        facts.stream = Some(geometry.unwrap_or_else(fallback_stream_geometry));
    }
    facts
}

/// Геометрия потока основной камеры OnePlus 11 (сенсор 4096×3072, 90°, полезная область — весь буфер).
fn fallback_stream_geometry() -> StreamGeometry {
    StreamGeometry {
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: FALLBACK_RAW_SIZE,
        },
        active_array_size: FALLBACK_RAW_SIZE,
        sensor_orientation: SensorOrientation::Degrees90,
        mirror: MirrorMode::None,
    }
}

/// Охват экрана стенда (см. «Отступления» модуля).
pub const BENCH_DISPLAY_GAMUT: DisplayGamut = DisplayGamut::DisplayP3;

#[cfg(test)]
#[path = "inputs_tests.rs"]
mod tests;
