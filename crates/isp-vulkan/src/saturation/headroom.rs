//! [`HeadroomSaturation`] — цепочка стенда V4 (`VideoHeadroomWorkload`) под стендом насыщения.
//!
//! # Чему соответствует
//! Один кадр работы = один кадр бенча «запас под видео» (`render-loop/.../headroom_bench.rs`): приём RAW для
//! compute → `VideoHeadroomWorkload::record` (уровни → 8 проходов Menon 2007 → [шумодав] → вывод) → возврат RAW.
//! Приёмник кадра — `OffscreenFrameSink` (картинка на слот). Это цена будущего видео-ISP на кадре 4096×2304.
//!
//! # Откуда умолчания
//! * кадр 4096×2304 (16:9, основная камера на 60 к/с) — `VideoHeadroomWorkload::new` («на телефоне — 4096×2304»);
//! * временной шумодав ВЫКЛЮЧЕН (решение владельца 169: прототип шумодава не входит в цену по умолчанию);
//!   включается параметром [`HeadroomSaturationConfig::with_temporal_denoise_reference`] только «справочно»;
//! * остальное — `VideoHeadroomConfig::default()` (2 слота, 16×16, формат вывода 10 бит UNORM).
//!
//! # Приёмник и публикация
//! `FrameSink::after_submit` вызывается из `release_slot_after_fence`: стенд владеет отправкой, а у
//! `OffscreenFrameSink` это только учёт публикации (семафоров нет), поэтому порядок безопасен.

use camera_contract::geometry::PixelSize;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::frame_sink::{FrameSink, FrameSinkPublish, OffscreenFrameSink, OffscreenFrameSinkDesc};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, ValidArea};
use gpu_vulkan::saturation::{FrameRecording, PassTimeLog, PassTimeSummary, SaturationWorkload, SlotId};
use isp_model::params::IspParams;
use std::time::Duration;

use super::error::{FirstFailure, SaturationWorkError};
use super::scene::{SceneContent, check_raw10_size};
use super::source::RawFrameHolder;
use super::viewfinder::{PASS_LOG_CAPACITY_FRAMES, check_source_size};
use crate::quality::{
    HeadroomPass, HeadroomRecordOutcome, HeadroomTimingCollect, StreamGeneration, TemporalDenoiseConfig,
    VideoFrameInput, VideoHeadroomConfig, VideoHeadroomWorkload,
};

/// Кадр бенча V4: основная камера 16:9 на 60 к/с.
pub const HEADROOM_FRAME_SIZE: PixelSize = PixelSize::new(4096, 2304);
/// Длительность кадра при 60 к/с, нс: метка времени кадра для приёмника (`FrameSinkStamp`).
const FRAME_PERIOD_NS_AT_60_FPS: u64 = 16_666_667;

/// Настройки работы «стенд V4».
#[derive(Debug, Clone, Copy)]
pub struct HeadroomSaturationConfig {
    /// Размер кадра (RAW10): ширина кратна 4, высота кратна 2.
    pub frame_size: PixelSize,
    /// Настройки стенда V4 (по умолчанию — продуктовые, но без шумодава).
    pub headroom: VideoHeadroomConfig,
    /// Содержимое синтетического кадра.
    pub content: SceneContent,
    /// Паддинг конца строки RAW10, байты.
    pub raw10_row_padding_bytes: u32,
}

impl Default for HeadroomSaturationConfig {
    fn default() -> Self {
        Self {
            frame_size: HEADROOM_FRAME_SIZE,
            headroom: VideoHeadroomConfig {
                temporal_denoise: TemporalDenoiseConfig {
                    enabled: false,
                    ..TemporalDenoiseConfig::default()
                },
                ..VideoHeadroomConfig::default()
            },
            content: SceneContent::default(),
            raw10_row_padding_bytes: 0,
        }
    }
}

impl HeadroomSaturationConfig {
    /// Справочный вариант с прототипом временного шумодава (решение 169: не входит в цену по умолчанию).
    #[must_use]
    pub fn with_temporal_denoise_reference(mut self) -> Self {
        self.headroom.temporal_denoise.enabled = true;
        self
    }

    /// Проверка без GPU.
    pub fn validate(&self) -> Result<(), SaturationWorkError> {
        check_raw10_size(self.frame_size)?;
        self.headroom.validate()?;
        Ok(())
    }

    /// Метки проходов в порядке записи: шумодав выключен — проходов 10, включён — 11.
    pub fn pass_labels(&self) -> Vec<&'static str> {
        HeadroomPass::ORDER
            .iter()
            .filter(|pass| self.headroom.temporal_denoise.enabled || **pass != HeadroomPass::TemporalDenoise)
            .map(|pass| pass.label())
            .collect()
    }
}

/// Счётчики работы.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HeadroomCounters {
    /// Кадров записано.
    pub frames_recorded: u64,
    /// Кадров освобождено после fence.
    pub frames_released: u64,
    /// Кадров, опубликованных приёмником (`after_submit` → `Published`).
    pub frames_published: u64,
}

/// Работа «стенд V4» (см. описание модуля).
pub struct HeadroomSaturation {
    // Порядок полей = порядок уничтожения: работа и приёмник раньше источника.
    workload: VideoHeadroomWorkload,
    sink: OffscreenFrameSink,
    params: IspParams,
    config: HeadroomSaturationConfig,
    source: MosaicSource,
    _holder: Box<dyn RawFrameHolder>,
    context: GpuContext,
    labels: Vec<&'static str>,
    times: PassTimeLog,
    counters: HeadroomCounters,
    /// Слот последнего записанного кадра: из его картинки читают результат в тестах.
    last_recorded_slot: Option<usize>,
    failure: FirstFailure,
}

impl HeadroomSaturation {
    /// Создаёт работу. `params` — параметры `Quality` ([`ProductParamsRecipe::quality`](super::ProductParamsRecipe)).
    pub fn new(
        context: &GpuContext,
        config: HeadroomSaturationConfig,
        params: IspParams,
        holder: Box<dyn RawFrameHolder>,
    ) -> Result<Self, SaturationWorkError> {
        config.validate()?;
        let source = holder.source();
        check_source_size(&source, config.frame_size)?;
        let extent = ash::vk::Extent2D {
            width: config.frame_size.width,
            height: config.frame_size.height,
        };
        let workload = VideoHeadroomWorkload::new(context, extent, &config.headroom)?;
        let sink = OffscreenFrameSink::new(
            context,
            &OffscreenFrameSinkDesc {
                extent,
                format_policy: config.headroom.output_format.storage_policy(),
                frame_slots: config.headroom.frame_slots,
                reader: ConsumerStage::Fragment,
            },
        )?;
        let labels = config.pass_labels();
        Ok(Self {
            times: PassTimeLog::new(&labels, PASS_LOG_CAPACITY_FRAMES),
            workload,
            sink,
            params,
            config,
            source,
            _holder: holder,
            context: context.clone(),
            labels,
            counters: HeadroomCounters::default(),
            last_recorded_slot: None,
            failure: FirstFailure::default(),
        })
    }

    /// Метки проходов, под которые создан журнал.
    pub fn labels(&self) -> &[&'static str] {
        &self.labels
    }

    /// Сколько слотов кадров у работы.
    pub fn slots(&self) -> usize {
        self.config.headroom.frame_slots
    }

    /// Настройки.
    pub fn config(&self) -> &HeadroomSaturationConfig {
        &self.config
    }

    /// Счётчики.
    pub fn counters(&self) -> HeadroomCounters {
        self.counters
    }

    /// Итог по проходам (метки — [`HeadroomSaturationConfig::pass_labels`]).
    pub fn pass_times(&self) -> PassTimeSummary {
        self.times.summary()
    }

    /// Обнуляет журнал времён и счётчики.
    pub fn reset_statistics(&mut self) {
        self.times.clear();
        self.counters = HeadroomCounters::default();
    }

    /// Первая ошибка, случившаяся внутри `record_frame`/`release_slot_after_fence`.
    pub fn take_failure(&mut self) -> Option<SaturationWorkError> {
        self.failure.take()
    }

    fn record_inner(&mut self, recording: FrameRecording) -> Result<(), SaturationWorkError> {
        let slot = recording.slot().index();
        let slots = self.slots();
        if slot >= slots {
            return Err(SaturationWorkError::SlotOutOfRange { slot, slots });
        }
        let frame_index = recording.frame_index();
        let command_buffer = recording.command_buffer();
        let input = VideoFrameInput {
            source: &self.source,
            params: &self.params,
            valid_area: ValidArea {
                left: 0,
                top: 0,
                width: self.config.frame_size.width,
                height: self.config.frame_size.height,
            },
            stream: StreamGeneration(1),
            sequence: frame_index,
            presentation_time_ns: frame_index * FRAME_PERIOD_NS_AT_60_FPS,
        };
        self.source
            .record_acquire_for(&self.context, command_buffer, ConsumerStage::Compute);
        let outcome = self.workload.record(command_buffer, slot, &input, &mut self.sink);
        // Возврат кадра записан всегда (как у бенча продукта).
        self.source
            .record_release_for(&self.context, command_buffer, ConsumerStage::Compute);
        match outcome? {
            HeadroomRecordOutcome::Recorded { .. } => {
                self.counters.frames_recorded += 1;
                self.last_recorded_slot = Some(slot);
                Ok(())
            }
            // Пустой кадр нельзя засчитывать в пропускную способность: стенд остановится с ошибкой.
            HeadroomRecordOutcome::SkippedSinkBusy => Err(SaturationWorkError::EmptyFrame {
                what: "приёмник кадра занят (SkippedSinkBusy): в командный буфер ничего не записано".to_owned(),
            }),
        }
    }

    fn release_inner(&mut self, slot: SlotId) -> Result<(), SaturationWorkError> {
        let index = slot.index();
        // Порядок: сначала публикация в приёмнике (учёт), потом сбор меток, но обе операции выполняются всегда.
        let published = self.sink.after_submit(index);
        let collected = self.workload.collect(index);
        self.counters.frames_released += 1;
        match collected? {
            HeadroomTimingCollect::Ready(timings) => {
                let durations: Vec<Duration> = timings
                    .passes
                    .iter()
                    .map(|timing| Duration::from_nanos(timing.duration_ns))
                    .collect();
                self.times.record_frame(&durations);
            }
            HeadroomTimingCollect::NotReady
            | HeadroomTimingCollect::NothingRecorded
            | HeadroomTimingCollect::Unsupported => self.times.record_unmeasured_frame(),
        }
        if matches!(published?, FrameSinkPublish::Published(_)) {
            self.counters.frames_published += 1;
        }
        Ok(())
    }
}

impl SaturationWorkload for HeadroomSaturation {
    fn record_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError> {
        self.record_inner(recording).map_err(|error| self.failure.note(error))
    }

    fn release_slot_after_fence(&mut self, slot: SlotId) {
        if let Err(error) = self.release_inner(slot) {
            self.failure.note(error);
        }
    }
}

impl HeadroomSaturation {
    /// Работа на синтетическом RAW10 (ПК, фича `test-support`).
    pub fn new_synthetic(
        context: &GpuContext,
        config: HeadroomSaturationConfig,
        params: IspParams,
    ) -> Result<Self, SaturationWorkError> {
        config.validate()?;
        let frame = super::scene::synthesize_raw10(config.frame_size, config.content, config.raw10_row_padding_bytes)?;
        let holder = super::source::synthetic_raw10_holder(context, &frame)?;
        Self::new(context, config, params, holder)
    }
}

#[cfg(feature = "test-support")]
impl HeadroomSaturation {
    /// Картинка последнего записанного кадра (RGBA построчно) — блокирующее чтение, после конца прогона.
    pub fn read_last_output_blocking(&self) -> Result<Vec<[f32; 4]>, SaturationWorkError> {
        use gpu_vulkan::kit_test_support::read_storage_image_rgba;
        let missing = || SaturationWorkError::EmptyFrame {
            what: "ни один кадр ещё не записан".to_owned(),
        };
        let slot = self.last_recorded_slot.ok_or_else(missing)?;
        let image = self.sink.image(slot).ok_or_else(missing)?;
        let last_use = self.sink.last_use(slot).ok_or_else(missing)?;
        Ok(read_storage_image_rgba(image, last_use)?)
    }
}
