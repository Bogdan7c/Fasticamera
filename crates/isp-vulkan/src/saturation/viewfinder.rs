//! [`ViewfinderSaturation`] — слитый проход видоискателя под стендом насыщения.
//!
//! # Чему соответствует в продукте
//! Один кадр работы = то, что `render-loop` записывает на каждый новый кадр камеры
//! (`pipelines.rs::record_new_frame`): приём кадра для compute → `ViewfinderIsp::record` → возврат кадра.
//! Без компоновщика, UI и приборов: они в других работах.
//!
//! # Откуда умолчания (продуктовые)
//! * размер кадра 4096×3072, RAW10 с шагом строки 5120 байт — буфер основной камеры OnePlus 11 (4:3);
//! * кадрирование `ViewfinderCrop::FULL_SENSOR`, ориентация 90° — `ViewfinderCropConfig::default` и паспорт
//!   основной камеры (`camera_contract::fixtures::main_wide_camera`);
//! * набор стадий `IspStageSet::ALL`, конфиг ISP `ViewfinderIspConfig::default` (слоты 2, сетка AWB 32×24);
//! * статистика AWB — каждый 4-й кадр: `WhiteBalanceConfig::statistics_every_n_frames` и расписание
//!   `StatisticsSchedule` (кадры 0, N, 2N…; на остальных — вариант pipeline без `IspStage::AwbStatistics`,
//!   `session/slot.rs::effective_stages`). AE-форсирование холодного старта не повторяется: это не установившийся режим;
//! * охват экрана Display P3; параметры (`IspParams`: калибровка, основа тона, вид, LUT, карта теней) задаёт вызывающий
//!   ([`ProductParamsRecipe`](super::ProductParamsRecipe)).

use ash::vk;
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::viewfinder_crop::ViewfinderCrop;
use gpu_vulkan::backdrop::BackdropFrameNumber;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::saturation::{
    FrameRecording, PassTimeLog, PassTimeSummary, SaturationWorkload, SlotId, WorkloadDigest, digest_storage_image,
};
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_model::params::IspParams;

use super::blocking::run_blocking;
use super::error::{FirstFailure, SaturationWorkError};
use super::scene::{SceneContent, check_raw10_size};
use super::source::RawFrameHolder;
use super::timing::log_collect;
use crate::viewfinder::{
    IspStage, IspStageSet, ViewfinderFrame, ViewfinderGeometry, ViewfinderImage, ViewfinderIsp, ViewfinderIspConfig,
};

/// Размер буфера основной камеры OnePlus 11 (4:3): `Camera2` отдаёт RAW10 4096×3072.
pub const PRODUCT_FRAME_SIZE: PixelSize = PixelSize::new(4096, 3072);
/// Статистика AWB — каждый N-й кадр: `WhiteBalanceConfig::default().statistics_every_n_frames` в `render-loop`.
pub const PRODUCT_AWB_STATISTICS_EVERY_N_FRAMES: u32 = 4;
/// Сколько последних кадров помнит журнал времён проходов (окна замера по ~1 с при ≥ 60 к/с хватает с запасом).
pub const PASS_LOG_CAPACITY_FRAMES: usize = 4096;
/// Метка единственного отрезка ISP (точки `isp-start` → `isp-end` в `ViewfinderIsp`).
const PASS_LABELS: [&str; 1] = ["isp-end"];
/// Слот, на котором идут прогревочные кадры конструктора.
const WARMUP_SLOT: usize = 0;

/// Настройки работы «видоискатель». `Default` = продукт (см. описание модуля).
#[derive(Debug, Clone, Copy)]
pub struct ViewfinderSaturationConfig {
    /// Размер кадра (RAW10): ширина кратна 4, высота кратна 2.
    pub frame_size: PixelSize,
    /// Кадрирование видоискателя.
    pub crop: ViewfinderCrop,
    /// Ориентация сенсора.
    pub sensor_orientation: SensorOrientation,
    /// Цель показа (охват параметров обязан ей соответствовать).
    pub display: DisplayColorSpace,
    /// Набор стадий (продукт — все).
    pub stages: IspStageSet,
    /// Конфиг ISP (слоты, сетка AWB, формат выхода, прогрев основ тона...).
    pub isp: ViewfinderIspConfig,
    /// Статистика AWB на каждом N-м кадре (≥ 1; первый кадр — со статистикой).
    pub awb_statistics_every_n_frames: u32,
    /// Содержимое синтетического кадра.
    pub content: SceneContent,
    /// Кто читает картинку после ISP: компоновщик продукта — `Fragment`; приборы — `Compute`.
    pub reader: ConsumerStage,
    /// Паддинг конца строки RAW10, байты (камера OnePlus 11 — 0: 4096 пикселей = ровно 5120 байт).
    pub raw10_row_padding_bytes: u32,
}

impl Default for ViewfinderSaturationConfig {
    fn default() -> Self {
        Self {
            frame_size: PRODUCT_FRAME_SIZE,
            crop: ViewfinderCrop::FULL_SENSOR,
            sensor_orientation: SensorOrientation::Degrees90,
            display: DisplayColorSpace::DisplayP3,
            stages: IspStageSet::ALL,
            isp: ViewfinderIspConfig::default(),
            awb_statistics_every_n_frames: PRODUCT_AWB_STATISTICS_EVERY_N_FRAMES,
            content: SceneContent::default(),
            reader: ConsumerStage::Fragment,
            raw10_row_padding_bytes: 0,
        }
    }
}

impl ViewfinderSaturationConfig {
    /// Проверка без GPU: ничего не создаётся, пока она не пройдена.
    pub fn validate(&self) -> Result<(), SaturationWorkError> {
        check_raw10_size(self.frame_size)?;
        if self.awb_statistics_every_n_frames == 0 {
            return Err(SaturationWorkError::invalid_config(
                "awb_statistics_every_n_frames = 0: «каждый 0-й кадр» не бывает (1 — на каждом)",
            ));
        }
        self.isp.validate()?;
        Ok(())
    }

    /// Нужна ли статистика AWB на кадре с номером `frame_index` (расписание продукта после `restart()`).
    pub fn statistics_due(&self, frame_index: u64) -> bool {
        frame_index.is_multiple_of(u64::from(self.awb_statistics_every_n_frames))
    }
}

/// Счётчики работы: наблюдаемый итог расписания статистики.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ViewfinderCounters {
    /// Кадров записано (отправлено стенду).
    pub frames_recorded: u64,
    /// Из них с включённой стадией статистики AWB.
    pub statistics_recorded: u64,
    /// Кадров освобождено после fence.
    pub frames_released: u64,
    /// Кадров, у которых после fence реально забрана статистика AWB (`collect` вернул `Some`).
    pub statistics_collected: u64,
    /// Был ли ПОСЛЕДНИЙ записанный кадр кадром со статистикой AWB (`None` — кадров ещё нет). Нужен дайджесту:
    /// картинка последнего кадра — это и есть то, что он читает; равенство дайджестов «со статистикой» и «без»
    /// на одном компиляторе шейдеров не переносится на другой, поэтому вариант кадра пишется в результат.
    pub last_frame_had_statistics: Option<bool>,
}

/// Работа «слитый проход видоискателя» (см. описание модуля).
pub struct ViewfinderSaturation {
    isp: ViewfinderIsp,
    params: IspParams,
    geometry: ViewfinderGeometry,
    config: ViewfinderSaturationConfig,
    // Источник и его владелец: поле ПОСЛЕ isp — уничтожается позже (GPU уже не читает).
    source: MosaicSource,
    _holder: Box<dyn RawFrameHolder>,
    context: GpuContext,
    times: PassTimeLog,
    counters: ViewfinderCounters,
    failure: FirstFailure,
}

impl ViewfinderSaturation {
    /// Создаёт работу и прогревает варианты pipeline (с статистикой и без) блокирующими кадрами, чтобы в
    /// цикле стенда ничего не создавалось. `holder` держит RAW-кадр размера `config.frame_size`.
    pub fn new(
        context: &GpuContext,
        config: ViewfinderSaturationConfig,
        params: IspParams,
        holder: Box<dyn RawFrameHolder>,
    ) -> Result<Self, SaturationWorkError> {
        config.validate()?;
        let source = holder.source();
        check_source_size(&source, config.frame_size)?;
        let valid_area = PixelRect {
            left: 0,
            top: 0,
            size: config.frame_size,
        };
        let geometry = ViewfinderGeometry::from_crop(valid_area, &config.crop, config.sensor_orientation)?;
        let mut work = Self {
            isp: ViewfinderIsp::new(context, &config.isp)?,
            params,
            geometry,
            config,
            source,
            _holder: holder,
            context: context.clone(),
            times: PassTimeLog::new(&PASS_LABELS, PASS_LOG_CAPACITY_FRAMES),
            counters: ViewfinderCounters::default(),
            failure: FirstFailure::default(),
        };
        work.warm_up()?;
        Ok(work)
    }

    /// Блокирующие кадры конструктора: создают изображение и оба варианта pipeline.
    fn warm_up(&mut self) -> Result<(), SaturationWorkError> {
        let mut variants = vec![self.config.stages];
        if self.config.awb_statistics_every_n_frames > 1 {
            variants.push(self.config.stages.without(IspStage::AwbStatistics));
        }
        for stages in variants {
            let (context, isp) = (&self.context, &mut self.isp);
            let (source, params, geometry, config) = (&self.source, &self.params, self.geometry, &self.config);
            run_blocking(context, |command_buffer| {
                record_isp_frame(
                    context,
                    isp,
                    command_buffer,
                    source,
                    params,
                    geometry,
                    config,
                    stages,
                    WARMUP_SLOT,
                    0,
                )
                .map_err(|error| error.to_gpu_error())
            })?;
            self.isp.collect(WARMUP_SLOT)?;
        }
        Ok(())
    }

    /// Сколько слотов кадров у работы: `frames_in_flight` стенда не должно быть больше.
    pub fn slots(&self) -> usize {
        self.isp.frame_slots()
    }

    /// Настройки работы.
    pub fn config(&self) -> &ViewfinderSaturationConfig {
        &self.config
    }

    /// Счётчики за время жизни работы (прогрев конструктора не входит).
    pub fn counters(&self) -> ViewfinderCounters {
        self.counters
    }

    /// Итог по проходам: единственный проход `isp-end`.
    pub fn pass_times(&self) -> PassTimeSummary {
        self.times.summary()
    }

    /// Обнуляет журнал времён и счётчики (например, после пробного прогона).
    pub fn reset_statistics(&mut self) {
        self.times.clear();
        self.counters = ViewfinderCounters::default();
    }

    /// Забирает первую ошибку, случившуюся внутри `record_frame`/`release_slot_after_fence`.
    pub fn take_failure(&mut self) -> Option<SaturationWorkError> {
        self.failure.take()
    }

    /// Картинка видоискателя (последняя записанная), для приборов и сверки.
    pub fn image(&self) -> Option<&ViewfinderImage> {
        self.isp.image()
    }

    /// ISP (чтение: диагностика, счётчики вариантов).
    pub fn isp(&self) -> &ViewfinderIsp {
        &self.isp
    }

    fn record_inner(&mut self, recording: FrameRecording) -> Result<(), SaturationWorkError> {
        let slot = recording.slot().index();
        let slots = self.isp.frame_slots();
        if slot >= slots {
            return Err(SaturationWorkError::SlotOutOfRange { slot, slots });
        }
        let frame_index = recording.frame_index();
        let statistics_due = self.config.statistics_due(frame_index);
        let stages = if statistics_due {
            self.config.stages
        } else {
            self.config.stages.without(IspStage::AwbStatistics)
        };
        record_isp_frame(
            &self.context,
            &mut self.isp,
            recording.command_buffer(),
            &self.source,
            &self.params,
            self.geometry,
            &self.config,
            stages,
            slot,
            frame_index,
        )?;
        self.counters.frames_recorded += 1;
        let has_statistics = stages.contains(IspStage::AwbStatistics);
        if has_statistics {
            self.counters.statistics_recorded += 1;
        }
        self.counters.last_frame_had_statistics = Some(has_statistics);
        Ok(())
    }

    fn release_inner(&mut self, slot: SlotId) -> Result<(), SaturationWorkError> {
        let report = self.isp.collect(slot.index())?;
        self.counters.frames_released += 1;
        if report.awb_statistics.is_some() {
            self.counters.statistics_collected += 1;
        }
        log_collect(&mut self.times, &report.isp_gpu_time, &PASS_LABELS);
        Ok(())
    }
}

impl SaturationWorkload for ViewfinderSaturation {
    fn record_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError> {
        self.record_inner(recording).map_err(|error| self.failure.note(error))
    }

    fn release_slot_after_fence(&mut self, slot: SlotId) {
        if let Err(error) = self.release_inner(slot) {
            // Контракт стенда не принимает ошибку из этого метода: запоминаем для `take_failure`.
            self.failure.note(error);
        }
    }
}

impl ViewfinderSaturation {
    /// Дайджест картинки ISP последнего записанного кадра (только содержимое, без хвоста ёмкости).
    ///
    /// Звать ПОСЛЕ конца прогона стенда и `wait_idle` (все fence пройдены): чтение блокирующее и вне окна замера.
    /// Работает без `test-support`. Раскладка картинки после чтения прежняя.
    pub fn output_digest(&self) -> Result<WorkloadDigest, SaturationWorkError> {
        let image = self.isp.image().ok_or_else(|| SaturationWorkError::EmptyFrame {
            what: "картинка видоискателя ещё не создана".to_owned(),
        })?;
        let digest = digest_storage_image(image.storage_image(), image.current_use(), image.content_extent())?;
        Ok(WorkloadDigest::single("image", digest))
    }
}

/// Размер буфера источника совпал с настройкой (иначе шаг строки и кропы были бы про другой кадр).
pub(crate) fn check_source_size(source: &MosaicSource, expected: PixelSize) -> Result<(), SaturationWorkError> {
    let (width, height) = match *source {
        MosaicSource::SampledR16 { width, height, .. } | MosaicSource::PackedRaw10 { width, height, .. } => {
            (width, height)
        }
    };
    if (width, height) != (expected.width, expected.height) {
        return Err(SaturationWorkError::invalid_config(format!(
            "источник RAW {width}x{height} не совпадает с frame_size {}x{}",
            expected.width, expected.height
        )));
    }
    Ok(())
}

/// Команды одного кадра, как у продукта: приём кадра → ISP → возврат кадра. Общая для кадров стенда и прогрева.
#[expect(
    clippy::too_many_arguments,
    reason = "один набор независимых входов кадра; структура только прибавила бы слой"
)]
fn record_isp_frame(
    context: &GpuContext,
    isp: &mut ViewfinderIsp,
    command_buffer: vk::CommandBuffer,
    source: &MosaicSource,
    params: &IspParams,
    geometry: ViewfinderGeometry,
    config: &ViewfinderSaturationConfig,
    stages: IspStageSet,
    slot: usize,
    frame_index: u64,
) -> Result<(), SaturationWorkError> {
    source.record_acquire_for(context, command_buffer, ConsumerStage::Compute);
    let recorded = isp.record(
        command_buffer,
        &ViewfinderFrame {
            source,
            params,
            geometry,
            display_color_space: config.display,
            stages,
            frame: BackdropFrameNumber(frame_index + 1),
            frame_slot: slot,
            reader: config.reader,
        },
    );
    // Возврат кадра записан всегда (как в продукте): без него следующий приём начался бы из неверного состояния.
    source.record_release_for(context, command_buffer, ConsumerStage::Compute);
    recorded?;
    Ok(())
}

impl ViewfinderSaturation {
    /// Работа на синтетическом RAW10 `config.frame_size` с содержимым `config.content` (ПК, фича `test-support`).
    pub fn new_synthetic(
        context: &GpuContext,
        config: ViewfinderSaturationConfig,
        params: IspParams,
    ) -> Result<Self, SaturationWorkError> {
        config.validate()?;
        let frame = super::scene::synthesize_raw10(config.frame_size, config.content, config.raw10_row_padding_bytes)?;
        let holder = super::source::synthetic_raw10_holder(context, &frame)?;
        Self::new(context, config, params, holder)
    }
}

#[cfg(feature = "test-support")]
impl ViewfinderSaturation {
    /// Содержимое последней записанной картинки (RGBA, построчно) — блокирующее чтение для ПК-тестов.
    /// Звать после того, как стенд закончил прогон (все fence пройдены).
    pub fn read_output_blocking(&self) -> Result<(PixelSize, Vec<[f32; 4]>), SaturationWorkError> {
        use gpu_vulkan::kit_test_support::read_storage_image_rgba;
        let image = self.isp.image().ok_or_else(|| SaturationWorkError::EmptyFrame {
            what: "картинка видоискателя ещё не создана".to_owned(),
        })?;
        let whole = read_storage_image_rgba(image.storage_image(), image.current_use())?;
        let (capacity, content) = (image.extent(), image.content_extent());
        let pixels = (0..content.height)
            .flat_map(|y| (0..content.width).map(move |x| (y * capacity.width + x) as usize))
            .map(|index| whole[index])
            .collect();
        Ok((PixelSize::new(content.width, content.height), pixels))
    }
}
