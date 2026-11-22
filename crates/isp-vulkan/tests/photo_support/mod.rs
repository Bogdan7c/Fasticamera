//! Общие помощники тестов проявления снимка: кадры (реальные кропы и зонная пластинка), загрузка в GPU
//! (RAW16-картинка и упакованный RAW10 с отступами и мусором в паддинге), блокирующие захват/проявка,
//! эталон RGBA8 из `isp-model`.
//!
//! Контекст и проверка validation — из `quality_support` (общий с тестами стенда V4).

#![allow(dead_code, reason = "каждый тестовый бинарь использует свою часть помощников")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "помощник тестов: падение и есть способ сообщить о провале"
)]

use std::time::{Duration, Instant};

use camera_contract::geometry::{PixelRect, PixelSize};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::kit_test_support::run_one_shot_blocking_traced;
use gpu_vulkan::raw_mosaic_preview::reference::pack_raw10;
use gpu_vulkan::raw_mosaic_preview::synthetic::{SyntheticRaw10, SyntheticRaw16};
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, ValidArea};
use isp_model::cfa::BayerPattern;
use isp_model::color::dng::CameraNeutral;
use isp_model::demosaic::DemosaicMethod;
use isp_model::exposure::ExposureGain;
use isp_model::image::{DisplayGamut, RawMosaic};
use isp_model::params::{FrameInputs, IspParams, LensShadingMapInput, ProcessingControls};
use isp_model::pipeline::{ReferenceRun, run_reference};
use isp_model::sensor::SensorDescription;
use isp_model::test_patterns::zone_plate;
use isp_vulkan::photo::{
    DevelopRequest, DevelopedPhoto, PhotoConfig, PhotoDeveloper, PhotoError, RawCapture, RawCapturer, RgbaReadback,
    SliceOutcome, quantize_display_encoded,
};
use isp_vulkan::quality::LinearRgbPlanes;

#[path = "../../../isp-model/tests/support/mod.rs"]
pub mod isp_support;
pub use isp_support::real_capture::load_real_capture;

/// Кадр-источник: RAW и параметры проявления.
pub struct SourceFrame {
    pub name: String,
    pub raw: RawMosaic,
    pub params: IspParams,
    pub sensor: SensorDescription,
    pub neutral: CameraNeutral,
}

impl SourceFrame {
    /// Тот же кадр с остаточной подтяжкой экспозиции `ev` ступеней (`k` нашего AE: она часть `IspParams` кадра,
    /// снимок обязан её повторять).
    pub fn with_exposure_ev(&self, ev: f32) -> SourceFrame {
        let frame = FrameInputs {
            levels: self.sensor.static_levels(),
            lens_shading_map: LensShadingMapInput::NotProvided,
        };
        let mut controls = ProcessingControls::with_defaults(self.neutral, DemosaicMethod::Quality, DisplayGamut::Srgb);
        controls.exposure_gain = ExposureGain::from_ev(ev).unwrap();
        SourceFrame {
            name: format!("{}+{ev}EV", self.name),
            raw: self.raw.clone(),
            params: IspParams::resolve(&self.sensor, &frame, &controls).unwrap(),
            sensor: self.sensor.clone(),
            neutral: self.neutral,
        }
    }
}

pub fn params_for(sensor: &SensorDescription, neutral: CameraNeutral) -> IspParams {
    let frame = FrameInputs {
        levels: sensor.static_levels(),
        lens_shading_map: LensShadingMapInput::NotProvided,
    };
    let controls = ProcessingControls::with_defaults(neutral, DemosaicMethod::Quality, DisplayGamut::Srgb);
    IspParams::resolve(sensor, &frame, &controls).unwrap()
}

/// Реальный кроп OnePlus 11 (48×48).
pub fn real_frame(name: &str) -> SourceFrame {
    let capture = load_real_capture(&isp_support::assets_dir(), name);
    let params = params_for(&capture.sensor, capture.neutral);
    SourceFrame {
        name: name.to_owned(),
        raw: capture.raw,
        params,
        sensor: capture.sensor,
        neutral: capture.neutral,
    }
}

/// Синтетическая зонная пластинка заданного размера и раскладки CFA (цвета серые, как в тестах V4).
pub fn zone_plate_frame(width: u32, height: u32, pattern: BayerPattern) -> SourceFrame {
    let calibration = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let scene = zone_plate(PixelSize::new(width, height), 0.5).unwrap();
    let sensor = SensorDescription::new(
        pattern,
        calibration.sensor.static_levels(),
        *calibration.sensor.color_profile(),
        calibration.sensor.vendor_lens_shading(),
    );
    let params = params_for(&sensor, calibration.neutral);
    let levels = sensor.static_levels();
    let neutral = calibration.neutral.values();
    let samples: Vec<u16> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let channel = pattern.channel_at(x, y).index();
            let light = f64::from(scene.pixel(x, y)[channel]) * 0.6 * neutral[channel];
            let black = f64::from(levels.black_pattern()[(y as usize % 2) * 2 + x as usize % 2]);
            (black + light * (f64::from(levels.white()) - black)).round() as u16
        })
        .collect();
    SourceFrame {
        name: format!("zone_plate_{width}x{height}_{pattern:?}"),
        raw: RawMosaic::new(PixelSize::new(width, height), pattern, samples).unwrap(),
        params,
        sensor,
        neutral: calibration.neutral,
    }
}

/// Вид кадра камеры в тесте.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Raw16,
    Raw10,
}

/// Где в буфере кадра камеры лежит полезная область (всё чётное) и чем забит паддинг.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub left: u32,
    pub top: u32,
    pub right_padding: u32,
    pub bottom_padding: u32,
    /// Значение паддинга RAW16 (RAW10 забивается байтом 0xA5 и значением 1023 в отсчётах).
    pub padding_value: u16,
}

impl Placement {
    pub const PADDED: Self = Self {
        left: 4,
        top: 2,
        right_padding: 4,
        bottom_padding: 4,
        padding_value: 1023,
    };
}

/// Кадр в памяти GPU (держит владельцев буферов).
pub struct GpuFrame {
    _raw16: Option<SyntheticRaw16>,
    _raw10: Option<SyntheticRaw10>,
    pub source: MosaicSource,
    pub valid_area: ValidArea,
}

pub fn upload(context: &GpuContext, raw: &RawMosaic, kind: SourceKind, placement: Placement) -> GpuFrame {
    let (width, height) = (raw.size().width, raw.size().height);
    let frame_width = placement.left + width + placement.right_padding;
    let frame_height = placement.top + height + placement.bottom_padding;
    let mut frame = vec![placement.padding_value; (frame_width * frame_height) as usize];
    for y in 0..height {
        for x in 0..width {
            frame[((y + placement.top) * frame_width + x + placement.left) as usize] = raw.sample(x, y);
        }
    }
    let valid_area = ValidArea {
        left: placement.left,
        top: placement.top,
        width,
        height,
    };
    match kind {
        SourceKind::Raw16 => {
            let raw16 = SyntheticRaw16::new(context, frame_width, frame_height, &frame).unwrap();
            GpuFrame {
                source: raw16.source(),
                _raw16: Some(raw16),
                _raw10: None,
                valid_area,
            }
        }
        SourceKind::Raw10 => {
            assert_eq!(
                frame_width % 4,
                0,
                "RAW10: ширина кадра {frame_width} должна делиться на 4"
            );
            let row_stride_bytes = frame_width / 4 * 5 + 6;
            let packed = pack_raw10(&frame, frame_width, row_stride_bytes, 0xA5);
            let raw10 = SyntheticRaw10::new(context, frame_width, frame_height, row_stride_bytes, &packed).unwrap();
            GpuFrame {
                source: raw10.source(),
                _raw16: None,
                _raw10: Some(raw10),
                valid_area,
            }
        }
    }
}

/// Переводит ошибку проявления в ошибку GPU для замыканий разовой отправки (остальное — провал теста).
pub fn into_gpu_error(error: PhotoError) -> GpuError {
    match error {
        PhotoError::Gpu(gpu) => gpu,
        other => panic!("проявление снимка: {other}"),
    }
}

/// Захват: запись → отправка с ожиданием fence → подтверждение.
pub fn capture(context: &GpuContext, capturer: &mut RawCapturer, frame: &GpuFrame) -> RawCapture {
    let mut pending = None;
    let run = run_one_shot_blocking_traced(context, |command_buffer| {
        pending = Some(
            capturer
                .record_capture(command_buffer, &frame.source, frame.valid_area)
                .map_err(into_gpu_error)?,
        );
        Ok(())
    })
    .unwrap();
    let mut pending = pending.unwrap();
    pending.note_submitted(&run.work);
    capturer.complete_after_fence(pending, &run.proof).unwrap()
}

/// Итог проявления целиком.
pub struct Developed {
    pub photo: DevelopedPhoto,
    /// RGBA8 готовой картинки, прочитанная из картинки GPU независимым блокирующим копированием.
    pub image_rgba: Vec<u8>,
    /// Время каждой полосы (отправка + ожидание fence на ПК ≈ мс GPU).
    pub slice_times: Vec<Duration>,
}

/// Проявляет, записывая по полосе в отдельный сабмит; `after_slice` зовётся после fence каждой полосы
/// (читать плоскости полосы и т.п.).
pub fn develop_with(
    context: &GpuContext,
    developer: &mut PhotoDeveloper,
    request: &DevelopRequest<'_>,
    after_slice: impl FnMut(&PhotoDeveloper, usize),
) -> Developed {
    let start = developer.begin(request).unwrap();
    drive_slices(context, developer, start, after_slice)
}

/// То же, но цели проявления выделены заранее (`DevelopTargets::allocate`, возможно на другом потоке).
pub fn develop_with_targets(
    context: &GpuContext,
    developer: &mut PhotoDeveloper,
    request: &DevelopRequest<'_>,
    targets: isp_vulkan::photo::DevelopTargets,
) -> Developed {
    let start = developer.begin_with_targets(request, targets).unwrap();
    drive_slices(context, developer, start, |_, _| {})
}

/// Полосы проявки по одной (каждая — свой сабмит с ожиданием), потом `finish` и чтение картинки.
fn drive_slices(
    context: &GpuContext,
    developer: &mut PhotoDeveloper,
    start: isp_vulkan::photo::DevelopmentStart,
    mut after_slice: impl FnMut(&PhotoDeveloper, usize),
) -> Developed {
    let mut slice_times = Vec::new();
    loop {
        let mut outcome = None;
        let began = Instant::now();
        let run = run_one_shot_blocking_traced(context, |command_buffer| {
            outcome = Some(developer.record_next_slice(command_buffer).map_err(into_gpu_error)?);
            Ok(())
        })
        .unwrap();
        match outcome.unwrap() {
            SliceOutcome::Finished => break,
            SliceOutcome::Recorded { remaining } => {
                slice_times.push(began.elapsed());
                developer.note_slice_submitted(&run.work).unwrap();
                developer.slice_completed(&run.proof).unwrap();
                after_slice(developer, slice_times.len() - 1);
                assert_eq!(remaining, start.slice_count - slice_times.len());
            }
        }
    }
    assert_eq!(slice_times.len(), start.slice_count);
    let photo = developer.finish().unwrap();
    let image_rgba = photo.gpu_image().read_back_blocking().unwrap();
    Developed {
        photo,
        image_rgba,
        slice_times,
    }
}

pub fn develop(
    context: &GpuContext,
    developer: &mut PhotoDeveloper,
    raw: &RawCapture,
    frame: &SourceFrame,
    crop: PixelRect,
    readback: RgbaReadback,
) -> Developed {
    develop_with(
        context,
        developer,
        &DevelopRequest {
            raw,
            params: &frame.params,
            crop,
            readback,
        },
        |_, _| {},
    )
}

/// Кроп в координатах БУФЕРА кадра из кропа в координатах полезной области.
pub fn crop_in_buffer(valid_area: ValidArea, left: u32, top: u32, width: u32, height: u32) -> PixelRect {
    PixelRect {
        left: valid_area.left + left,
        top: valid_area.top + top,
        size: PixelSize::new(width, height),
    }
}

pub fn developer_for(context: &GpuContext, slice_rows: u32, max_width: u32) -> PhotoDeveloper {
    let config = PhotoConfig {
        slice_rows,
        ..PhotoConfig::default()
    };
    PhotoDeveloper::new(context, &config, max_width).unwrap()
}

/// Эталон RGBA8 всего кадра: `display_encoded` эталона, квантованный правилом снимка; альфа 255.
pub fn reference_rgba(run: &ReferenceRun) -> Vec<u8> {
    run.display_encoded
        .pixels()
        .iter()
        .flat_map(|pixel| {
            [
                quantize_display_encoded(pixel[0]),
                quantize_display_encoded(pixel[1]),
                quantize_display_encoded(pixel[2]),
                255,
            ]
        })
        .collect()
}

pub fn run_reference_for(frame: &SourceFrame) -> ReferenceRun {
    run_reference(&frame.raw, &frame.params).unwrap()
}

/// Вырезает прямоугольник из RGBA-картинки шириной `width`.
pub fn crop_rgba(rgba: &[u8], width: u32, left: u32, top: u32, crop_width: u32, crop_height: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((crop_width * crop_height * 4) as usize);
    for y in top..top + crop_height {
        let start = ((y * width + left) * 4) as usize;
        out.extend_from_slice(&rgba[start..start + (crop_width * 4) as usize]);
    }
    out
}

/// Наибольшая разница по каналам и число отличающихся байтов; позиция первого отличия (x, y, канал).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RgbaDifference {
    pub max_abs: u8,
    pub differing_bytes: usize,
    pub first: Option<(u32, u32, usize)>,
}

pub fn compare_rgba(got: &[u8], want: &[u8], width: u32) -> RgbaDifference {
    assert_eq!(got.len(), want.len(), "размер картинок");
    let mut result = RgbaDifference {
        max_abs: 0,
        differing_bytes: 0,
        first: None,
    };
    for (index, (a, b)) in got.iter().zip(want).enumerate() {
        if a != b {
            result.max_abs = result.max_abs.max(a.abs_diff(*b));
            result.differing_bytes += 1;
            if result.first.is_none() {
                let pixel = (index / 4) as u32;
                result.first = Some((pixel % width, pixel / width, index % 4));
            }
        }
    }
    result
}

/// Плоскости полосы → пиксели `[R, G, B]` (первые `pixels` отсчётов каждой плоскости, плотные строки).
pub fn read_planes(planes: LinearRgbPlanes<'_>, pixels: usize) -> Vec<[f32; 3]> {
    let read = |plane: &gpu_vulkan::device_buffer::DeviceBuffer| -> Vec<f32> {
        plane
            .read_back_blocking()
            .unwrap()
            .chunks_exact(4)
            .take(pixels)
            .map(|quad| f32::from_ne_bytes([quad[0], quad[1], quad[2], quad[3]]))
            .collect()
    };
    let (red, green, blue) = (read(planes.red), read(planes.green), read(planes.blue));
    red.iter()
        .zip(&green)
        .zip(&blue)
        .map(|((red, green), blue)| [*red, *green, *blue])
        .collect()
}

pub const PATTERNS: [BayerPattern; 4] = [
    BayerPattern::RGGB,
    BayerPattern::GRBG,
    BayerPattern::GBRG,
    BayerPattern::BGGR,
];
