//! Общие помощники GPU-тестов ISP видоискателя: контекст, синтетические RAW-кадры, прогон через
//! `ViewfinderIsp` с readback, эталон `isp-model` в прямой ориентации и метрики паритета.
//!
//! Политика «нет GPU» — как у `gpu-vulkan`: тест ПАДАЕТ с понятным текстом, если только не задано
//! `FASTICAMERA_SKIP_GPU_TESTS=1` (тогда явный пропуск).

#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "помощник тестов: каждый бинарь берёт свою часть; падение — способ сообщить о провале"
)]

#[path = "../../../isp-model/tests/support/mod.rs"]
pub mod isp_support;

use ash::vk;
use camera_contract::capabilities::LensShading;
use camera_contract::geometry::{PixelRect, SensorOrientation};
use gpu_vulkan::backdrop::BackdropFrameNumber;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::{read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::raw_mosaic_preview::reference::pack_raw10;
use gpu_vulkan::raw_mosaic_preview::synthetic::{SyntheticRaw10, SyntheticRaw16};
use gpu_vulkan::storage_image::StorageImageFormat;
use gpu_vulkan::swapchain::DisplayColorSpace;
use gpu_vulkan::validation::ValidationMode;
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::CameraNeutral;
use isp_model::color::stage::{camera_rgb_to_xyz_d50, xyz_d50_to_scene_linear};
use isp_model::demosaic::DemosaicMethod;
use isp_model::demosaic::demosaic;
use isp_model::gamut::GamutCompressionSettings;
use isp_model::gamut::compress_to_display_gamut;
use isp_model::image::DisplayGamut;
use isp_model::lens_shading::LensShadingControl;
use isp_model::lens_shading::{CropInValidArea, LensShadingSampling, apply_lens_shading_sampled};
use isp_model::levels::NormalizationLevels;
use isp_model::levels::normalize_levels;
use isp_model::look::stage::apply_look;
use isp_model::output::OutputProfile;
use isp_model::params::{ExposureGain, FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::{ReferenceRun, run_reference, run_reference_encoded};
use isp_model::sensor::SensorDescription;
use isp_model::statistics::{
    AwbStatistics, AwbStatisticsInput, AwbStatisticsSettings, TileGrid, cells_size, compute_awb_statistics,
    quantize_sample_with_headroom, tile_of_cell,
};
use isp_model::tone::ToneCurveSettings;
use isp_model::tone::apply_tone_curve;
use isp_model::transfer::encode_display;
use isp_model::white_balance::apply_white_balance;
use isp_vulkan::viewfinder::{
    IspStageSet, OutputAllocation, ViewfinderFrame, ViewfinderGeometry, ViewfinderIsp, ViewfinderIspConfig,
    ViewfinderSlotReport, ViewfinderStorageFormat,
};

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

// Линейка паритета живёт в библиотеке (`isp_vulkan::saturation::parity`): те же числа меряет приложение на телефоне.
// Здесь только имена, под которыми её знают тесты.
#[allow(unused_imports, reason = "каждый тестовый бинарь использует свою часть помощников")]
pub use isp_vulkan::saturation::parity::{ONE_LSB, Parity, SyntheticRawFrame as RawFrame, storage_step};

/// Нейтраль кадра C7 «лампа» (основная камера, 4532 K).
pub const LAMP: [f64; 3] = [0.515625, 1.0, 0.4990234375];
/// Дневной свет.
pub const DAYLIGHT: [f64; 3] = [0.46, 1.0, 0.68];
/// Очень тёплый свет: самый яркий канал — красный.
pub const RED_BRIGHTEST: [f64; 3] = [1.3, 1.0, 0.6];

/// Контекст `Headless` с полной validation или `None` при осознанном пропуске.
pub fn gpu_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
                None
            } else {
                panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно");
            }
        }
    }
}

/// Финальная проверка: GPU простаивает, validation не сообщила ни одной ошибки.
pub fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}

/// Какой источник кадра подать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLayout {
    /// `R16_UINT`.
    Raw16,
    /// Упакованный RAW10 со строкой длиннее данных на `padding_bytes` (заполнены мусором 0xA5).
    Raw10 { padding_bytes: u32 },
}

/// Источник на GPU (держит память, пока жив).
pub enum GpuSource {
    Raw16(SyntheticRaw16),
    Raw10(SyntheticRaw10),
}

impl GpuSource {
    pub fn new(context: &GpuContext, raw: &RawFrame, layout: SourceLayout) -> Self {
        match layout {
            SourceLayout::Raw16 => {
                Self::Raw16(SyntheticRaw16::new(context, raw.width, raw.height, &raw.samples).unwrap())
            }
            SourceLayout::Raw10 { padding_bytes } => {
                assert_eq!(raw.width % 4, 0, "RAW10: ширина кратна 4");
                let stride = raw.width * 5 / 4 + padding_bytes;
                let packed = pack_raw10(&raw.samples, raw.width, stride, 0xA5);
                Self::Raw10(SyntheticRaw10::new(context, raw.width, raw.height, stride, &packed).unwrap())
            }
        }
    }

    pub fn source(&self) -> MosaicSource {
        match self {
            Self::Raw16(raw) => raw.source(),
            Self::Raw10(raw) => raw.source(),
        }
    }
}

/// Описание параметров кадра для синтетики: калибровка цвета — настоящая (основная камера OnePlus 11).
#[derive(Debug, Clone, Copy)]
pub struct ParamsSpec {
    pub pattern: BayerPattern,
    pub black: [f32; 4],
    pub white: f32,
    pub neutral: [f64; 3],
    pub gamut: DisplayGamut,
    pub tone: ToneCurveSettings,
    pub gamut_compression: GamutCompressionSettings,
    /// Основа тона (по умолчанию прежняя: старые тесты остаются бит в бит).
    pub tone_base: isp_model::tone::base::ToneBase,
}

impl Default for ParamsSpec {
    fn default() -> Self {
        Self {
            pattern: BayerPattern::BGGR,
            black: [64.0; 4],
            white: 1023.0,
            neutral: LAMP,
            gamut: DisplayGamut::DisplayP3,
            tone: ToneCurveSettings::default(),
            gamut_compression: GamutCompressionSettings::default(),
            tone_base: isp_model::tone::base::ToneBase::Legacy,
        }
    }
}

impl ParamsSpec {
    pub fn levels(&self) -> ([f32; 4], f32) {
        (self.black, self.white)
    }

    pub fn display(&self) -> DisplayColorSpace {
        match self.gamut {
            DisplayGamut::Srgb => DisplayColorSpace::Srgb,
            DisplayGamut::DisplayP3 => DisplayColorSpace::DisplayP3,
        }
    }

    pub fn build(&self) -> IspParams {
        self.build_with_look(isp_model::look::LookControls::default())
    }

    /// Те же параметры с видом картинки `look` (ручки цвета, теплота, таблица): значения решены в `IspParams`.
    pub fn build_with_look(&self, look: isp_model::look::LookControls) -> IspParams {
        self.build_with_look_and_exposure(look, ExposureGain::UNITY)
    }

    /// Те же параметры с видом `look` и подтяжкой AE `exposure_gain` (серый сцены модели делится на неё): нужна краевым
    /// тестам ручек тона.
    pub fn build_with_look_and_exposure(
        &self,
        look: isp_model::look::LookControls,
        exposure_gain: ExposureGain,
    ) -> IspParams {
        let calibrated =
            isp_support::real_capture::load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile").sensor;
        let levels = NormalizationLevels::new(self.black, self.white).unwrap();
        let sensor = SensorDescription::new(
            self.pattern,
            levels,
            *calibrated.color_profile(),
            LensShading::AppliedByVendor,
        );
        let frame = FrameInputs { levels };
        let controls = ProcessingControls {
            white_balance: CameraNeutral::new(self.neutral).unwrap(),
            demosaic: DemosaicMethod::Preview,
            output_profile: OutputProfile::sdr(self.gamut),
            gamut_compression: self.gamut_compression,
            tone: self.tone,
            exposure_gain,
            lens_shading: LensShadingControl::default(),
            tone_base: self.tone_base,
            look,
        };
        IspParams::resolve(&sensor, &frame, &controls).unwrap()
    }
}

/// Конфиг тестов: сетка AWB 4×3 (влезает в маленькие кадры), формат по выбору.
pub fn test_config(format: ViewfinderStorageFormat) -> ViewfinderIspConfig {
    ViewfinderIspConfig {
        frame_slots: 2,
        awb_statistics: AwbStatisticsSettings {
            grid: TileGrid { columns: 4, rows: 3 },
            ..AwbStatisticsSettings::default()
        },
        storage_format: format,
        // Наибольший кадр тестов — плоское поле 256×192: даже сетка 1×1 на нём не переполняет суммы.
        largest_mosaic: isp_vulkan::viewfinder::LargestMosaicConfig {
            width: 256,
            height: 192,
        },
        // Тесты точно считают варианты пайплайна: прогрев моделей основы включают сами тесты прогрева.
        variant_prewarm: isp_vulkan::viewfinder::VariantPrewarm::Off,
        ..ViewfinderIspConfig::default()
    }
}

/// Что вернул один прогон.
#[derive(Debug, Clone)]
pub struct ViewfinderRun {
    pub width: u32,
    pub height: u32,
    pub format: StorageImageFormat,
    pub pixels: Vec<[f32; 4]>,
    pub report: ViewfinderSlotReport,
}

impl ViewfinderRun {
    pub fn pixel(&self, x: u32, y: u32) -> [f32; 4] {
        self.pixels[(y * self.width + x) as usize]
    }
}

/// Один кадр от начала до конца.
#[derive(Debug, Clone, Copy)]
pub struct RunRequest<'a> {
    pub raw: &'a RawFrame,
    pub layout: SourceLayout,
    pub params: &'a IspParams,
    pub geometry: ViewfinderGeometry,
    pub display: DisplayColorSpace,
    pub stages: IspStageSet,
}

impl<'a> RunRequest<'a> {
    /// Весь кадр, без поворота, RAW16, все стадии.
    pub fn whole_frame(raw: &'a RawFrame, params: &'a IspParams, display: DisplayColorSpace) -> Self {
        Self {
            raw,
            layout: SourceLayout::Raw16,
            params,
            geometry: ViewfinderGeometry {
                valid_area: raw.full_rect(),
                crop: raw.full_rect(),
                sensor_orientation: SensorOrientation::Degrees0,
                allocation: OutputAllocation::Exact,
            },
            display,
            stages: IspStageSet::ALL,
        }
    }
}

/// Прогоняет кадр через новый `ViewfinderIsp` в одной разовой отправке, читает изображение и итоги слота.
pub fn run_viewfinder(context: &GpuContext, config: &ViewfinderIspConfig, request: &RunRequest<'_>) -> ViewfinderRun {
    let gpu_source = GpuSource::new(context, request.raw, request.layout);
    let source = gpu_source.source();
    let mut isp = ViewfinderIsp::new(context, config).unwrap();
    record_and_submit(context, &mut isp, request, &source, 0, BackdropFrameNumber(1));
    readback(&mut isp, 0)
}

/// Записывает и отправляет кадр в слот `slot` (блокирующе), как поток рендера: приём кадра → ISP → возврат.
pub fn record_and_submit(
    context: &GpuContext,
    isp: &mut ViewfinderIsp,
    request: &RunRequest<'_>,
    source: &MosaicSource,
    slot: usize,
    frame: BackdropFrameNumber,
) {
    run_one_shot_blocking(context, |command_buffer| {
        source.record_acquire_for(context, command_buffer, ConsumerStage::Compute);
        isp.record(
            command_buffer,
            &ViewfinderFrame {
                source,
                params: request.params,
                geometry: request.geometry,
                display_color_space: request.display,
                stages: request.stages,
                frame,
                frame_slot: slot,
                reader: ConsumerStage::Fragment,
            },
        )
        .map_err(|error| gpu_vulkan::error::GpuError::InvalidState {
            what: format!("record: {error}"),
        })?;
        source.record_release_for(context, command_buffer, ConsumerStage::Compute);
        Ok(())
    })
    .unwrap();
}

/// Пытается записать кадр в слот `slot` и отдаёт итог `record` как есть (для проверки отказов ДО команд: команд
/// при отказе нет, отправка пустая).
pub fn try_record(
    context: &GpuContext,
    isp: &mut ViewfinderIsp,
    request: &RunRequest<'_>,
    source: &MosaicSource,
    slot: usize,
) -> Result<(), isp_vulkan::viewfinder::ViewfinderError> {
    let mut outcome = None;
    run_one_shot_blocking(context, |command_buffer| {
        outcome = Some(isp.record(
            command_buffer,
            &ViewfinderFrame {
                source,
                params: request.params,
                geometry: request.geometry,
                display_color_space: request.display,
                stages: request.stages,
                frame: BackdropFrameNumber(1),
                frame_slot: slot,
                reader: ConsumerStage::Fragment,
            },
        ));
        Ok(())
    })
    .unwrap();
    outcome.expect("замыкание отправки вызвано").map(|_| ())
}

/// Читает изображение и итоги слота (после блокирующей отправки fence уже сигнален).
pub fn readback(isp: &mut ViewfinderIsp, slot: usize) -> ViewfinderRun {
    let report = isp.collect(slot).unwrap();
    let image = isp.image().unwrap();
    let whole = read_storage_image_rgba(image.storage_image(), image.current_use()).unwrap();
    // Картинка может быть больше содержимого (ёмкость под зум): отдаём ровно левый верхний прямоугольник кадра.
    let (capacity, content) = (image.extent(), image.content_extent());
    let pixels = (0..content.height)
        .flat_map(|y| (0..content.width).map(move |x| (y * capacity.width + x) as usize))
        .map(|index| whole[index])
        .collect();
    ViewfinderRun {
        width: content.width,
        height: content.height,
        format: image.format(),
        pixels,
        report,
    }
}

/// Эталон в прямой ориентации: прогон `isp-model` по кадрированию + поворот по часовой на `turns`.
pub struct UprightReference {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
    pub run: ReferenceRun,
}

pub fn reference_upright(
    raw: &RawFrame,
    crop: PixelRect,
    orientation: SensorOrientation,
    params: &IspParams,
) -> UprightReference {
    let run = run_reference(&raw.mosaic(crop).unwrap(), params).unwrap();
    upright_from_run(run, orientation)
}

/// Эталон с учётом того, где кадрирование лежит в полезной области: коррекция виньетирования берёт
/// усиление по координатам ПОЛЕЗНОЙ области (`apply_lens_shading_sampled`; выборка — по демозаику: Preview — по ячейке), а не по координатам кропа. Остальные
/// стадии — те же функции, что у `run_reference` (его тело повторено здесь, потому что `run_reference`
/// всегда считает мозаику целой полезной областью); при `crop == valid_area` результат совпадает с
/// `reference_upright` (проверяет `tests/viewfinder_lens_shading.rs`).
pub fn reference_upright_in_valid_area(
    raw: &RawFrame,
    valid_area: PixelRect,
    crop: PixelRect,
    orientation: SensorOrientation,
    params: &IspParams,
) -> UprightReference {
    let sampling = LensShadingSampling::for_demosaic(params.demosaic);
    reference_upright_in_valid_area_sampled(raw, valid_area, crop, orientation, params, sampling)
}

/// То же с явной выборкой усиления (тест различимости: видоискатель берёт усиление по ячейке, не по отсчёту).
pub fn reference_upright_in_valid_area_sampled(
    raw: &RawFrame,
    valid_area: PixelRect,
    crop: PixelRect,
    orientation: SensorOrientation,
    params: &IspParams,
    sampling: LensShadingSampling,
) -> UprightReference {
    let mosaic = raw.mosaic(crop).unwrap();
    let normalized = normalize_levels(&mosaic, &params.levels);
    let place = CropInValidArea::at(crop.left - valid_area.left, crop.top - valid_area.top, valid_area.size).unwrap();
    let (shading_corrected, lens_shading) =
        apply_lens_shading_sampled(&normalized, &params.lens_shading, place, sampling).unwrap();
    let white_balanced = apply_white_balance(&shading_corrected, &params.white_balance);
    let camera_rgb = demosaic(&white_balanced, params.demosaic);
    let xyz_d50 = camera_rgb_to_xyz_d50(&camera_rgb, params.color.camera_to_xyz_d50());
    let scene_linear = xyz_d50_to_scene_linear(&xyz_d50, params.color.xyz_d50_to_scene(), params.color.gamut());
    let scene_in_gamut = compress_to_display_gamut(&scene_linear, &params.gamut_compression);
    let toned = apply_tone_curve(&scene_in_gamut, &params.tone);
    // Вид картинки в `W` = цвета выхода; значения вне [0, 1] обрезает кодирование (`srgb_encode`), как и у
    // `run_reference`, где обрезка стоит перед ним.
    let display_linear = apply_look(&toned, &params.look);
    let display_encoded = encode_display(&display_linear, params.output_profile.transfer());
    upright_from_run(
        ReferenceRun {
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
        },
        orientation,
    )
}

/// Итог эталона любой основы тона (без стадий): закодированные пиксели выхода в прямой ориентации.
pub struct UprightPixels {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[f32; 3]>,
}

/// Эталон ЛЮБОЙ основы тона (`run_reference_encoded`) по кадрированию, повёрнутый в прямую ориентацию. Стадии
/// не отдаются: для прежней основы с ними есть [`reference_upright`].
pub fn reference_upright_encoded(
    raw: &RawFrame,
    crop: PixelRect,
    orientation: SensorOrientation,
    params: &IspParams,
) -> UprightPixels {
    let encoded = run_reference_encoded(&raw.mosaic(crop).unwrap(), params).unwrap();
    upright_pixels(&encoded, orientation)
}

/// Поворачивает итог эталона в прямую ориентацию.
fn upright_from_run(run: ReferenceRun, orientation: SensorOrientation) -> UprightReference {
    let pixels = upright_pixels(&run.display_encoded, orientation);
    UprightReference {
        width: pixels.width,
        height: pixels.height,
        pixels: pixels.pixels,
        run,
    }
}

/// Поворот закодированного изображения в прямую ориентацию.
pub(super) fn upright_pixels(
    encoded: &isp_model::image::DisplayEncodedImage,
    orientation: SensorOrientation,
) -> UprightPixels {
    let (cells_width, cells_height) = (encoded.size().width, encoded.size().height);
    let turns = orientation.quarter_turns_clockwise();
    let (width, height) = if turns % 2 == 1 {
        (cells_height, cells_width)
    } else {
        (cells_width, cells_height)
    };
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for upright_y in 0..height {
        for upright_x in 0..width {
            // Поворот кадра по часовой на turns·90°: пиксель (x, y) сенсора уходит в
            // 90° → (H−1−y, x), 180° → (W−1−x, H−1−y), 270° → (y, W−1−x); здесь — обратное отображение.
            let (sensor_x, sensor_y) = match turns {
                1 => (upright_y, cells_height - 1 - upright_x),
                2 => (cells_width - 1 - upright_x, cells_height - 1 - upright_y),
                3 => (cells_width - 1 - upright_y, upright_x),
                _ => (upright_x, upright_y),
            };
            pixels.push(encoded.pixel(sensor_x, sensor_y));
        }
    }
    UprightPixels { width, height, pixels }
}

pub fn parity(gpu: &ViewfinderRun, reference: &UprightReference) -> Parity {
    parity_with_pixels(gpu, reference.width, reference.height, &reference.pixels)
}

/// Паритет GPU против эталона, заданного пикселями (любая основа тона).
pub fn parity_with_pixels(gpu: &ViewfinderRun, width: u32, height: u32, reference_pixels: &[[f32; 3]]) -> Parity {
    isp_vulkan::saturation::parity::compare_with_reference(
        &gpu.pixels,
        (gpu.width, gpu.height),
        gpu.format,
        reference_pixels,
        (width, height),
    )
    .expect("размер выхода совпадает с эталоном")
}

/// Размер выхода как `vk::Extent2D` (для сравнения с `ViewfinderGeometry::output_extent`).
pub fn extent(run: &ViewfinderRun) -> vk::Extent2D {
    vk::Extent2D {
        width: run.width,
        height: run.height,
    }
}

/// Ячейки, где ±`ulps` ULP нормированного отсчёта меняют квантование (`quantization`) или пересвет.
#[derive(Default, Clone, Copy)]
pub struct BoundaryCells {
    pub quantization: u32,
    pub saturation: u32,
}

/// Эталонная статистика AWB для прогона эталона: пересвет — по нормированной мозаике, суммы — по скорректированной.
pub fn cpu_awb_statistics(run: &ReferenceRun, settings: &AwbStatisticsSettings) -> AwbStatistics {
    compute_awb_statistics(
        AwbStatisticsInput::new(&run.normalized, &run.shading_corrected).unwrap(),
        settings,
    )
    .unwrap()
}

/// Пограничные ячейки: пересвет судится по нормированному отсчёту ДО виньетирования, квантование — по
/// скорректированному (с запасом суммы из настроек). ±`ulps` ULP соответствующего значения.
pub fn boundary_cells(run: &ReferenceRun, settings: &AwbStatisticsSettings, ulps: u32) -> Vec<BoundaryCells> {
    let (normalized, corrected) = (&run.normalized, &run.shading_corrected);
    let grid = settings.grid;
    let cells = cells_size(normalized.size());
    let mut per_tile = vec![BoundaryCells::default(); (grid.columns * grid.rows) as usize];
    let shift = |value: f32| {
        [
            f32::from_bits(value.to_bits().saturating_sub(ulps)),
            f32::from_bits(value.to_bits() + ulps),
        ]
    };
    for cell_y in 0..cells.height {
        for cell_x in 0..cells.width {
            let (column, row) = tile_of_cell(cell_x, cell_y, cells, grid);
            let tile = &mut per_tile[(row * grid.columns + column) as usize];
            let mut quantization = false;
            let mut saturation = false;
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (x, y) = (cell_x * 2 + dx, cell_y * 2 + dy);
                let raw_value = normalized.sample(x, y);
                let threshold = settings.saturation_threshold;
                for neighbour in shift(raw_value) {
                    saturation |= (neighbour < threshold) != (raw_value < threshold);
                }
                let value = corrected.sample(x, y);
                let quantize = |sample| quantize_sample_with_headroom(sample, settings.sum_headroom);
                for neighbour in shift(value) {
                    quantization |= quantize(neighbour) != quantize(value);
                }
            }
            tile.quantization += u32::from(quantization);
            tile.saturation += u32::from(saturation);
        }
    }
    per_tile
}

#[allow(unused_imports, reason = "каждый тестовый бинарь использует свою часть помощников")]
mod reference_agx;
mod scenes;
#[allow(unused_imports, reason = "каждый тестовый бинарь использует свою часть помощников")]
pub use reference_agx::*;
#[allow(unused_imports, reason = "каждый тестовый бинарь использует свою часть помощников")]
pub use scenes::*;
