//! [`StillSaturation`] — проявление одного снимка целиком (`PhotoDeveloper`) под стендом насыщения.
//!
//! # Чему соответствует в продукте
//! Один кадр работы = одно проявление снимка (`still/vulkan_gpu.rs`): все полосы вывода (уровни → 8 проходов
//! Menon → вывод RGBA8) и завершающий шаг отделки (миниатюра) записаны ПОДРЯД в один командный буфер стенда.
//! Полосы `PhotoDeveloper` не пишут в хост-буферы между собой (параметры — push-константы, плоскости общие,
//! между полосами стоят барьеры `record_compute_pass_dependency`), поэтому одним буфером они записываются так же,
//! как продукт записывает их по одной на кадр видоискателя. Число шагов на снимок —
//! [`StillSaturation::steps_per_still`] (полосы + отделка).
//!
//! # Чего здесь НЕТ и почему (ограничения контракта `photo`)
//! * **Копия RAW (стадия захвата) не входит в кадр.** `RawCapture` существует только после fence захвата
//!   (`complete_after_fence` требует доказательство), поэтому в одном командном буфере с проявлением её быть не
//!   может. Захват делается ОДИН раз в конструкторе; цена копии (один dispatch памяти 24 МиБ) в кадр не входит.
//! * **Доказательство завершения.** `slice_completed`/`finish` требуют `CompletedWork`, которое умеет выдавать
//!   только `OneShotSubmitter::poll`, а fence кадра принадлежит стенду. Мы честно подтверждаем полосы
//!   «маркерной» работой: после fence кадра отправляется пустая работа через свою очередь, и её доказательство
//!   подтверждает всю проявку (кадр к тому моменту завершён: маркер стоит в очереди ПОСЛЕ него). Нужен «намерение-метод»
//!   у владельца (`photo::PhotoDeveloper`): подтверждение проявки внешним fence-ом — см. отчёт пакета.
//! * **Выделения.** `PhotoDeveloper::begin` выделяет картинку снимка, поэтому на `record_frame` его вызывать
//!   нельзя: проявители заводятся с запасом (`2 × слотов + 1`), а `begin` следующего снимка делается при подтверждении
//!   предыдущего (в `release_slot_after_fence`, до того как проявитель понадобится снова).
//! * Времена по проходам меряются метками `photo::timing` (режим [`StillPassTiming::PerPass`]): образцовая полоса
//!   разбирается по проходам, остальные идут двумя отрезками (подробности — в `photo/timing.rs`). Метки собираются
//!   при подтверждении проявки маркером, а не в `release_slot_after_fence`: проявка завершается позже кадра.
//!   Цена кадра — по-прежнему общая пропускная способность стенда, метки лишь делят её по проходам.

use std::sync::Arc;
use std::time::Duration;

use camera_contract::geometry::{PixelRect, PixelSize};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::one_shot_queue::{DrainOutcome, OneShotSubmitter, SubmitError, SubmittedWork, WorkStatus};
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, ValidArea};
use gpu_vulkan::rgba8_image::Rgba8StorageImage;
use gpu_vulkan::saturation::{
    DigestPart, FrameRecording, OutputDigest, PassTimeLog, PassTimeSummary, SaturationWorkload, SlotId, WorkloadDigest,
    digest_rgba8_bytes, digest_rgba8_image,
};
use isp_model::params::IspParams;
use still_contract::shot::StillRotation;

use super::blocking::{PREPARATION_WAIT_TIMEOUT, run_blocking};
use super::error::{FirstFailure, SaturationWorkError};
use super::scene::{SceneContent, check_raw10_size};
use super::source::RawFrameHolder;
use super::viewfinder::check_source_size;
use crate::photo::{
    DevelopRequest, PhotoConfig, PhotoDeveloper, PhotoGpuPoint, PhotoGpuTimingCollect, RawCapture, RawCapturer,
    RgbaReadback, SliceOutcome, ThumbnailOutcome,
};

/// Размер кольца журнала проходов (кадров).
const PASS_LOG_CAPACITY_FRAMES: usize = 512;

/// Режим замера времени GPU по проходам проявки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StillPassTiming {
    /// Метки не пишутся: работа идёт так же, как у продукта.
    Off,
    /// Метки на образцовой полосе и между группами полос (14 меток на снимок).
    #[default]
    PerPass,
}

/// Кадр основной камеры для снимка (4:3, 12 Мпикс): та же сторона, что у видоискателя.
pub const STILL_FRAME_SIZE: PixelSize = PixelSize::new(4096, 3072);

/// Настройки работы «снимок».
#[derive(Debug, Clone, Copy)]
pub struct StillSaturationConfig {
    /// Размер кадра (RAW10).
    pub frame_size: PixelSize,
    /// Настройки проявления (продуктовые: полоса 256 строк, миниатюра 256 px).
    pub photo: PhotoConfig,
    /// Поворот снимка (миниатюра пишется уже повёрнутой).
    pub rotation: StillRotation,
    /// Нужна ли копия картинки в память хоста. `[estimate]` `NotNeeded`: путь HEIF с GPU (`PrimaryTarget::EncoderSurface`);
    /// путь JPEG (`ForJpeg`) добавляет копию 48 МиБ.
    pub readback: RgbaReadback,
    /// Слотов стенда (`frames_in_flight`), под которые заведены проявители.
    pub frame_slots: usize,
    /// Содержимое синтетического кадра.
    pub content: SceneContent,
    /// Паддинг конца строки RAW10, байты.
    pub raw10_row_padding_bytes: u32,
    /// Замер времени GPU по проходам.
    pub pass_timing: StillPassTiming,
}

impl Default for StillSaturationConfig {
    fn default() -> Self {
        Self {
            frame_size: STILL_FRAME_SIZE,
            photo: PhotoConfig::default(),
            rotation: StillRotation::None,
            readback: RgbaReadback::NotNeeded,
            frame_slots: 2,
            content: SceneContent::default(),
            raw10_row_padding_bytes: 0,
            pass_timing: StillPassTiming::default(),
        }
    }
}

impl StillSaturationConfig {
    /// Проверка без GPU.
    pub fn validate(&self) -> Result<(), SaturationWorkError> {
        check_raw10_size(self.frame_size)?;
        if self.frame_slots == 0 {
            return Err(SaturationWorkError::invalid_config(
                "frame_slots = 0: нужен хотя бы один слот",
            ));
        }
        self.photo.validate()?;
        Ok(())
    }

    /// Сколько проявителей заводится: проявитель занят от записи кадра до подтверждения маркером, а маркер
    /// завершается лишь после кадров, стоящих в очереди ПЕРЕД ним (до `frame_slots` штук) — запас, чтобы запись
    /// нового кадра не упёрлась в неподтверждённый проявитель.
    pub fn developer_count(&self) -> usize {
        2 * self.frame_slots + 1
    }
}

/// Счётчики работы.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct StillCounters {
    /// Кадров (снимков) записано.
    pub frames_recorded: u64,
    /// Кадров освобождено после fence.
    pub frames_released: u64,
    /// Проявок, подтверждённых и завершённых (`finish` вернул снимок).
    pub developments_finished: u64,
    /// Из них с готовой миниатюрой.
    pub thumbnails_ready: u64,
}

/// Состояние проявителя.
enum DeveloperState {
    /// Проявка начата (`begin`), шаги не записаны.
    Ready,
    /// Шаги записаны в командный буфер стенда, fence кадра ещё не пройден.
    Recorded,
    /// Кадр завершён, отправлен маркер подтверждения; ждём его завершения.
    AwaitingMarker(SubmittedWork),
    /// Нет активной проявки (после ошибки).
    Idle,
}

struct DeveloperEntry {
    developer: PhotoDeveloper,
    state: DeveloperState,
}

/// Работа «снимок» (см. описание модуля).
pub struct StillSaturation {
    // Порядок полей = порядок уничтожения: проявители до очереди маркеров и до источника.
    developers: Vec<DeveloperEntry>,
    markers: OneShotSubmitter,
    raw: RawCapture,
    params: IspParams,
    config: StillSaturationConfig,
    steps_per_still: usize,
    /// Какой проявитель записан в каждом слоте стенда (до `release_slot_after_fence`).
    slot_developer: Vec<Option<usize>>,
    last_image: Option<Arc<Rgba8StorageImage>>,
    /// Дайджест миниатюры последнего завершённого снимка (`None` — миниатюры не было).
    last_thumbnail_digest: Option<OutputDigest>,
    times: PassTimeLog,
    counters: StillCounters,
    failure: FirstFailure,
    _holder: Box<dyn RawFrameHolder>,
}

impl StillSaturation {
    /// Создаёт работу: захватывает RAW один раз, заводит проявители и начинает в каждом первую проявку.
    pub fn new(
        context: &GpuContext,
        config: StillSaturationConfig,
        params: IspParams,
        holder: Box<dyn RawFrameHolder>,
    ) -> Result<Self, SaturationWorkError> {
        config.validate()?;
        let source = holder.source();
        check_source_size(&source, config.frame_size)?;
        let raw = capture_once(context, &config, &source)?;
        let count = config.developer_count();
        let mut developers = Vec::with_capacity(count);
        for _ in 0..count {
            let mut developer = PhotoDeveloper::new(context, &config.photo, config.frame_size.width)?;
            if config.pass_timing == StillPassTiming::PerPass {
                developer.enable_gpu_timing()?;
            }
            developers.push(DeveloperEntry {
                developer,
                state: DeveloperState::Idle,
            });
        }
        let mut work = Self {
            developers,
            markers: OneShotSubmitter::new(context, count)?,
            raw,
            params,
            config,
            steps_per_still: 0,
            slot_developer: vec![None; config.frame_slots],
            last_image: None,
            last_thumbnail_digest: None,
            times: PassTimeLog::new(&PhotoGpuPoint::span_labels(), PASS_LOG_CAPACITY_FRAMES),
            counters: StillCounters::default(),
            failure: FirstFailure::default(),
            _holder: holder,
        };
        for index in 0..count {
            work.begin_development(index)?;
        }
        Ok(work)
    }

    /// Шагов записи на один снимок: полосы вывода + отделка (миниатюра).
    pub fn steps_per_still(&self) -> usize {
        self.steps_per_still
    }

    /// Сколько слотов стенда допустимо.
    pub fn slots(&self) -> usize {
        self.config.frame_slots
    }

    /// Настройки.
    pub fn config(&self) -> &StillSaturationConfig {
        &self.config
    }

    /// Счётчики.
    pub fn counters(&self) -> StillCounters {
        self.counters
    }

    /// Итог «по проходам»: проходов нет, только число кадров без замера.
    pub fn pass_times(&self) -> PassTimeSummary {
        self.times.summary()
    }

    /// Обнуляет журнал и счётчики.
    pub fn reset_statistics(&mut self) {
        self.times.clear();
        self.counters = StillCounters::default();
    }

    /// Первая ошибка внутри `record_frame`/`release_slot_after_fence`.
    pub fn take_failure(&mut self) -> Option<SaturationWorkError> {
        self.failure.take()
    }

    /// Начинает проявку в проявителе `index` (выделения здесь, не в кадре).
    fn begin_development(&mut self, index: usize) -> Result<(), SaturationWorkError> {
        let request = DevelopRequest {
            raw: &self.raw,
            params: &self.params,
            crop: PixelRect {
                left: 0,
                top: 0,
                size: self.config.frame_size,
            },
            readback: self.config.readback,
            rotation: self.config.rotation,
        };
        let entry = &mut self.developers[index];
        let start = entry.developer.begin(&request)?;
        entry.state = DeveloperState::Ready;
        self.steps_per_still = start.slice_count;
        Ok(())
    }

    /// Кладёт в журнал времена проявки по проходам или отметку «без замера» (метки не готовы, выключены).
    fn log_development_timing(&mut self, timing: &PhotoGpuTimingCollect) {
        let PhotoGpuTimingCollect::Ready(timings) = timing else {
            self.times.record_unmeasured_frame();
            return;
        };
        let durations: Option<Vec<Duration>> = PhotoGpuPoint::span_labels()
            .iter()
            .map(|label| {
                timings
                    .spans
                    .iter()
                    .find(|(span_label, _)| span_label == label)
                    .map(|&(_, nanoseconds)| Duration::from_nanos(nanoseconds))
            })
            .collect();
        match durations {
            Some(durations) => self.times.record_frame(&durations),
            None => self.times.record_unmeasured_frame(),
        }
    }

    /// Подтверждает и завершает все проявки, чей маркер уже выполнен (без ожидания GPU).
    fn reap_confirmed(&mut self) -> Result<(), SaturationWorkError> {
        for index in 0..self.developers.len() {
            if !matches!(self.developers[index].state, DeveloperState::AwaitingMarker(_)) {
                continue;
            }
            let DeveloperState::AwaitingMarker(marker) =
                std::mem::replace(&mut self.developers[index].state, DeveloperState::Idle)
            else {
                continue;
            };
            match self.markers.poll(&marker) {
                WorkStatus::Completed(proof) => {
                    for _ in 0..self.steps_per_still {
                        self.developers[index].developer.slice_completed(&proof)?;
                    }
                    let mut photo = self.developers[index].developer.finish()?;
                    // Метки собираем до новой проявки этого проявителя: её первый шаг сбросит их.
                    let timing = self.developers[index].developer.collect_gpu_timing()?;
                    self.log_development_timing(&timing);
                    if let ThumbnailOutcome::Ready { size, rgba } = photo.take_thumbnail() {
                        self.counters.thumbnails_ready += 1;
                        // Дайджест миниатюры берём сразу: байты в обычной памяти, GPU не нужен.
                        self.last_thumbnail_digest = Some(digest_rgba8_bytes(
                            ash::vk::Extent2D {
                                width: size.width,
                                height: size.height,
                            },
                            &rgba,
                        ));
                    }
                    self.last_image = Some(Arc::clone(photo.gpu_image()));
                    self.counters.developments_finished += 1;
                    self.begin_development(index)?;
                }
                WorkStatus::Pending => self.developers[index].state = DeveloperState::AwaitingMarker(marker),
                WorkStatus::Failed(failure) => {
                    return Err(SaturationWorkError::Gpu(GpuError::InvalidState {
                        what: format!("опрос маркера подтверждения не удался: {failure:?}"),
                    }));
                }
            }
        }
        Ok(())
    }

    /// Ждёт все маркеры и завершает все проявки; не начатые заново проявки отменяются. Звать после прогона
    /// стенда (все fence кадров пройдены). После вызова [`Self::read_last_image_blocking`] видит последний снимок.
    pub fn finish_pending_blocking(&mut self) -> Result<(), SaturationWorkError> {
        match self.markers.drain(PREPARATION_WAIT_TIMEOUT)? {
            DrainOutcome::Drained => {}
            DrainOutcome::TimedOut { .. } => {
                return Err(SaturationWorkError::Gpu(GpuError::Timeout {
                    step: "OneShotSubmitter::drain(StillSaturation::finish_pending_blocking)",
                }));
            }
        }
        self.reap_confirmed()?;
        for entry in &mut self.developers {
            if matches!(entry.state, DeveloperState::Ready) {
                entry.developer.cancel()?;
                entry.state = DeveloperState::Idle;
            }
        }
        Ok(())
    }

    fn record_inner(&mut self, recording: FrameRecording) -> Result<(), SaturationWorkError> {
        let slot = recording.slot().index();
        let slots = self.config.frame_slots;
        if slot >= slots {
            return Err(SaturationWorkError::SlotOutOfRange { slot, slots });
        }
        self.reap_confirmed()?;
        let index = usize::try_from(recording.frame_index() % self.developers.len() as u64).unwrap_or(0);
        let entry = &mut self.developers[index];
        if !matches!(entry.state, DeveloperState::Ready) {
            return Err(SaturationWorkError::EmptyFrame {
                what: format!("проявитель {index} ещё не подтверждён маркером: кадр не записан"),
            });
        }
        for step in 0..self.steps_per_still {
            match entry.developer.record_next_slice(recording.command_buffer())? {
                SliceOutcome::Recorded { .. } => {}
                SliceOutcome::Finished => {
                    return Err(SaturationWorkError::EmptyFrame {
                        what: format!(
                            "шагов проявки меньше ожидаемых: остановились на {step} из {}",
                            self.steps_per_still
                        ),
                    });
                }
            }
        }
        entry.state = DeveloperState::Recorded;
        self.slot_developer[slot] = Some(index);
        self.counters.frames_recorded += 1;
        Ok(())
    }

    fn release_inner(&mut self, slot: SlotId) -> Result<(), SaturationWorkError> {
        let index = self.slot_developer[slot.index()]
            .take()
            .ok_or_else(|| SaturationWorkError::EmptyFrame {
                what: format!("слот {} освобождён без записанного кадра", slot.index()),
            })?;
        // Кадр завершён (fence стенда). Маркер встаёт в очередь после него; его доказательство подтвердит проявку.
        let marker = self
            .markers
            .submit(Vec::new(), |_command_buffer| Ok::<(), GpuError>(()))
            .map_err(|error| match error {
                SubmitError::Record(error) | SubmitError::Gpu(error) => SaturationWorkError::Gpu(error),
                SubmitError::CapacityExhausted { capacity } => SaturationWorkError::EmptyFrame {
                    what: format!("очередь маркеров ({capacity}) переполнена"),
                },
            })?;
        let entry = &mut self.developers[index];
        entry.developer.note_slice_submitted(&marker)?;
        entry.state = DeveloperState::AwaitingMarker(marker);
        self.counters.frames_released += 1;
        self.reap_confirmed()
    }
}

impl SaturationWorkload for StillSaturation {
    fn record_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError> {
        self.record_inner(recording).map_err(|error| self.failure.note(error))
    }

    fn release_slot_after_fence(&mut self, slot: SlotId) {
        if let Err(error) = self.release_inner(slot) {
            self.failure.note(error);
        }
    }
}

impl Drop for StillSaturation {
    fn drop(&mut self) {
        // Проявитель с неподтверждёнными полосами при уничтожении намеренно утекает (соглашение `photo`):
        // подтверждаем всё, что можно, прежде чем объекты уйдут.
        if let Err(error) = self.finish_pending_blocking() {
            tracing::warn!(target: "fasticamera::saturation", %error, "проявки снимка не подтверждены при завершении работы");
        }
    }
}

/// Захват RAW один раз (блокирующе): копия полезной области кадра камеры в буфер снимка.
fn capture_once(
    context: &GpuContext,
    config: &StillSaturationConfig,
    source: &MosaicSource,
) -> Result<RawCapture, SaturationWorkError> {
    let mut capturer = RawCapturer::new(context, &config.photo)?;
    let valid_area = ValidArea {
        left: 0,
        top: 0,
        width: config.frame_size.width,
        height: config.frame_size.height,
    };
    let mut pending = None;
    let run = run_blocking(context, |command_buffer| {
        source.record_acquire_for(context, command_buffer, ConsumerStage::Compute);
        let recorded = capturer.record_capture(command_buffer, source, valid_area);
        source.record_release_for(context, command_buffer, ConsumerStage::Compute);
        pending = Some(recorded.map_err(|error| GpuError::InvalidState {
            what: format!("захват RAW: {error}"),
        })?);
        Ok(())
    })?;
    let mut pending = pending.ok_or_else(|| SaturationWorkError::EmptyFrame {
        what: "захват RAW не записан".to_owned(),
    })?;
    pending.note_submitted(&run.work);
    Ok(capturer.complete_after_fence(pending, &run.proof)?)
}

impl StillSaturation {
    /// Работа на синтетическом RAW10 (ПК, фича `test-support`).
    pub fn new_synthetic(
        context: &GpuContext,
        config: StillSaturationConfig,
        params: IspParams,
    ) -> Result<Self, SaturationWorkError> {
        config.validate()?;
        let frame = super::scene::synthesize_raw10(config.frame_size, config.content, config.raw10_row_padding_bytes)?;
        let holder = super::source::synthetic_raw10_holder(context, &frame)?;
        Self::new(context, config, params, holder)
    }
}

impl StillSaturation {
    /// Дайджест последнего завершённого снимка: картинка RGBA8 и (если была) миниатюра. Звать после
    /// [`Self::finish_pending_blocking`] и `wait_idle`; работает без `test-support`.
    pub fn output_digest(&self) -> Result<WorkloadDigest, SaturationWorkError> {
        let image = self
            .last_image
            .as_ref()
            .ok_or_else(|| SaturationWorkError::EmptyFrame {
                what: "ни одна проявка ещё не завершена".to_owned(),
            })?;
        let mut parts = vec![DigestPart {
            label: "image",
            digest: digest_rgba8_image(image)?,
        }];
        if let Some(digest) = self.last_thumbnail_digest {
            parts.push(DigestPart {
                label: "thumbnail",
                digest,
            });
        }
        Ok(WorkloadDigest::from_parts(parts))
    }
}

#[cfg(feature = "test-support")]
impl StillSaturation {
    /// Последний завершённый снимок: RGBA8 построчно. Звать после [`Self::finish_pending_blocking`].
    pub fn read_last_image_blocking(&self) -> Result<Vec<u8>, SaturationWorkError> {
        let image = self
            .last_image
            .as_ref()
            .ok_or_else(|| SaturationWorkError::EmptyFrame {
                what: "ни одна проявка ещё не завершена".to_owned(),
            })?;
        Ok(image.read_back_blocking()?)
    }
}
