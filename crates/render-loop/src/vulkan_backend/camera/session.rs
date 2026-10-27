//! `CameraSession` — состояние показа кадров камеры одной surface и порядок одного кадра.
//!
//! Один кадр показа (`draw_and_present`):
//! 1. слот кольца выдан ⇒ сабмиты до прошлого сабмита слота завершены ([`SubmissionTracker`]) —
//!    отпускаем ставшие ненужными импорты и семафоры ожидания;
//! 2. опрос видоискателя: новый кадр (импорт + acquire-fence), перерисовка прежнего или чёрный;
//! 3. запись предпросмотра в командный буфер кадра, сабмит через `present_with_sync`: ждём
//!    acquire-fence нового кадра, при необходимости сигналим экспортируемый семафор;
//! 4. по итогу сабмита [`LeaseLedger`] фиксирует смену показанного кадра, fence отдаётся ПРЕДЫДУЩИМ
//!    кадрам (никогда тому, что GPU только что прочитал).
//!
//! Ошибки разведены: сабмит не принят (GPU кадр не читал — лизинг нового кадра возвращается как
//! нетронутый) и ошибка показа после принятого сабмита (GPU читал — возврат только по fence).

use std::time::Instant;

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::raw_mosaic_preview::{
    FrameHandoff, MosaicPreviewParams, MosaicSource, PreviewFrame, RawMosaicPreview, SourceAcquire,
};
use gpu_vulkan::swapchain::{
    DisplayFrame, DisplayTarget, PresentOutcome as GpuPresentOutcome, PresentRequest, PresentSync, SubmitProgress,
    SwapchainError, SyncedPresent,
};
use gpu_vulkan::sync::{ExportableSemaphore, GpuReleaseFence, WaitSemaphore, import_wait_fence};
use raw_frame_stream::{RawFrameLease, ViewfinderFrames, ViewfinderPoll};
use std::sync::Arc;

use super::export_recovery::{ExportRecovery, RecoveryStep};
use super::lease::{FrameChange, LeaseLedger};
use super::ownership::{AcquireNew, OwnershipPlan, ReleaseDisplayed};
use super::passport::preview_params;
use super::submissions::SubmissionTracker;
use super::{CameraFrameImporter, CameraImportError, CameraImporterFactory, SubmissionSerial};
use crate::camera_diagnostics::{CameraStreamDiagnostics, PresentedCameraFrame};

const LOG_TARGET: &str = "fasticamera::render::camera";

/// Почему кадр камеры не нарисован.
#[derive(Debug)]
pub enum CameraDrawError {
    /// Не удалось записать предпросмотр (параметры отвергнуты до записи команд).
    Record(GpuError),
    /// Ошибка показа/сабмита цели.
    Swapchain(SwapchainError),
}

/// Что рисуем в этом кадре.
enum Content {
    /// Кадр камеры.
    Camera {
        source: MosaicSource,
        params: MosaicPreviewParams,
    },
    /// Чёрный кадр (потока нет или показывать нечего).
    Black,
}

/// Решение о содержимом кадра, принятое до записи команд.
struct Prepared {
    content: Content,
    change: FrameChange,
    /// Новый кадр, которому предстоит стать показанным (`Some` только при `Replace`).
    new_lease: Option<RawFrameLease>,
    /// Ожидание acquire-fence нового кадра (должно жить до конца сабмита на GPU).
    camera_wait: Option<WaitSemaphore>,
    /// Поток неактивен (для очистки кэша импортов).
    stream_inactive: bool,
}

/// Итог освобождения сессии.
pub struct CameraShutdown {
    /// Счётчики сессии (в них же — сколько лизингов возвращено после простоя и сколько утекло).
    pub diagnostics: CameraStreamDiagnostics,
}

/// Показ кадров камеры для одной surface.
pub struct CameraSession {
    frames: Arc<ViewfinderFrames>,
    importer: Box<dyn CameraFrameImporter>,
    ledger: LeaseLedger<RawFrameLease>,
    tracker: SubmissionTracker,
    export: ExportableSemaphore,
    recovery: ExportRecovery,
    /// Источник показанного кадра: его нужно вернуть камере в сабмите, который заменит кадр. Живёт в кэше
    /// импортов (ёмкость кэша заведомо больше числа кадров на руках у показа).
    displayed_source: Option<MosaicSource>,
    /// Семафоры ожидания acquire-fence: живут, пока GPU не закончит сабмит, который их ждёт.
    waits_in_flight: Vec<(SubmissionSerial, WaitSemaphore)>,
    stats: CameraStreamDiagnostics,
    history_capacity: usize,
    context: GpuContext,
}

impl CameraSession {
    /// Создаёт сессию: импортёр (по фабрике бэкенда) и экспортируемый семафор.
    pub fn new(
        context: &GpuContext,
        frames: Arc<ViewfinderFrames>,
        make_importer: &CameraImporterFactory,
        history_capacity: usize,
    ) -> Result<Self, CameraSessionError> {
        let importer = make_importer(context).map_err(CameraSessionError::Importer)?;
        let export = ExportableSemaphore::new(context).map_err(CameraSessionError::ExportSemaphore)?;
        Ok(Self {
            frames,
            importer,
            ledger: LeaseLedger::default(),
            tracker: SubmissionTracker::new(),
            export,
            recovery: ExportRecovery::new(),
            displayed_source: None,
            waits_in_flight: Vec::new(),
            stats: CameraStreamDiagnostics::default(),
            history_capacity,
            context: context.clone(),
        })
    }

    /// Снимок счётчиков с актуальными данными кэша импортов.
    pub fn diagnostics(&self) -> CameraStreamDiagnostics {
        let cache = self.importer.cache_counters();
        CameraStreamDiagnostics {
            export_semaphore_recreate_failures: self.recovery.recreate_failures(),
            import_cache_hits: cache.hits,
            import_cache_misses: cache.misses,
            ..self.stats.clone()
        }
    }

    /// Рисует и показывает один кадр. `frame` уже выдан целью и будет показан в любом случае
    /// (при ошибке записи — через сброс неотрисованного кадра).
    pub fn draw_and_present(
        &mut self,
        display: &mut DisplayTarget,
        preview: &mut RawMosaicPreview,
        frame: DisplayFrame,
        request: PresentRequest,
    ) -> Result<GpuPresentOutcome, CameraDrawError> {
        let slot = frame.ring_slot();
        // 1. Слот свободен: GPU закончил его прошлый сабмит и все до него.
        let completed = self.tracker.note_slot_acquired(slot);
        self.importer.retire_completed(completed);
        self.waits_in_flight.retain(|(serial, _)| *serial > completed);
        // Без ожиданий GPU: лизинги, чья серия возврата уже завершена, уходят камере сразу.
        self.stats.leases_released_by_completion += self.ledger.release_retiring_completed(completed) as u64;
        self.maintain_export_semaphore(completed);
        let serial = self.tracker.next_serial();

        // 2. Что показывать.
        let Prepared {
            content,
            change,
            new_lease,
            camera_wait,
            stream_inactive,
        } = self.prepare_content(serial);
        if stream_inactive {
            self.release_imports_when_gpu_is_done();
        }

        // 3. Запись команд. План владения: приём нового кадра — только в первом сабмите, читающем его;
        //    возврат прежнего — в сабмите, который его заменяет; перерисовка владения не трогает.
        let plan = OwnershipPlan::for_change(change, self.displayed_source.is_some());
        let release_previous = match plan.release_displayed {
            ReleaseDisplayed::Yes => self.displayed_source,
            ReleaseDisplayed::No => None,
        };
        let recorded = match &content {
            Content::Camera { source, params } => {
                let handoff = FrameHandoff {
                    acquire_source: match plan.acquire_new {
                        AcquireNew::Yes => SourceAcquire::BeforeRead,
                        AcquireNew::No => SourceAcquire::AlreadyAcquired,
                    },
                    release_previous,
                };
                preview.record(
                    frame.command_buffer(),
                    frame.pattern_target(),
                    &PreviewFrame {
                        source,
                        params,
                        transform: frame.transform(),
                        frame_slot: slot,
                        handoff,
                    },
                )
            }
            Content::Black => preview.record_black(frame.command_buffer(), frame.pattern_target(), release_previous),
        };
        if let Err(error) = recorded {
            // Команды камеры не записаны (проверка идёт до записи): новый кадр GPU не касался.
            if let Some(lease) = new_lease {
                lease.release_unused();
            }
            let abandoned = display.present_with_sync(frame, PresentRequest::default(), &PresentSync::none());
            if abandoned.submit == SubmitProgress::Submitted {
                self.tracker.note_submitted(slot, serial);
            }
            return Err(CameraDrawError::Record(error));
        }

        // 4. Сабмит и показ.
        let wants_release_fence = self.ledger.needs_release_fence(change) && self.recovery.can_export();
        let mut sync = PresentSync::none();
        if let Some(wait) = &camera_wait {
            sync = sync.wait_for_camera_acquire(wait);
        }
        if wants_release_fence {
            sync = sync.export_release_after_submit(&self.export);
        }
        let synced = display.present_with_sync(frame, request, &sync);
        let SyncedPresent {
            submit,
            release_signal,
            outcome,
        } = synced;
        // Токен экспорта потребляем сразу: так заимствование `self.export` кончается до правки полей.
        let exported = release_signal.map(gpu_vulkan::sync::PendingReleaseSignal::export);

        match submit {
            SubmitProgress::NotSubmitted => {
                // GPU кадра не читал: новый лизинг возвращается нетронутым, семафор ожидания не нужен.
                if let Some(lease) = new_lease {
                    lease.release_unused();
                }
                drop(camera_wait);
                outcome.map_err(CameraDrawError::Swapchain)
            }
            SubmitProgress::Submitted => {
                self.tracker.note_submitted(slot, serial);
                if let Some(wait) = camera_wait {
                    self.waits_in_flight.push((serial, wait));
                }
                self.note_submitted_content(&content, change, new_lease.as_ref(), &outcome);
                self.displayed_source = match (change, &content) {
                    (FrameChange::Replace, Content::Camera { source, .. }) => Some(*source),
                    (FrameChange::Clear, _) => None,
                    _ => self.displayed_source,
                };
                self.ledger.commit_submitted(change, new_lease, serial);
                if let Some(export_result) = exported {
                    self.finish_release_export(export_result, serial);
                }
                outcome.map_err(CameraDrawError::Swapchain)
            }
        }
    }

    /// Освобождение сессии при отсоединении surface. Возвращает итоговые счётчики.
    ///
    /// GPU дождался ⇒ все лизинги возвращаются камере с сигнальным fence, импорты уничтожаются.
    /// GPU не освободился вовремя ⇒ лизинги НАМЕРЕННО выбрасываются (учтённая утечка, решение 43), импортёр и
    /// семафоры тоже оставляются в утечке: GPU может их ещё использовать.
    pub fn shutdown(mut self, display: &DisplayTarget) -> CameraShutdown {
        let gpu_wait = display.wait_for_submitted_frames();
        match &gpu_wait {
            Ok(()) => {
                self.tracker.note_gpu_idle();
                // Кадр на экране ещё у нас: отдельным возвратом владения передаём его камере до возврата лизинга.
                if let Some(source) = self.displayed_source.take()
                    && let Err(error) = RawMosaicPreview::release_source_blocking(&self.context, &source)
                {
                    tracing::warn!(target: LOG_TARGET, %error, "возврат владения показанным кадром не записан");
                }
                let released = self.ledger.release_all_after_gpu_idle();
                self.stats.leases_released_after_idle += released as u64;
                self.importer.release_all_imports_gpu_idle();
                self.waits_in_flight.clear();
            }
            Err(error) => {
                let leaked = self.ledger.leak_all();
                self.stats.leases_leaked_on_teardown += leaked as u64;
                tracing::error!(
                    target: LOG_TARGET,
                    %error,
                    leaked,
                    "GPU не освободился: лизинги кадров камеры намеренно выброшены (учтённая утечка), импорты оставлены"
                );
            }
        }
        let diagnostics = self.diagnostics();
        if gpu_wait.is_err() {
            // Импортёр, семафор экспорта и семафоры ожидания может ещё использовать GPU: не уничтожаем.
            let Self {
                importer,
                export,
                waits_in_flight,
                ..
            } = self;
            std::mem::forget(importer);
            std::mem::forget(export);
            std::mem::forget(waits_in_flight);
        }
        CameraShutdown { diagnostics }
    }

    /// Решает, что рисовать в этом кадре (опрос видоискателя, импорт, fence).
    fn prepare_content(&mut self, serial: SubmissionSerial) -> Prepared {
        match self.frames.poll_latest(Instant::now()) {
            ViewfinderPoll::NewFrame(lease) => match self.prepare_new_frame(&lease, serial) {
                Ok((source, params, wait)) => {
                    self.stats.camera_frames_imported += 1;
                    Prepared {
                        content: Content::Camera { source, params },
                        change: FrameChange::Replace,
                        new_lease: Some(lease),
                        camera_wait: Some(wait),
                        stream_inactive: false,
                    }
                }
                Err(error) => {
                    self.stats.camera_frame_failures += 1;
                    tracing::warn!(target: LOG_TARGET, %error, "кадр камеры не подготовлен: возвращён нетронутым");
                    lease.release_unused();
                    self.prepare_redraw(serial)
                }
            },
            ViewfinderPoll::NoNewFrame => self.prepare_redraw(serial),
            ViewfinderPoll::StreamInactive => Prepared {
                content: Content::Black,
                change: FrameChange::Clear,
                new_lease: None,
                camera_wait: None,
                stream_inactive: true,
            },
        }
    }

    /// Перерисовка показанного кадра (или чёрный, если показывать нечего).
    fn prepare_redraw(&mut self, serial: SubmissionSerial) -> Prepared {
        let content = match self.ledger.displayed() {
            Some(lease) => match preview_params(lease.passport())
                .map_err(|error| error.to_string())
                .and_then(|params| {
                    self.importer
                        .import_frame(lease, serial)
                        .map(|source| Content::Camera { source, params })
                        .map_err(|error| error.to_string())
                }) {
                Ok(content) => content,
                Err(error) => {
                    self.stats.camera_frame_failures += 1;
                    tracing::warn!(target: LOG_TARGET, %error, "перерисовать показанный кадр не удалось: чёрный кадр");
                    Content::Black
                }
            },
            None => Content::Black,
        };
        Prepared {
            content,
            change: FrameChange::Keep,
            new_lease: None,
            camera_wait: None,
            stream_inactive: false,
        }
    }

    /// Параметры, источник и семафор ожидания для НОВОГО кадра.
    fn prepare_new_frame(
        &mut self,
        lease: &RawFrameLease,
        serial: SubmissionSerial,
    ) -> Result<(MosaicSource, MosaicPreviewParams, WaitSemaphore), NewFrameError> {
        let params = preview_params(lease.passport()).map_err(|error| NewFrameError(error.to_string()))?;
        let source = self.importer.import_frame(lease, serial).map_err(NewFrameError::from)?;
        let fence = lease
            .acquire_fence()
            .map_err(|error| NewFrameError(format!("acquire-fence кадра: {error}")))?;
        let wait = import_wait_fence(&self.context, fence)
            .map_err(|error| NewFrameError(format!("импорт acquire-fence в семафор: {}", error.reason)))?;
        Ok((source, params, wait))
    }

    /// Счётчики и «последний показанный» по итогу принятого сабмита.
    fn note_submitted_content(
        &mut self,
        content: &Content,
        change: FrameChange,
        new_lease: Option<&RawFrameLease>,
        outcome: &Result<GpuPresentOutcome, SwapchainError>,
    ) {
        let present_id = match outcome {
            Ok(GpuPresentOutcome::Presented { present_id } | GpuPresentOutcome::Suboptimal { present_id }) => {
                Some(*present_id)
            }
            Ok(GpuPresentOutcome::NeedsRecreate) | Err(_) => None,
        };
        match (change, new_lease, present_id) {
            (FrameChange::Replace, Some(lease), Some(present_id)) => {
                let passport = lease.passport();
                self.stats.note_new_frame_presented(
                    PresentedCameraFrame {
                        sequence: passport.sequence.0,
                        sensor_timestamp_nanos: passport.sensor_timestamp.as_nanos(),
                        frame_number: passport.capture.details().map(|details| details.result.frame_number.0),
                        present_id,
                    },
                    self.history_capacity,
                );
            }
            (FrameChange::Keep, _, Some(_)) if matches!(content, Content::Camera { .. }) => {
                self.stats.camera_frame_redraws += 1;
            }
            _ => {}
        }
    }

    /// Итог экспорта fence: отдаём уходящие кадры или переходим в режим «ждать простоя».
    fn finish_release_export(&mut self, exported: Result<GpuReleaseFence, GpuError>, serial: SubmissionSerial) {
        match exported {
            Ok(fence) => {
                self.stats.release_fences_exported += 1;
                let waiting = self.ledger.release_retiring_after_fence(fence);
                if waiting > 0 {
                    tracing::warn!(target: LOG_TARGET, waiting, "часть лизингов ждёт следующего fence");
                }
            }
            Err(error) => {
                self.stats.release_export_failures += 1;
                self.recovery.note_export_failed(serial);
                tracing::warn!(
                    target: LOG_TARGET,
                    %error,
                    "экспорт release-fence не удался: лизинги вернём по завершению серий сабмитов, без fence раньше не отпускаем"
                );
            }
        }
    }

    /// Заменяет залипший семафор экспорта (без ожиданий GPU: замена разрешена, когда завершён сабмит,
    /// сигналивший старый семафор; неудачи повторяются с паузой в кадрах, см. `export_recovery`).
    fn maintain_export_semaphore(&mut self, completed: SubmissionSerial) {
        if self.recovery.on_frame(completed) != RecoveryStep::TryRecreate {
            return;
        }
        match ExportableSemaphore::new(&self.context) {
            Ok(fresh) => {
                // Старый семафор уничтожается здесь: GPU закончил его сигнал (completed >= signaled_by).
                self.export = fresh;
                self.recovery.note_recreate_result(true);
            }
            Err(error) => {
                self.recovery.note_recreate_result(false);
                tracing::warn!(target: LOG_TARGET, %error, "новый семафор экспорта не создан: повтор позже");
            }
        }
    }

    /// Поток неактивен, показанного кадра нет и GPU точно закончил всё отправленное — импорты не нужны.
    fn release_imports_when_gpu_is_done(&mut self) {
        if self.ledger.is_empty() && self.tracker.everything_completed() && self.importer.imports_alive() > 0 {
            self.importer.release_all_imports_gpu_idle();
        }
    }
}

/// Ошибка подготовки нового кадра (причина уже в тексте).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct NewFrameError(String);

impl From<CameraImportError> for NewFrameError {
    fn from(error: CameraImportError) -> Self {
        Self(error.to_string())
    }
}

/// Не удалось создать сессию показа камеры.
#[derive(Debug, thiserror::Error)]
pub enum CameraSessionError {
    /// Фабрика импортёра отказала.
    #[error("импортёр кадров камеры: {0}")]
    Importer(CameraImportError),
    /// Экспортируемый семафор не создан.
    #[error("экспортируемый семафор release-fence: {0}")]
    ExportSemaphore(GpuError),
}
