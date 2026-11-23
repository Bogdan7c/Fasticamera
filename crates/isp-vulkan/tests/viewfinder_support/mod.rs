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
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
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
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::lens_shading::LensShadingControl;
use isp_model::lens_shading::{CropInValidArea, LensShadingSampling, apply_lens_shading_sampled};
use isp_model::levels::NormalizationLevels;
use isp_model::levels::normalize_levels;
use isp_model::params::{ExposureGain, FrameInputs, IspParams, ProcessingControls};
use isp_model::pipeline::{ReferenceRun, run_reference};
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

/// Шаг 10-битного выхода — «1 LSB» в отчётах паритета.
pub const ONE_LSB: f32 = 1.0 / 1023.0;

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

/// Синтетический RAW-кадр (значения u16 в пикселях буфера).
#[derive(Debug, Clone)]
pub struct RawFrame {
    pub width: u32,
    pub height: u32,
    pub pattern: BayerPattern,
    pub samples: Vec<u16>,
}

impl RawFrame {
    /// Кадр из функции «значение RAW в пикселе (x, y) с цветом фильтра `channel` (0 R, 1 G, 2 B)».
    pub fn from_fn(width: u32, height: u32, pattern: BayerPattern, value: impl Fn(u32, u32, usize) -> u16) -> Self {
        let samples = (0..width * height)
            .map(|index| {
                let (x, y) = (index % width, index / width);
                value(x, y, pattern.channel_at(x, y).index())
            })
            .collect();
        Self {
            width,
            height,
            pattern,
            samples,
        }
    }

    /// Кадр из линейного света сенсора (0 — чёрный, 1 — белый), округление к ближайшему, обрезка до 1023.
    pub fn from_linear(
        width: u32,
        height: u32,
        pattern: BayerPattern,
        levels: ([f32; 4], f32),
        linear: impl Fn(u32, u32, usize) -> f64,
    ) -> Self {
        let (black, white) = levels;
        Self::from_fn(width, height, pattern, |x, y, channel| {
            let cell_black = f64::from(black[((y % 2) * 2 + (x % 2)) as usize]);
            let value = cell_black + linear(x, y, channel) * (f64::from(white) - cell_black);
            value.round().clamp(0.0, 1023.0) as u16
        })
    }

    pub fn full_rect(&self) -> PixelRect {
        PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(self.width, self.height),
        }
    }

    /// Мозаика эталона для прямоугольника (чётные края: раскладка CFA сохраняется).
    pub fn mosaic(&self, rect: PixelRect) -> RawMosaic {
        let mut samples = Vec::with_capacity(rect.size.pixel_count() as usize);
        for y in rect.top..rect.top + rect.size.height {
            for x in rect.left..rect.left + rect.size.width {
                samples.push(self.samples[(y * self.width + x) as usize]);
            }
        }
        RawMosaic::new(rect.size, self.pattern, samples).unwrap()
    }
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
            output_gamut: self.gamut,
            gamut_compression: self.gamut_compression,
            tone: self.tone,
            exposure_gain: ExposureGain::UNITY,
            lens_shading: LensShadingControl::default(),
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
    let run = run_reference(&raw.mosaic(crop), params).unwrap();
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
    let mosaic = raw.mosaic(crop);
    let normalized = normalize_levels(&mosaic, &params.levels);
    let place = CropInValidArea::at(crop.left - valid_area.left, crop.top - valid_area.top, valid_area.size).unwrap();
    let (shading_corrected, lens_shading) =
        apply_lens_shading_sampled(&normalized, &params.lens_shading, place, sampling).unwrap();
    let white_balanced = apply_white_balance(&shading_corrected, &params.white_balance);
    let camera_rgb = demosaic(&white_balanced, params.demosaic);
    let xyz_d50 = camera_rgb_to_xyz_d50(&camera_rgb, params.color.camera_to_xyz_d50());
    let scene_linear = xyz_d50_to_scene_linear(&xyz_d50, params.color.xyz_d50_to_scene(), params.color.gamut());
    let scene_in_gamut = compress_to_display_gamut(&scene_linear, &params.gamut_compression);
    let display_linear = apply_tone_curve(&scene_in_gamut, &params.tone);
    let display_encoded = encode_display(&display_linear);
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

/// Поворачивает итог эталона в прямую ориентацию.
fn upright_from_run(run: ReferenceRun, orientation: SensorOrientation) -> UprightReference {
    let encoded = &run.display_encoded;
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
    UprightReference {
        width,
        height,
        pixels,
        run,
    }
}

/// Паритет GPU против эталона.
#[derive(Debug, Clone, Copy)]
pub struct Parity {
    /// max |Δ| по всем каналам.
    pub max_abs: f32,
    /// Доля пикселей, где хоть один канал отличается больше 1 LSB 10-битного выхода.
    pub over_one_lsb: f64,
    /// max(|Δ| − шаг хранения) — то, что НЕ объясняется форматом картинки: ошибка математики GPU.
    /// Шаг хранения: fp16 — один ULP fp16 у значения эталона (драйвер вправе округлять к нулю при
    /// записи в картинку), 10 бит UNORM — полшага 1/2046 (округление к ближайшему).
    pub math_excess: f32,
}

/// Шаг хранения значения `value` в формате картинки (см. `Parity::math_excess`).
pub fn storage_step(format: StorageImageFormat, value: f32) -> f32 {
    match format {
        StorageImageFormat::Rgba16Float => {
            let magnitude = value.abs().max(f32::from_bits(0x3880_0000)); // 2^-14: нормальные fp16
            // ULP fp16 = 2^(экспонента − 10).
            2.0_f32.powi(magnitude.log2().floor() as i32 - 10)
        }
        StorageImageFormat::A2B10G10R10Unorm => 0.5 / 1023.0,
    }
}

pub fn parity(gpu: &ViewfinderRun, reference: &UprightReference) -> Parity {
    assert_eq!(
        (gpu.width, gpu.height),
        (reference.width, reference.height),
        "размер выхода"
    );
    let mut max_abs = 0.0_f32;
    let mut math_excess = 0.0_f32;
    let mut over = 0_usize;
    for (gpu_pixel, reference_pixel) in gpu.pixels.iter().zip(&reference.pixels) {
        let mut worst = 0.0_f32;
        for channel in 0..3 {
            let difference = (gpu_pixel[channel] - reference_pixel[channel]).abs();
            worst = worst.max(difference);
            let step = storage_step(gpu.format, reference_pixel[channel]);
            math_excess = math_excess.max(difference - step);
        }
        max_abs = max_abs.max(worst);
        if worst > ONE_LSB {
            over += 1;
        }
    }
    Parity {
        max_abs,
        over_one_lsb: over as f64 / reference.pixels.len() as f64,
        math_excess: math_excess.max(0.0),
    }
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
