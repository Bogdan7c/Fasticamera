//! Сама линейка: кадр → GPU (видоискатель) в двух форматах хранения → сверка с CPU-эталоном `isp-model`.
//!
//! # Что делает линейка
//! Для синтетического RAW-кадра и готовых `IspParams` она
//! 1. считает эталон на CPU (`run_reference_encoded`, любая основа тона), кадр целиком, без поворота;
//! 2. гонит тот же кадр через [`ViewfinderIsp`] дважды: формат «только fp16» и формат, который просит продукт
//!    («10 бит, если устройство умеет, иначе fp16»);
//! 3. возвращает числа [`ParityMeasure`] (без вердикта: вердикт — за вызывающим, у каждого случая свой допуск).
//!
//! Один `ViewfinderIsp` на формат живёт, пока жива линейка: компиляция варианта шейдера на Adreno стоит 145–180 мс,
//! и пересоздавать её на каждый кадр линейки незачем. Источник RAW — обычная память Vulkan ([`HostRawFrame`]), без
//! тестовых фич: линейка работает и в release на телефоне.

use camera_contract::geometry::SensorOrientation;
use gpu_vulkan::backdrop::BackdropFrameNumber;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::read_storage_image_rgba;
use gpu_vulkan::raw_mosaic_preview::reference::pack_raw10;
use gpu_vulkan::saturation::{HostRawFrame, RawFrameError};
use gpu_vulkan::storage_image::StorageImageFormat;
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_model::params::IspParams;
use isp_model::pipeline::run_reference_encoded;
use isp_model::statistics::{AwbStatisticsSettings, TileGrid};

use super::allowance::{ErrorAllowance, HALF_LSB};
use super::frame::SyntheticRawFrame;
use super::metrics::{Parity, compare_with_reference};
use crate::saturation::blocking::run_blocking;
use crate::viewfinder::{
    IspStageSet, LargestMosaicConfig, OutputAllocation, VariantPrewarm, ViewfinderError, ViewfinderFrame,
    ViewfinderGeometry, ViewfinderIsp, ViewfinderIspConfig, ViewfinderStorageFormat,
};

/// Наибольший кадр линейки: суммы AWB считаются на нём без переполнения u32 даже на сетке 4×3.
const LARGEST_FRAME: LargestMosaicConfig = LargestMosaicConfig {
    width: 256,
    height: 192,
};
/// Сетка статистики AWB линейки: влезает и в маленькие кадры синтетики.
const AWB_GRID: TileGrid = TileGrid { columns: 4, rows: 3 };
/// Наибольшее значение отсчёта RAW10.
const RAW10_MAX_SAMPLE: u16 = 1023;
/// Заполнение паддинга строки (если шейдер его прочтёт, паритет это покажет).
const PADDING_FILL_BYTE: u8 = 0xA5;
/// Слот, в который идут кадры линейки (кадры строго по одному, с ожиданием).
const RULER_SLOT: usize = 0;

/// Почему линейка не смогла измерить.
#[derive(Debug, thiserror::Error)]
pub enum ParityError {
    /// Ошибка GPU (создание, отправка, ожидание, чтение).
    #[error("GPU: {0}")]
    Gpu(#[from] GpuError),
    /// ISP отклонил кадр или параметры.
    #[error("ISP: {0}")]
    Isp(#[from] ViewfinderError),
    /// Не удалось заполнить буфер RAW-кадра.
    #[error("буфер RAW-кадра: {0}")]
    RawFrame(#[from] RawFrameError),
    /// Картинка GPU и эталон разного размера: числа были бы про разные картинки.
    #[error("размер картинки GPU {gpu:?} и эталона {reference:?} разный")]
    SizeMismatch {
        /// Размер картинки GPU.
        gpu: (u32, u32),
        /// Размер эталона.
        reference: (u32, u32),
    },
    /// Кадр не годится для RAW10 или больше предела линейки.
    #[error("кадр {width}x{height}: {reason}")]
    UnsupportedFrame {
        /// Ширина кадра.
        width: u32,
        /// Высота кадра.
        height: u32,
        /// Что именно не так.
        reason: &'static str,
    },
    /// Эталон `isp-model` не смог обработать кадр.
    #[error("эталон isp-model: {0}")]
    Reference(String),
    /// Режим «только fp16» вернул картинку не в fp16: числа ушли бы под чужим шагом хранения.
    #[error("режим «только fp16» дал формат {0:?}")]
    HalfFloatModeGaveOtherFormat(StorageImageFormat),
    /// После кадра у ISP нет картинки (внутренняя ошибка порядка вызовов).
    #[error("после кадра у ISP нет картинки")]
    NoImage,
}

/// Итог сверки одного кадра с эталоном в двух форматах хранения: числа, без вердикта.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParityMeasure {
    /// fp16-выход.
    pub half: Parity,
    /// Выход формата «10 бит, если устройство умеет» — то, что просит продукт.
    pub ten_bit: Parity,
    /// Формат, который в этом режиме РЕАЛЬНО получен (на устройстве без записи rgb10a2 это fp16).
    pub ten_bit_format: StorageImageFormat,
}

impl ParityMeasure {
    /// Формат «10 бит» действительно получен (а не запасной fp16).
    pub fn ten_bit_format_is_unorm(&self) -> bool {
        self.ten_bit_format == StorageImageFormat::A2B10G10R10Unorm
    }

    /// Нарушения допусков по числам (пусто — кадр прошёл): ошибка математики сверх шага хранения не больше
    /// `allowance`, max |Δ| ≤ полшага 10 бит + то же, ноль пикселей хуже 1 LSB на fp16. Формат хранения не судится:
    /// на устройстве без rgb10a2 запасной формат — законный исход (см. [`Self::violations`]).
    pub fn math_violations(&self, allowance: ErrorAllowance) -> Vec<String> {
        let tolerance = allowance.math_excess();
        let mut found = Vec::new();
        for (label, parity) in [("fp16", &self.half), ("10 бит", &self.ten_bit)] {
            if parity.math_excess > tolerance {
                found.push(format!(
                    "{label}: ошибка математики {:.3e} > {tolerance:e}",
                    parity.math_excess
                ));
            }
            if parity.max_abs > HALF_LSB + tolerance {
                found.push(format!("{label}: max |Δ| {:.3e} > ½ LSB + допуск", parity.max_abs));
            }
        }
        if self.half.over_one_lsb != 0.0 {
            found.push(format!("fp16: доля хуже 1 LSB {}", self.half.over_one_lsb));
        }
        found
    }

    /// Нарушения допусков паритета строго: [`Self::math_violations`] плюс «формат 10 бит обязан быть rgb10a2»
    /// (на ПК RADV он есть, и тесты ловят его пропажу).
    pub fn violations(&self, allowance: ErrorAllowance) -> Vec<String> {
        let mut found = self.math_violations(allowance);
        if !self.ten_bit_format_is_unorm() {
            found.push("10 бит: формат не rgb10_a2".to_owned());
        }
        found
    }

    /// Наибольшая ошибка математики сверх шага хранения по двум форматам.
    pub fn worst_math_excess(&self) -> f32 {
        self.half.math_excess.max(self.ten_bit.math_excess)
    }

    /// Наибольший max |Δ| по двум форматам.
    pub fn worst_max_abs(&self) -> f32 {
        self.half.max_abs.max(self.ten_bit.max_abs)
    }
}

/// Картинка ISP, прочитанная с GPU.
struct GpuPicture {
    width: u32,
    height: u32,
    format: StorageImageFormat,
    pixels: Vec<[f32; 4]>,
}

/// Линейка паритета: два ISP (по формату хранения) и счётчик кадров.
pub struct ParityRuler {
    context: GpuContext,
    half_float: ViewfinderIsp,
    ten_bit_preferred: ViewfinderIsp,
    frames_recorded: u64,
}

impl ParityRuler {
    /// Создаёт линейку: оба ISP на заданном контексте GPU.
    pub fn new(context: &GpuContext) -> Result<Self, ParityError> {
        Ok(Self {
            context: context.clone(),
            half_float: ViewfinderIsp::new(context, &isp_config(ViewfinderStorageFormat::HalfFloatOnly))?,
            ten_bit_preferred: ViewfinderIsp::new(context, &isp_config(ViewfinderStorageFormat::PreferTenBitUnorm))?,
            frames_recorded: 0,
        })
    }

    /// Измеряет паритет кадра `frame` при параметрах `params` и цели показа `display`.
    pub fn measure(
        &mut self,
        frame: &SyntheticRawFrame,
        params: &IspParams,
        display: DisplayColorSpace,
    ) -> Result<ParityMeasure, ParityError> {
        check_frame(frame)?;
        let reference = reference_pixels(frame, params)?;
        let half = self.picture_of(Which::HalfFloat, frame, params, display)?;
        if half.format != StorageImageFormat::Rgba16Float {
            return Err(ParityError::HalfFloatModeGaveOtherFormat(half.format));
        }
        let ten_bit = self.picture_of(Which::TenBitPreferred, frame, params, display)?;
        let compare = |picture: &GpuPicture| {
            compare_with_reference(
                &picture.pixels,
                (picture.width, picture.height),
                picture.format,
                &reference.pixels,
                (reference.width, reference.height),
            )
        };
        Ok(ParityMeasure {
            half: compare(&half)?,
            ten_bit: compare(&ten_bit)?,
            ten_bit_format: ten_bit.format,
        })
    }

    /// Прогоняет кадр через выбранный ISP одной блокирующей отправкой и читает картинку.
    fn picture_of(
        &mut self,
        which: Which,
        frame: &SyntheticRawFrame,
        params: &IspParams,
        display: DisplayColorSpace,
    ) -> Result<GpuPicture, ParityError> {
        let stride = frame.width / 4 * 5;
        let packed = pack_raw10(&frame.samples, frame.width, stride, PADDING_FILL_BYTE);
        let raw_buffer = HostRawFrame::new(&self.context, frame.width, frame.height, stride, &packed)?;
        let source = raw_buffer.source();
        let geometry = ViewfinderGeometry {
            valid_area: frame.full_rect(),
            crop: frame.full_rect(),
            sensor_orientation: SensorOrientation::Degrees0,
            allocation: OutputAllocation::Exact,
        };
        self.frames_recorded += 1;
        let frame_number = BackdropFrameNumber(self.frames_recorded);
        let context = &self.context;
        let isp = match which {
            Which::HalfFloat => &mut self.half_float,
            Which::TenBitPreferred => &mut self.ten_bit_preferred,
        };
        let mut record_error = None;
        let submitted = run_blocking(context, |command_buffer| {
            source.record_acquire_for(context, command_buffer, ConsumerStage::Compute);
            let recorded = isp.record(
                command_buffer,
                &ViewfinderFrame {
                    source: &source,
                    params,
                    geometry,
                    display_color_space: display,
                    stages: IspStageSet::ALL,
                    frame: frame_number,
                    frame_slot: RULER_SLOT,
                    reader: ConsumerStage::Fragment,
                },
            );
            // Возврат кадра записан всегда (как в продукте): без него следующий приём начался бы из неверного состояния.
            source.record_release_for(context, command_buffer, ConsumerStage::Compute);
            match recorded {
                Ok(_) => Ok(()),
                Err(error) => {
                    record_error = Some(error);
                    Err(GpuError::InvalidState {
                        what: "ISP отклонил кадр линейки (причина — в ParityError::Isp)".to_owned(),
                    })
                }
            }
        });
        if let Some(error) = record_error {
            return Err(ParityError::Isp(error));
        }
        submitted?;
        isp.collect(RULER_SLOT)?;
        let image = isp.image().ok_or(ParityError::NoImage)?;
        let whole = read_storage_image_rgba(image.storage_image(), image.current_use())?;
        // Картинка может быть больше содержимого (ёмкость под зум): берём ровно левый верхний прямоугольник кадра.
        let (capacity, content) = (image.extent(), image.content_extent());
        let pixels = (0..content.height)
            .flat_map(|y| (0..content.width).map(move |x| (y * capacity.width + x) as usize))
            .map(|index| whole[index])
            .collect();
        Ok(GpuPicture {
            width: content.width,
            height: content.height,
            format: image.format(),
            pixels,
        })
    }
}

/// Какой из двух ISP линейки.
#[derive(Debug, Clone, Copy)]
enum Which {
    HalfFloat,
    TenBitPreferred,
}

/// Конфиг ISP линейки: сетка AWB 4×3 (влезает в маленькие кадры), формат по выбору, без заготовки вариантов
/// (линейке нужны ровно те варианты, что она трогает).
fn isp_config(format: ViewfinderStorageFormat) -> ViewfinderIspConfig {
    ViewfinderIspConfig {
        frame_slots: 2,
        awb_statistics: AwbStatisticsSettings {
            grid: AWB_GRID,
            ..AwbStatisticsSettings::default()
        },
        storage_format: format,
        largest_mosaic: LARGEST_FRAME,
        variant_prewarm: VariantPrewarm::Off,
        ..ViewfinderIspConfig::default()
    }
}

/// Кадр пригоден для линейки: RAW10 (ширина кратна 4, высота чётная, отсчёты ≤ 1023) и не больше предела.
fn check_frame(frame: &SyntheticRawFrame) -> Result<(), ParityError> {
    let reason = if frame.width == 0 || frame.height == 0 {
        Some("нулевая сторона")
    } else if !frame.width.is_multiple_of(4) || !frame.height.is_multiple_of(2) {
        Some("RAW10: ширина кратна 4, высота чётная")
    } else if frame.width > LARGEST_FRAME.width || frame.height > LARGEST_FRAME.height {
        Some("больше предела линейки 256x192")
    } else if frame.samples.len() != (frame.width * frame.height) as usize {
        Some("число отсчётов не равно площади")
    } else if frame.samples.iter().any(|sample| *sample > RAW10_MAX_SAMPLE) {
        Some("отсчёт больше 1023: в RAW10 не помещается")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(ParityError::UnsupportedFrame {
            width: frame.width,
            height: frame.height,
            reason,
        }),
        None => Ok(()),
    }
}

/// Эталон в прямой ориентации (без поворота): закодированные пиксели.
struct ReferencePicture {
    width: u32,
    height: u32,
    pixels: Vec<[f32; 3]>,
}

fn reference_pixels(frame: &SyntheticRawFrame, params: &IspParams) -> Result<ReferencePicture, ParityError> {
    let mosaic = frame
        .mosaic(frame.full_rect())
        .map_err(|error| ParityError::Reference(format!("мозаика кадра: {error}")))?;
    let encoded = run_reference_encoded(&mosaic, params).map_err(|error| ParityError::Reference(error.to_string()))?;
    let size = encoded.size();
    let pixels = (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| (x, y)))
        .map(|(x, y)| encoded.pixel(x, y))
        .collect();
    Ok(ReferencePicture {
        width: size.width,
        height: size.height,
        pixels,
    })
}
