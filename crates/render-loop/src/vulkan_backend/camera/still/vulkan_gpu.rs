//! Настоящая GPU-часть снимка поверх `isp-vulkan::photo` и `gpu-vulkan` (`OneShotSubmitter`, `EncoderInputTarget`).
//!
//! # Что и в каком порядке происходит на GPU
//! ```text
//! capture_start:  импорт буфера камеры → acquire-fence в семафор → ОДИН короткий сабмит:
//!                 приём владения (FOREIGN → наша очередь) · копия полезной области в буфер снимка ·
//!                 возврат владения (наша очередь → FOREIGN)
//! capture_poll:   fence сабмита сигнален → запечатать буфер → кадр возвращается камере (кадр уже скопирован)
//! develop_*:      по полосе на сабмит (PhotoDeveloper), каждая полоса — свой fence, опрос без ожидания
//! present_*:      swapchain на поверхности кодера → копия картинки → present → неблокирующий разбор цели
//!                 (`present.rs`: опрос fence-ов показа на следующих кадрах, без ожидания)
//! ```
//! Приём и возврат владения — в ОДНОМ сабмите: кадр снимка не живёт в нашей очереди дольше копии, а fence копии
//! покрывает и возврат (соглашение «Лизинг и видоискатель» conventions).
//!
//! # Импорт и серии
//! У снимка СВОЙ импортёр (та же фабрика, что у показа) и свои номера серий: жизнь импорта по серии не
//! конфликтует с видоискателем, а чужой кэш снимок не засоряет.
//!
//! # Уничтожение
//! Ни одного бесконечного ожидания: остановка — `OneShotSubmitter::drain(teardown_timeout)`. Не успели — объекты
//! намеренно утекают (импортёр, лизинг, проявитель), см. [`StillGpu::abandon`].

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::{PixelRect, PixelSize};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_readback::HostReadbackBuffer;
use gpu_vulkan::one_shot_queue::{DrainOutcome, OneShotSubmitter, SubmitError, SubmittedWork, WorkStatus};
use gpu_vulkan::raw_mosaic_preview::ValidArea;
use gpu_vulkan::sync::{GpuReleaseFence, import_wait_fence};
use isp_model::params::IspParams;
use isp_vulkan::photo::{
    DevelopRequest, DevelopTargets, DevelopedPhoto, PendingRawCapture, PhotoConfig, PhotoDeveloper, PhotoError,
    RawCapture, RawCapturer, RgbaReadback, SliceOutcome,
};
use raw_frame_stream::RawFrameLease;
use shell_contract::surface::NativeSurface;
use still_contract::shot::{RawSamples16, Rgba8Image};

use super::gpu::{
    AbandonReport, CapturePoll, DevelopProgress, DevelopStart, PresentProgress, PresentRefusal, StillGpu,
};
use crate::config::StillDevelopConfig;
use crate::vulkan_backend::camera::{CameraFrameImporter, SharedImporterFactory, SubmissionSerial};
use present::PresentChannel;
use workshop::{StillWorkshop, Ticket, TicketPoll};

mod present;
mod workshop;

/// Слотов одноразовой очереди, зарезервированных вне проявления: копия RAW (1) и показ в кодер (1). Проявление
/// ограничено `max_slices_in_flight`, поэтому занять эти слоты не может (второй затвор во время проявления
/// первого всегда получает слот под копию).
const RESERVED_SUBMIT_SLOTS: usize = 2;

/// Настройки GPU снимков (из `StillDevelopConfig` и `VulkanBackendConfig`).
#[derive(Debug, Clone, Copy)]
pub struct VulkanStillSettings {
    pub photo: PhotoConfig,
    /// Сколько ждать GPU при остановке (`teardown_timeout` бэкенда).
    pub teardown_timeout: Duration,
    /// Сколько полос проявления может быть в полёте одновременно.
    pub max_slices_in_flight: usize,
    /// Сколько целей кодера, чей показ нельзя отследить, держать до остановки.
    pub retired_present_capacity: usize,
}

impl VulkanStillSettings {
    /// Настройки из секции `still` конфига рендера.
    pub fn from_config(config: &StillDevelopConfig, teardown_timeout: Duration) -> Self {
        Self {
            photo: config.photo,
            teardown_timeout,
            max_slices_in_flight: usize::try_from(config.max_slices_in_flight).unwrap_or(usize::MAX),
            retired_present_capacity: usize::try_from(config.retired_present_capacity).unwrap_or(usize::MAX),
        }
    }

    /// Ёмкость очереди одноразовой работы: полосы проявления в полёте + зарезервированные слоты копии и показа.
    pub fn submit_capacity(&self) -> usize {
        self.max_slices_in_flight.saturating_add(RESERVED_SUBMIT_SLOTS)
    }
}

/// Копия RAW в полёте.
struct CaptureJob {
    work: SubmittedWork,
    pending: PendingRawCapture,
    lease: RawFrameLease,
    serial: SubmissionSerial,
}

/// Проявление в полёте.
struct DevelopJob {
    /// Сколько полос ещё не записано.
    slices_left: usize,
    /// Отправленные полосы по порядку (их fence-ы подтверждаются по порядку записи).
    in_flight: VecDeque<SubmittedWork>,
    /// Сбой: полосы не отправляются, ждём завершения уже отправленных и сообщаем итог.
    failure: Option<String>,
}

/// Проявление, чьи цели (картинка, копия для JPEG) выделяет мастерская: всё, что нужно для `begin_with_targets`.
struct PendingDevelop {
    targets: Ticket<Result<DevelopTargets, PhotoError>>,
    raw: RawCapture,
    params: IspParams,
    crop: PixelRect,
    readback: RgbaReadback,
}

impl DevelopJob {
    fn new(slice_count: usize) -> Self {
        Self {
            slices_left: slice_count,
            in_flight: VecDeque::new(),
            failure: None,
        }
    }
}

/// GPU снимков под один поток кадров.
pub struct VulkanStillGpu {
    context: GpuContext,
    settings: VulkanStillSettings,
    submitter: OneShotSubmitter,
    capturer: RawCapturer,
    developer: PhotoDeveloper,
    /// `Option`: при зависшем GPU импортёр намеренно утекает (`take` + `forget`).
    importer: Option<Box<dyn CameraFrameImporter>>,
    max_valid_width: u32,
    next_serial: u64,
    capture: Option<CaptureJob>,
    develop: Option<DevelopJob>,
    present: PresentChannel,
    /// Мастерская снимка (решение 144): подключение цели кодера и буфер следующей копии RAW — не на потоке
    /// рендера. `None` — поток не запустился: та же работа идёт на потоке рендера, как до 14c (медленнее, но верно).
    /// Общая на все GPU снимков одного движка (создаёт `maker`): GPU пересоздаётся при смене потока или потере,
    /// а мастерская живёт, пока жив движок. Поэтому её `Drop` (с ожиданием начатой работы) случается только при
    /// разборе движка, а не на потоке рендера посреди кадров или при отсоединении surface.
    workshop: Option<Arc<StillWorkshop>>,
    /// Проявление ждёт свои цели из мастерской (до первой полосы).
    pending_develop: Option<PendingDevelop>,
    /// Буфер следующей копии RAW готовится в мастерской.
    next_capture_buffer: Option<Ticket<Result<HostReadbackBuffer, GpuError>>>,
    /// GPU объявлен потерянным ([`StillGpu::declare_lost`]): дальше им пользоваться нельзя.
    lost: bool,
}

impl VulkanStillGpu {
    /// Фабрика GPU снимков для движка: запускает мастерскую снимка ОДИН раз (решение 144) и отдаёт её каждому
    /// создаваемому GPU. Мастерская живёт, пока жива фабрика (то есть движок): её поток не переживает владельца
    /// и не разбирает Vulkan при выходе процесса (14c: отпущенный поток с последним клоном `GpuContext` ронял
    /// процесс тестов SIGSEGV на выходе).
    pub fn maker(
        context: &GpuContext,
        settings: VulkanStillSettings,
        importers: &SharedImporterFactory,
    ) -> super::engine::GpuMaker<Self> {
        let workshop = StillWorkshop::start()
            .inspect_err(|error| {
                tracing::warn!(target: super::LOG_TARGET, %error, "без мастерской: подготовка снимка — на потоке рендера");
            })
            .ok()
            .map(Arc::new);
        let (context, importers) = (context.clone(), importers.clone());
        Box::new(move |stream| Self::new(&context, settings, &importers, stream, workshop.clone()))
    }

    /// Создаёт GPU снимков под поток `stream`: очередь работ, захватчик, проявитель, импортёр. Выделяет буфер
    /// копии заранее (миллисекунды CPU), чтобы путь затвора его не выделял. `workshop` — мастерская движка
    /// (`None` — подготовка на потоке рендера).
    fn new(
        context: &GpuContext,
        settings: VulkanStillSettings,
        importers: &SharedImporterFactory,
        stream: &StreamPassport,
        workshop: Option<Arc<StillWorkshop>>,
    ) -> Result<Self, String> {
        let max_valid_width = stream.valid_area.size.width;
        let submitter =
            OneShotSubmitter::new(context, settings.submit_capacity()).map_err(|error| error.to_string())?;
        let mut capturer = RawCapturer::new(context, &settings.photo).map_err(|error| error.to_string())?;
        let developer =
            PhotoDeveloper::new(context, &settings.photo, max_valid_width).map_err(|error| error.to_string())?;
        let importer = importers.make(context).map_err(|error| error.to_string())?;
        capturer
            .prepare(stream.valid_area.size)
            .map_err(|error| error.to_string())?;
        Ok(Self {
            context: context.clone(),
            settings,
            submitter,
            capturer,
            developer,
            importer: Some(importer),
            max_valid_width,
            next_serial: 1,
            capture: None,
            develop: None,
            present: PresentChannel::new(settings.retired_present_capacity),
            workshop,
            pending_develop: None,
            next_capture_buffer: None,
            lost: false,
        })
    }

    fn valid_area(rect: PixelRect) -> ValidArea {
        ValidArea {
            left: rect.left,
            top: rect.top,
            width: rect.size.width,
            height: rect.size.height,
        }
    }

    /// Лизинг после неудавшегося запуска копии: GPU кадр не читал — возвращаем нетронутым.
    fn refuse_capture(&mut self, lease: RawFrameLease, serial: SubmissionSerial, error: String) -> Result<(), String> {
        lease.release_unused();
        if let Some(importer) = self.importer.as_mut() {
            importer.retire_completed(serial);
        }
        Err(error)
    }

    // ---- остановка ----

    /// Доводит до конца всё, что завершилось (после `drain`): копия возвращает кадр камере, проявление отменяется,
    /// показ разбирается. Вызывается, когда GPU подтверждённо простаивает.
    fn settle_after_idle(&mut self) {
        if let Some(job) = self.capture.take() {
            match self.submitter.poll(&job.work) {
                WorkStatus::Completed(proof) => {
                    let CaptureJob {
                        pending, lease, serial, ..
                    } = job;
                    // Результат копии не нужен (снимок брошен); подтверждение освобождает набор дескрипторов.
                    drop(self.capturer.complete_after_fence(pending, &proof));
                    lease.release_after_gpu_fence(GpuReleaseFence::already_signaled());
                    if let Some(importer) = self.importer.as_mut() {
                        importer.retire_completed(serial);
                    }
                }
                WorkStatus::Pending | WorkStatus::Failed(_) => {
                    // Исход неизвестен: лизинг не возвращаем (учтённая утечка), память копии остаётся (Drop).
                    let CaptureJob { pending, lease, .. } = job;
                    std::mem::forget(pending);
                    drop(lease);
                }
            }
        }
        if let Some(job) = self.develop.take() {
            for work in job.in_flight {
                if let WorkStatus::Completed(proof) = self.submitter.poll(&work) {
                    drop(self.developer.slice_completed(&proof));
                }
            }
            if let Err(error) = self.developer.cancel() {
                tracing::warn!(target: super::LOG_TARGET, %error, "проявление не отменилось при остановке");
            }
        }
        // Проявление, не дождавшееся целей, на GPU ничего не отправляло: просто бросаем.
        self.pending_develop = None;
        self.present.settle_after_idle(&mut self.submitter);
    }

    /// Цели проявления из мастерской (без ожидания): готовы — проявление начинается (`None`: дальше обычный шаг
    /// с полосами), не готовы — `Some(InFlight)`, сбой — `Some(Failed)`. GPU цели ещё не видел: при сбое их можно
    /// уничтожать сразу.
    fn begin_when_targets_ready(&mut self, pending: PendingDevelop) -> Option<DevelopProgress<DevelopedPhoto>> {
        let targets = match pending.targets.poll() {
            TicketPoll::Pending => {
                self.pending_develop = Some(pending);
                return Some(DevelopProgress::InFlight { submitted_now: 0 });
            }
            TicketPoll::Ready(Ok(targets)) => targets,
            TicketPoll::Ready(Err(error)) => {
                return Some(DevelopProgress::Failed(format!("цели проявления: {error}")));
            }
            TicketPoll::NoResult => {
                return Some(DevelopProgress::Failed(
                    "выделение целей проявления не дало результата".to_owned(),
                ));
            }
        };
        let request = DevelopRequest {
            raw: &pending.raw,
            params: &pending.params,
            crop: pending.crop,
            readback: pending.readback,
        };
        match self.developer.begin_with_targets(&request, targets) {
            Ok(start) => {
                self.develop = Some(DevelopJob::new(start.slice_count));
                None
            }
            Err(error) => Some(DevelopProgress::Failed(error.to_string())),
        }
    }

    /// Буфер следующей копии RAW: в мастерской, если она есть, иначе здесь же (поток рендера, как до 14c).
    fn request_next_capture_buffer(&mut self, valid_size: PixelSize) {
        let Some(workshop) = self.workshop.as_ref() else {
            if let Err(error) = self.capturer.prepare(valid_size) {
                tracing::warn!(target: super::LOG_TARGET, %error, "буфер следующей копии не подготовлен");
            }
            return;
        };
        let context = self.context.clone();
        let size_bytes = RawCapturer::buffer_bytes(valid_size);
        match workshop.submit("буфер копии RAW", move || {
            HostReadbackBuffer::new(&context, size_bytes)
        }) {
            Ok(ticket) => self.next_capture_buffer = Some(ticket),
            Err(error) => tracing::warn!(target: super::LOG_TARGET, %error, "буфер следующей копии не заказан"),
        }
    }

    /// Забирает готовый буфер следующей копии из мастерской (без ожидания). Не успел — копия выделит буфер сама
    /// на потоке рендера (верно, но медленно: это видно в логе).
    fn collect_next_capture_buffer(&mut self) {
        let Some(ticket) = self.next_capture_buffer.take() else {
            return;
        };
        match ticket.poll() {
            TicketPoll::Ready(Ok(buffer)) => self.capturer.install_prepared(buffer),
            TicketPoll::Ready(Err(error)) => {
                tracing::warn!(target: super::LOG_TARGET, %error, "мастерская не выделила буфер копии RAW");
            }
            TicketPoll::Pending => {
                tracing::warn!(
                    target: super::LOG_TARGET,
                    "буфер копии RAW из мастерской не успел к снимку: выделение на потоке рендера"
                );
                // Билет не нужен этому снимку, но готовый буфер пригодится следующему.
                self.next_capture_buffer = Some(ticket);
            }
            TicketPoll::NoResult => {
                tracing::warn!(target: super::LOG_TARGET, "работа мастерской с буфером копии RAW упала");
            }
        }
    }
}

impl StillGpu for VulkanStillGpu {
    type Raw = RawCapture;
    type Image = DevelopedPhoto;

    fn serves(&self, stream: &StreamPassport) -> bool {
        stream.valid_area.size.width <= self.max_valid_width
    }

    fn capture_in_flight(&self) -> bool {
        self.capture.is_some()
    }

    fn develop_in_flight(&self) -> bool {
        self.develop.is_some() || self.pending_develop.is_some()
    }

    fn present_in_flight(&self) -> bool {
        self.present.in_flight()
    }

    fn capture_start(&mut self, lease: RawFrameLease) -> Result<(), String> {
        let serial = SubmissionSerial(self.next_serial);
        self.next_serial += 1;
        if self.lost {
            return self.refuse_capture(lease, serial, "GPU снимков потерян".to_owned());
        }
        if self.capture.is_some() {
            return self.refuse_capture(lease, serial, "копия RAW уже идёт".to_owned());
        }
        let area = Self::valid_area(lease.passport().stream.valid_area);
        self.collect_next_capture_buffer();
        let Some(importer) = self.importer.as_mut() else {
            return self.refuse_capture(lease, serial, "импортёр снимков остановлен".to_owned());
        };
        let source = match importer.import_frame(&lease, serial) {
            Ok(source) => source,
            Err(error) => return self.refuse_capture(lease, serial, error.to_string()),
        };
        let wait = match lease
            .acquire_fence()
            .map_err(|error| format!("acquire-fence кадра: {error}"))
            .and_then(|fence| {
                import_wait_fence(&self.context, fence)
                    .map_err(|error| format!("импорт acquire-fence: {}", error.reason))
            }) {
            Ok(wait) => wait,
            Err(error) => return self.refuse_capture(lease, serial, error),
        };
        let context = self.context.clone();
        let mut recorded: Option<PendingRawCapture> = None;
        let submitted = self.submitter.submit(vec![wait], |command_buffer| {
            // Приём и возврат владения — в одном сабмите, копия между ними (см. модуль).
            source.record_acquire_for(&context, command_buffer, ConsumerStage::Compute);
            recorded = Some(self.capturer.record_capture(command_buffer, &source, area)?);
            source.record_release_for(&context, command_buffer, ConsumerStage::Compute);
            Ok::<(), PhotoError>(())
        });
        match (submitted, recorded) {
            (Ok(work), Some(mut pending)) => {
                // Захват узнаёт записавшую работу: запечатать буфер можно только её доказательством.
                pending.note_submitted(&work);
                self.capture = Some(CaptureJob {
                    work,
                    pending,
                    lease,
                    serial,
                });
                Ok(())
            }
            (Ok(_), None) => {
                // Недостижимо (замыкание успешно только с записанной копией); GPU работу принял — кадр не возвращаем.
                drop(lease);
                Err("копия принята без записи: лизинг оставлен (учтённая утечка)".to_owned())
            }
            (Err(error), pending) => {
                // Работа GPU не принята: командный буфер выброшен, GPU кадр не читал.
                if let Some(pending) = pending
                    && let Err(discard) = self.capturer.discard_unsubmitted(pending)
                {
                    tracing::warn!(target: super::LOG_TARGET, error = %discard, "несостоявшаяся копия не сброшена");
                }
                let reason = match &error {
                    SubmitError::CapacityExhausted { capacity } => format!("очередь работ занята ({capacity} слотов)"),
                    SubmitError::Record(error) => format!("запись копии: {error}"),
                    SubmitError::Gpu(error) => format!("отправка копии: {error}"),
                };
                self.refuse_capture(lease, serial, reason)
            }
        }
    }

    fn capture_poll(&mut self) -> CapturePoll<RawCapture> {
        let Some(job) = self.capture.take() else {
            return CapturePoll::NothingInFlight;
        };
        match self.submitter.poll(&job.work) {
            WorkStatus::Pending => {
                self.capture = Some(job);
                CapturePoll::Pending
            }
            WorkStatus::Completed(proof) => {
                let CaptureJob {
                    pending, lease, serial, ..
                } = job;
                let completed = self.capturer.complete_after_fence(pending, &proof);
                // Копия на GPU закончена в любом случае (fence сигнален): кадр камере возвращаем.
                lease.release_after_gpu_fence(GpuReleaseFence::already_signaled());
                if let Some(importer) = self.importer.as_mut() {
                    importer.retire_completed(serial);
                }
                match completed {
                    Ok(raw) => {
                        // Буфер для следующей копии — заранее, вне пути затвора (и вне потока рендера).
                        self.request_next_capture_buffer(raw.size());
                        CapturePoll::Ready(raw)
                    }
                    Err(error) => CapturePoll::Failed(error.to_string()),
                }
            }
            WorkStatus::Failed(failure) => {
                // Исход неизвестен (потеря устройства): лизинг — учтённая утечка, память копии остаётся. GPU
                // снимков движок теперь считает потерянным (`Lost`): `capture_in_flight` не залипает в «занято».
                let CaptureJob { pending, lease, .. } = job;
                std::mem::forget(pending);
                drop(lease);
                CapturePoll::Lost(format!("опрос копии: {failure:?}"))
            }
        }
    }

    fn raw_samples(&self, raw: &RawCapture) -> Result<RawSamples16, String> {
        raw.samples().map_err(|error| error.to_string())
    }

    fn develop_start(&mut self, request: DevelopStart<'_, RawCapture>) -> Result<(), String> {
        if self.develop_in_flight() {
            return Err("проявление уже идёт".to_owned());
        }
        let Some(workshop) = self.workshop.as_ref() else {
            // Без мастерской цели выделяются здесь же, на потоке рендера (как до 14c).
            let start = self
                .developer
                .begin(&DevelopRequest {
                    raw: request.raw,
                    params: request.params,
                    crop: request.crop,
                    readback: request.readback,
                })
                .map_err(|error| error.to_string())?;
            self.develop = Some(DevelopJob::new(start.slice_count));
            return Ok(());
        };
        // Отказ по входу — сразу, как до мастерской (а не через опрос): цели под негодный запрос не заказываем.
        self.developer
            .check_request(&DevelopRequest {
                raw: request.raw,
                params: request.params,
                crop: request.crop,
                readback: request.readback,
            })
            .map_err(|error| error.to_string())?;
        // Цели (≈ 48 МБ картинки и, для JPEG, ещё ≈ 48 МБ копии) выделяет мастерская; первая полоса — когда готовы.
        let (context, image_size, readback) = (self.context.clone(), request.crop.size, request.readback);
        let targets = workshop
            .submit("цели проявления", move || {
                DevelopTargets::allocate(&context, image_size, readback)
            })
            .map_err(|error| format!("цели проявления не заказаны: {error}"))?;
        self.pending_develop = Some(PendingDevelop {
            targets,
            raw: request.raw.clone(),
            params: request.params.clone(),
            crop: request.crop,
            readback: request.readback,
        });
        Ok(())
    }

    fn develop_advance(&mut self, max_new_slices: u32) -> DevelopProgress<DevelopedPhoto> {
        if let Some(pending) = self.pending_develop.take()
            && let Some(progress) = self.begin_when_targets_ready(pending)
        {
            return progress;
        }
        let Some(mut job) = self.develop.take() else {
            return DevelopProgress::Idle;
        };
        // 1. Завершённые полосы (по порядку записи) подтверждаем проявителю.
        while let Some(work) = job.in_flight.front() {
            match self.submitter.poll(work) {
                WorkStatus::Completed(proof) => {
                    job.in_flight.pop_front();
                    if let Err(error) = self.developer.slice_completed(&proof) {
                        job.failure.get_or_insert_with(|| error.to_string());
                    }
                }
                WorkStatus::Pending => break,
                WorkStatus::Failed(failure) => {
                    // Исход полосы неизвестен: она могла ещё писать в картинку. Подтверждать её проявителю нельзя,
                    // ждать нечего — GPU потерян, движок оставит проявление в учтённой утечке.
                    return DevelopProgress::Lost(format!("опрос полосы: {failure:?}"));
                }
            }
        }
        // 2. Новые полосы — не больше `max_new_slices`.
        let mut submitted_now = 0;
        // Полос в полёте не больше `max_slices_in_flight`: остальные слоты очереди зарезервированы под копию RAW
        // следующего затвора и показ в кодер (`RESERVED_SUBMIT_SLOTS`).
        while job.failure.is_none()
            && job.slices_left > 0
            && submitted_now < max_new_slices
            && job.in_flight.len() < self.settings.max_slices_in_flight
        {
            let mut outcome = None;
            let developer = &mut self.developer;
            let submitted = self.submitter.submit(Vec::new(), |command_buffer| {
                outcome = Some(developer.record_next_slice(command_buffer)?);
                Ok::<(), PhotoError>(())
            });
            match submitted {
                Ok(work) => {
                    // Проявитель узнаёт записавшую работу (связь «буфер чтения ↔ работа» для `finish`).
                    if let Err(error) = self.developer.note_slice_submitted(&work) {
                        job.failure.get_or_insert_with(|| error.to_string());
                    }
                    job.in_flight.push_back(work);
                    job.slices_left = match outcome {
                        Some(SliceOutcome::Recorded { remaining }) => remaining,
                        Some(SliceOutcome::Finished) | None => 0,
                    };
                    submitted_now += 1;
                }
                Err(SubmitError::CapacityExhausted { .. }) => break,
                Err(error) => {
                    job.failure = Some(format!("отправка полосы: {error}"));
                }
            }
        }
        // 3. Итог: сбой (когда всё отправленное завершилось), готово или идёт.
        if job.in_flight.is_empty() {
            if let Some(failure) = job.failure.take() {
                if let Err(error) = self.developer.cancel() {
                    tracing::warn!(target: super::LOG_TARGET, %error, "проявление не отменилось после сбоя");
                }
                return DevelopProgress::Failed(failure);
            }
            if job.slices_left == 0 {
                return match self.developer.finish() {
                    Ok(photo) => DevelopProgress::Done(photo),
                    Err(error) => DevelopProgress::Failed(error.to_string()),
                };
            }
        }
        self.develop = Some(job);
        DevelopProgress::InFlight { submitted_now }
    }

    fn take_readback(&self, image: &mut DevelopedPhoto) -> Option<Rgba8Image> {
        image.take_host_rgba8()
    }

    fn present_start(&mut self, image: &DevelopedPhoto, surface: NativeSurface) -> Result<(), PresentRefusal> {
        self.present.start(
            &self.context,
            &mut self.submitter,
            image.gpu_image(),
            surface,
            self.workshop.as_deref(),
        )
    }

    fn present_poll(&mut self) -> PresentProgress {
        self.present.poll(&mut self.submitter)
    }

    fn declare_lost(&mut self) {
        self.lost = true;
        // Лизинг кадра камеры НЕ возвращается: GPU мог ещё читать буфер (учтённая утечка лизинга, решение 43).
        if let Some(job) = self.capture.take() {
            std::mem::forget(job.pending);
            drop(job.lease);
        }
        self.develop = None;
        // Цели, которых GPU ещё не видел: уничтожаются где угодно (здесь или на потоке мастерской).
        self.pending_develop = None;
        self.present.declare_lost();
        if let Some(importer) = self.importer.take() {
            std::mem::forget(importer);
        }
    }

    fn abandon(&mut self) -> AbandonReport {
        let drained = matches!(
            self.submitter.drain(self.settings.teardown_timeout),
            Ok(DrainOutcome::Drained)
        );
        if drained {
            self.settle_after_idle();
        } else {
            // GPU не освободился: всё, что он может ещё читать или писать, остаётся в утечке.
            tracing::error!(target: super::LOG_TARGET, "GPU снимков не освободился за срок: объекты оставлены в утечку");
            if let Some(job) = self.capture.take() {
                std::mem::forget(job.pending);
                drop(job.lease);
            }
            self.present.abandon_leaking();
            self.develop = None;
            // Цели, которых GPU ещё не видел: уничтожаются где угодно (здесь или на потоке мастерской).
            self.pending_develop = None;
            if let Some(importer) = self.importer.take() {
                std::mem::forget(importer);
            }
        }
        AbandonReport { gpu_idle: drained }
    }
}
