//! Общие помощники GPU-тестов качественного конвейера (стенд V4): контекст с полной validation,
//! чтение плоскостей f32, сравнение с эталоном `isp-model`.
//!
//! Политика «нет GPU» — как у `gpu-vulkan`: тест ПАДАЕТ, если только не задано
//! `FASTICAMERA_SKIP_GPU_TESTS=1` (тогда печатается явная причина пропуска).

#![allow(dead_code, reason = "каждый тестовый бинарь использует свою часть помощников")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "помощник тестов: падение и есть способ сообщить о провале"
)]

use ash::vk;
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::device_buffer::DeviceBuffer;
use gpu_vulkan::frame_sink::{FrameSink, OffscreenFrameSink, OffscreenFrameSinkDesc};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::{read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, ValidArea};
use gpu_vulkan::validation::ValidationMode;
use isp_model::image::RawMosaic;
use isp_model::params::IspParams;
use isp_vulkan::quality::LinearRgbPlanes;
use isp_vulkan::quality::{
    HeadroomRecordOutcome, QualityError, QualityOutputFormat, StreamGeneration, TemporalDenoiseConfig, VideoFrameInput,
    VideoHeadroomConfig, VideoHeadroomWorkload,
};

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Контекст `Headless` с полной validation (включая синхронизацию) или `None` при осознанном пропуске.
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

/// Финальная проверка: GPU простаивает, validation не нашла ни одной ошибки.
pub fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}

pub fn extent(width: u32, height: u32) -> vk::Extent2D {
    vk::Extent2D { width, height }
}

/// Плоскость f32 → вектор.
pub fn read_plane(plane: &DeviceBuffer) -> Vec<f32> {
    plane
        .read_back_blocking()
        .unwrap()
        .chunks_exact(4)
        .map(|quad| f32::from_ne_bytes([quad[0], quad[1], quad[2], quad[3]]))
        .collect()
}

/// Загружает отсчёты f32 в плоскость.
pub fn upload_plane(plane: &DeviceBuffer, values: &[f32]) {
    let bytes: Vec<u8> = values.iter().flat_map(|value| value.to_ne_bytes()).collect();
    plane.upload_blocking(&bytes).unwrap();
}

/// Три плоскости → пиксели `[R, G, B]`.
pub fn read_rgb(planes: LinearRgbPlanes<'_>) -> Vec<[f32; 3]> {
    let (red, green, blue) = (
        read_plane(planes.red),
        read_plane(planes.green),
        read_plane(planes.blue),
    );
    red.iter()
        .zip(&green)
        .zip(&blue)
        .map(|((red, green), blue)| [*red, *green, *blue])
        .collect()
}

/// Наибольшее |GPU − эталон| по всем каналам и пиксель, где оно достигнуто.
pub fn max_rgb_difference(gpu: &[[f32; 3]], reference: &[[f32; 3]], width: u32) -> (f32, (u32, u32)) {
    assert_eq!(gpu.len(), reference.len(), "размеры изображений");
    let mut worst = (0.0_f32, (0, 0));
    for (index, (got, want)) in gpu.iter().zip(reference).enumerate() {
        for channel in 0..3 {
            let difference = (got[channel] - want[channel]).abs();
            assert!(
                !difference.is_nan(),
                "NaN в пикселе {index}: GPU {got:?}, эталон {want:?}"
            );
            if difference > worst.0 {
                worst = (difference, ((index as u32) % width, (index as u32) / width));
            }
        }
    }
    worst
}

/// Простой детерминированный генератор (xorshift32) для шума тестов.
pub struct NoiseSource(u32);

impl NoiseSource {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    /// Равномерно в [0, 1).
    pub fn uniform(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Нормальный N(0, 1) (Бокс — Мюллер).
    pub fn gaussian(&mut self) -> f32 {
        let first = self.uniform().max(1e-7);
        let second = self.uniform();
        (-2.0 * first.ln()).sqrt() * (2.0 * std::f32::consts::PI * second).cos()
    }
}

pub fn workload_config() -> VideoHeadroomConfig {
    VideoHeadroomConfig {
        frame_slots: 1,
        output_format: QualityOutputFormat::HalfFloat,
        temporal_denoise: TemporalDenoiseConfig {
            enabled: false,
            ..TemporalDenoiseConfig::default()
        },
        ..VideoHeadroomConfig::default()
    }
}

pub fn sink(context: &GpuContext, width: u32, height: u32) -> OffscreenFrameSink {
    OffscreenFrameSink::new(
        context,
        &OffscreenFrameSinkDesc {
            extent: extent(width, height),
            format_policy: QualityOutputFormat::HalfFloat.storage_policy(),
            frame_slots: 1,
            reader: ConsumerStage::Compute,
        },
    )
    .unwrap()
}

/// Записывает и выполняет один кадр; возвращает (линейный RGB, сигнал экрана) с GPU.
pub fn run_gpu(
    context: &GpuContext,
    source: &MosaicSource,
    params: &IspParams,
    valid_area: ValidArea,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let size = extent(valid_area.width, valid_area.height);
    let mut workload = VideoHeadroomWorkload::new(context, size, &workload_config()).unwrap();
    let mut frame_sink = sink(context, size.width, size.height);
    let input = VideoFrameInput {
        source,
        params,
        valid_area,
        stream: StreamGeneration(1),
        sequence: 1,
        presentation_time_ns: 0,
    };
    let mut outcome = None;
    run_one_shot_blocking(context, |command_buffer| {
        outcome = Some(
            workload
                .record(command_buffer, 0, &input, &mut frame_sink)
                .map_err(|error| match error {
                    QualityError::Gpu(gpu) => gpu,
                    other => panic!("запись кадра: {other}"),
                })?,
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(outcome, Some(HeadroomRecordOutcome::Recorded { history: None }));
    frame_sink.after_submit(0).unwrap();
    let linear = read_rgb(workload.planes().linear_rgb());
    let image = frame_sink.image(0).unwrap();
    let display = read_storage_image_rgba(image, frame_sink.last_use(0).unwrap())
        .unwrap()
        .into_iter()
        .map(|pixel| [pixel[0], pixel[1], pixel[2]])
        .collect();
    (linear, display)
}

/// Кадр-источник больше полезной области: полезная область с отступом 2/2 (проверка адресации).
pub fn padded_frame(raw: &RawMosaic, padding_value: u16) -> (Vec<u16>, u32, u32) {
    let (width, height) = (raw.size().width, raw.size().height);
    let (frame_width, frame_height) = (width + 4, height + 4);
    let mut frame = vec![padding_value; (frame_width * frame_height) as usize];
    for y in 0..height {
        for x in 0..width {
            frame[((y + 2) * frame_width + x + 2) as usize] = raw.sample(x, y);
        }
    }
    (frame, frame_width, frame_height)
}

pub fn area_of(raw: &RawMosaic) -> ValidArea {
    ValidArea {
        left: 2,
        top: 2,
        width: raw.size().width,
        height: raw.size().height,
    }
}
