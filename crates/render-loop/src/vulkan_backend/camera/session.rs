//! `CameraSession` — состояние показа кадров камеры одной surface и порядок одного кадра.
//!
//! Один кадр показа (`draw_and_present`):
//! 1. слот кольца выдан ⇒ сабмиты до прошлого сабмита слота завершены ([`SubmissionTracker`]) —
//!    отпускаем ставшие ненужными импорты, семафоры ожидания и лизинги, собираем итоги слота
//!    (время GPU, статистика AWB → [`AwbStatisticsFeed`]);
//! 2. решение о содержимом (`prepare`): рисовальщик ISP — новый кадр через ISP, перекомпоновка
//!    прежнего изображения видоискателя или чёрный; отладочная мозаика — как в сессии 05;
//! 3. запись команд (`record`) по плану владения ([`OwnershipPlan`]);
//! 4. сабмит через `present_with_sync`: ждём acquire-fence нового кадра (стадия `ALL_COMMANDS`,
//!    см. `PresentSync`), при необходимости экспортируем release-fence;
//! 5. по итогу сабмита [`LeaseLedger`] решает, кому отдать fence. Кадр ISP поглощается своим же
//!    сабмитом (приём, проход, возврат) — его fence и отдаётся камере; кадр мозаики ждёт сабмита,
//!    который его заменит.
//!
//! Стенд V4 (`headroom_bench`, выключен по умолчанию) живёт здесь же: его кадр записывается в
//! сабмит нового кадра ISP, его итоги собираются со слота, а объекты освобождаются по тем же
//! завершённым сериям, что импорты.
//!
//! Ошибки разведены: сабмит не принят (GPU кадр не читал — лизинг нового кадра возвращается как
//! нетронутый) и ошибка показа после принятого сабмита (GPU читал — возврат только по fence).

mod collect;
mod prepare;
mod record;
mod slot;

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::raw_mosaic_preview::{MosaicPreviewParams, MosaicSource, RawMosaicPreview};
use gpu_vulkan::swapchain::{
    DisplayFrame, DisplayTarget, PresentOutcome as GpuPresentOutcome, PresentRequest, PresentSync, SubmitProgress,
    SwapchainError, SyncedPresent,
};
use gpu_vulkan::sync::{ExportableSemaphore, GpuReleaseFence, WaitSemaphore};
use isp_vulkan::viewfinder::ViewfinderGeometry;
use raw_frame_stream::{RawFrameLease, ViewfinderFrames};
use std::sync::Arc;

use self::slot::SlotFrame;
use super::export_recovery::{ExportRecovery, RecoveryStep};
use super::exposure::ExposureSlotFrame;
use super::exposure::{AutoExposureParts, AutoExposureSetupError, ExposureRunner};
use super::headroom_bench::{BenchSubmit, HeadroomBenchParts, HeadroomBenchRunner, HeadroomFrame};
use super::isp_params::IspParamsCache;
use super::lease::{FrameChange, LeaseLedger};
use super::ownership::OwnershipPlan;
use super::pipelines::{CameraPipelines, IspPresentation};
use super::settings::ViewfinderSettings;
use super::statistics_schedule::StatisticsDecision;
use super::submissions::SubmissionTracker;
use super::ui_state::UiOverlayFrame;
use super::viewfinder_stats::ViewfinderStats;
use super::white_balance::{FramePlan, WhiteBalanceParts, WhiteBalanceRunner};
use super::{CameraFrameImporter, CameraImportError, CameraImporterFactory, SubmissionSerial};
use crate::backend::FrameGpuTiming;
use crate::camera_diagnostics::{CameraStreamDiagnostics, PresentedCameraFrame};
use crate::config::ViewfinderToneConfig;
use crate::viewfinder::{AwbFrameIdentity, AwbStatisticsFeed, ViewfinderDiagnostics};

const LOG_TARGET: &str = "fasticamera::render::camera";

/// Почему кадр камеры не нарисован.
#[derive(Debug)]
pub enum CameraDrawError {
    /// Не удалось записать отладочную мозаику или чёрный кадр (проверка до записи команд).
    Record(GpuError),
    /// Ошибка показа/сабмита цели.
    Swapchain(SwapchainError),
}

/// Что рисуем в этом кадре.
enum Content {
    /// Отладочная мозаика кадра камеры (05).
    Mosaic {
        source: MosaicSource,
        params: MosaicPreviewParams,
    },
    /// Новый кадр через ISP видоискателя.
    IspNew {
        source: MosaicSource,
        geometry: ViewfinderGeometry,
        identity: AwbFrameIdentity,
        /// Решение AWB для кадра: белая, показ, нужна ли статистика.
        plan: FramePlan,
        /// Общее решение о статистике кадра (AWB + AE) и то, что нужно AE для замера слота.
        statistics: StatisticsDecision,
        exposure: Option<ExposureSlotFrame>,
        /// Тот же кадр через стенд V4 (`None` — стенд выключен или кадр мимо него).
        headroom: Option<HeadroomFrame>,
    },
    /// Прежнее изображение видоискателя без ISP и без импорта.
    Recompose,
    /// Чёрный кадр (потока нет или показывать нечего).
    Black,
}

/// Решение о содержимом кадра, принятое до записи команд.
struct Prepared {
    content: Content,
    change: FrameChange,
    /// Новый кадр, который читает этот сабмит (`Some` только при `Replace` и `Consume`).
    new_lease: Option<RawFrameLease>,
    /// Ожидание acquire-fence нового кадра (должно жить до конца сабмита на GPU).
    camera_wait: Option<WaitSemaphore>,
    /// Поток неактивен (для очистки кэша импортов).
    stream_inactive: bool,
}

/// Что записано в командный буфер (для счётчиков после сабмита).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Recorded {
    /// Отладочная мозаика.
    Mosaic,
    /// Новый кадр: `processed` — ISP записан, `shown` — изображение скомпоновано на экран.
    IspNew { processed: bool, shown: bool },
    /// Перекомпоновка: `shown` — изображение скомпоновано (иначе чёрный).
    Recompose { shown: bool },
    /// Чёрный.
    Black,
}

/// Итог освобождения сессии.
pub struct CameraShutdown {
    /// Счётчики сессии (в них же — сколько лизингов возвращено после простоя и сколько утекло).
    pub diagnostics: CameraStreamDiagnostics,
    /// Статистика видоискателя возвращается владельцу (бэкенду) — она копится за его жизнь.
    pub viewfinder: ViewfinderStats,
}

/// Что сессия получает от бэкенда при создании.
pub struct CameraSessionParts<'a> {
    /// Видоискатель потока кадров.
    pub frames: Arc<ViewfinderFrames>,
    /// Фабрика импортёра.
    pub make_importer: &'a CameraImporterFactory,
    /// Ёмкость истории показов.
    pub history_capacity: usize,
    /// Действующие настройки видоискателя.
    pub settings: ViewfinderSettings,
    /// Статистика видоискателя (вернётся в `CameraShutdown`).
    pub viewfinder: ViewfinderStats,
    /// Лента статистики AWB.
    pub awb_feed: AwbStatisticsFeed,
    /// Наш AWB: настройки, указания 3A, память белой сцены.
    pub white_balance: WhiteBalanceParts,
    /// Наш AE: настройки и ящики движка (`None` — вендорский AE, подтяжки и предложений нет).
    pub auto_exposure: Option<AutoExposureParts>,
    /// Стенд V4: настройки конвейера, слоты кольца, включён ли.
    pub headroom: HeadroomBenchParts,
    /// Ручки тона видоискателя (`viewfinder.tone`, уже проверены при старте).
    pub tone: ViewfinderToneConfig,
}

/// Показ кадров камеры для одной surface.
pub struct CameraSession {
    frames: Arc<ViewfinderFrames>,
    importer: Box<dyn CameraFrameImporter>,
    ledger: LeaseLedger<RawFrameLease>,
    tracker: SubmissionTracker,
    export: ExportableSemaphore,
    recovery: ExportRecovery,
    /// Источник кадра отладочной мозаики на экране: его нужно вернуть камере в сабмите, который заменит
    /// кадр. Живёт в кэше импортов (ёмкость кэша заведомо больше числа кадров на руках у показа).
    displayed_source: Option<MosaicSource>,
    /// Семафоры ожидания acquire-fence: живут, пока GPU не закончит сабмит, который их ждёт.
    waits_in_flight: Vec<(SubmissionSerial, WaitSemaphore)>,
    stats: CameraStreamDiagnostics,
    history_capacity: usize,
    settings: ViewfinderSettings,
    params: IspParamsCache,
    viewfinder: ViewfinderStats,
    awb_feed: AwbStatisticsFeed,
    /// Наш AWB (сессия 08): какая белая идёт в ISP, показывать ли кадр, нужна ли статистика.
    white: WhiteBalanceRunner,
    /// Наш AE (сессия 08a): остаточная подтяжка ISP, замер по статистике, предложения движку. `None` — выключен.
    exposure: Option<ExposureRunner>,
    /// В изображении видоискателя лежит актуальный кадр, его можно перекомпоновать. Сбрасывается
    /// чёрным кадром, остановкой потока, сменой рисовальщика и неудачей ISP.
    isp_image_shown: bool,
    /// Чей кадр ISP записал в слот (для привязки статистики AWB); `None` — кадр слота не дошёл до GPU.
    slot_frames: Vec<Option<SlotFrame>>,
    /// Номер кадра потока, чьи команды записаны в слот (`None` — сабмита не было): по нему время
    /// кадра на GPU привязывается к кадру, когда метки слота собраны.
    slot_frame_indices: Vec<Option<u64>>,
    /// Собранные времена кадров на GPU, ждущие потока рендера (`drain_frame_gpu_timings`).
    completed_gpu_timings: Vec<FrameGpuTiming>,
    /// Стенд «запас под видео» (V4).
    bench: HeadroomBenchRunner,
    context: GpuContext,
}

impl CameraSession {
    /// Создаёт сессию: импортёр (по фабрике бэкенда) и экспортируемый семафор.
    /// Ошибка возвращает статистику видоискателя владельцу.
    pub fn new(context: &GpuContext, parts: CameraSessionParts<'_>) -> Result<Self, Box<CameraSessionFailure>> {
        let CameraSessionParts {
            frames,
            make_importer,
            history_capacity,
            settings,
            viewfinder,
            awb_feed,
            white_balance,
            auto_exposure,
            headroom,
            tone,
        } = parts;
        let importer = match make_importer(context) {
            Ok(importer) => importer,
            Err(error) => {
                return Err(Box::new(CameraSessionFailure {
                    error: CameraSessionError::Importer(error),
                    viewfinder,
                }));
            }
        };
        let white = match WhiteBalanceRunner::new(white_balance) {
            Ok(white) => white,
            Err(error) => {
                return Err(Box::new(CameraSessionFailure {
                    error: CameraSessionError::WhiteBalance(error),
                    viewfinder,
                }));
            }
        };
        let exposure = match auto_exposure
            .map(|parts| ExposureRunner::new(parts, IspParamsCache::base_scene_middle_gray()))
            .transpose()
        {
            Ok(exposure) => exposure,
            Err(error) => {
                return Err(Box::new(CameraSessionFailure {
                    error: CameraSessionError::AutoExposure(error),
                    viewfinder,
                }));
            }
        };
        let export = match ExportableSemaphore::new(context) {
            Ok(export) => export,
            Err(error) => {
                return Err(Box::new(CameraSessionFailure {
                    error: CameraSessionError::ExportSemaphore(error),
                    viewfinder,
                }));
            }
        };
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
            settings,
            params: IspParamsCache::default().with_tone(tone),

            viewfinder,
            awb_feed,
            white,
            exposure,
            isp_image_shown: false,
            slot_frames: Vec::new(),
            slot_frame_indices: Vec::new(),
            completed_gpu_timings: Vec::new(),
            bench: HeadroomBenchRunner::new(context, headroom),
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

    /// Снимок диагностики видоискателя.
    pub fn viewfinder_diagnostics(&mut self) -> ViewfinderDiagnostics {
        let mut diagnostics =
            self.viewfinder
                .snapshot(&self.settings, self.bench.live_state(), self.white.diagnostics());
        diagnostics.auto_exposure = self.exposure.as_ref().map(ExposureRunner::diagnostics);
        diagnostics
    }

    /// Новые настройки (после команды). Смена рисовальщика сбрасывает «изображение актуально»:
    /// после мозаики изображение видоискателя устарело, до нового кадра — чёрный.
    pub fn apply_settings(&mut self, settings: ViewfinderSettings) {
        if settings.painter != self.settings.painter {
            self.isp_image_shown = false;
        }
        if settings.headroom != self.settings.headroom {
            self.bench.set_mode(settings.headroom, self.viewfinder.headroom_mut());
        }
        self.settings = settings;
    }

    /// Рисует и показывает один кадр. `frame` уже выдан целью и будет показан в любом случае
    /// (при ошибке записи — через сброс неотрисованного кадра). `frame_index` — номер кадра потока
    /// рендера: с ним позже выдаётся время кадра на GPU. `ui` — UI-проход кадра (источник, счётчики, время
    /// показа): без установленного источника ничего не меняет.
    pub fn draw_and_present(
        &mut self,
        display: &mut DisplayTarget,
        pipelines: &mut CameraPipelines,
        frame: DisplayFrame,
        request: PresentRequest,
        frame_index: u64,
        ui: &mut UiOverlayFrame<'_>,
    ) -> Result<GpuPresentOutcome, CameraDrawError> {
        let slot = frame.ring_slot();
        // 1. Слот свободен: GPU закончил его прошлый сабмит и все до него.
        let completed = self.tracker.note_slot_acquired(slot);
        self.retire_completed_work(completed);
        self.collect_slot(pipelines, slot, ui.state);
        // Метки слота собраны: следующий сабмит в слот (если будет) назначит ему новый кадр.
        self.note_slot_frame_index(slot, None);
        let serial = self.tracker.next_serial();
        self.viewfinder.note_display_color_space(frame.display_color_space());

        // 2. Что показывать.
        let Prepared {
            content,
            change,
            new_lease,
            camera_wait,
            stream_inactive,
        } = self.prepare_content(serial, frame.display_color_space());
        if stream_inactive {
            self.release_imports_when_gpu_is_done();
        }

        // 3. Запись команд.
        let plan = OwnershipPlan::for_change(change, self.displayed_source.is_some());
        let recorded = match self.record_content(pipelines, &frame, &content, plan, serial, ui) {
            Ok(recorded) => recorded,
            Err(error) => {
                // Мозаика и чёрный проверяют всё до записи команд: новый кадр GPU не касался.
                if let Some(lease) = new_lease {
                    lease.release_unused();
                }
                let abandoned = display.present_with_sync(frame, PresentRequest::default(), &PresentSync::none());
                if abandoned.submit == SubmitProgress::Submitted {
                    // Время такого сабмита — не время кадра видоискателя: к кадру не привязываем.
                    self.tracker.note_submitted(slot, serial);
                }
                return Err(CameraDrawError::Record(error));
            }
        };

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

        // 5. Итог сабмита.
        match submit {
            SubmitProgress::NotSubmitted => {
                // GPU кадра не читал: новый лизинг возвращается нетронутым, семафор ожидания не нужен.
                if let Some(lease) = new_lease {
                    lease.release_unused();
                }
                drop(camera_wait);
                self.bench
                    .finish_frame(slot, BenchSubmit::NotSubmitted, self.viewfinder.headroom_mut());
                self.note_slot_frame(slot, None);
                self.isp_image_shown = false;
                outcome.map_err(CameraDrawError::Swapchain)
            }
            SubmitProgress::Submitted => {
                self.tracker.note_submitted(slot, serial);
                self.note_slot_frame_index(slot, Some(frame_index));
                if let Some(wait) = camera_wait {
                    self.waits_in_flight.push((serial, wait));
                }
                self.note_submitted_content(&content, recorded, change, new_lease.as_ref(), &outcome);
                self.note_submitted_viewfinder(slot, &content, recorded);
                self.bench
                    .finish_frame(slot, BenchSubmit::Submitted(serial), self.viewfinder.headroom_mut());
                self.displayed_source = match (change, &content) {
                    (FrameChange::Replace, Content::Mosaic { source, .. }) => Some(*source),
                    (FrameChange::Clear | FrameChange::Consume, _) => None,
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
                // Кадр мозаики на экране ещё у нас: отдельным возвратом владения передаём его камере.
                if let Some(source) = self.displayed_source.take()
                    && let Err(error) = RawMosaicPreview::release_source_blocking(&self.context, &source)
                {
                    tracing::warn!(target: LOG_TARGET, %error, "возврат владения показанным кадром не записан");
                }
                let released = self.ledger.release_all_after_gpu_idle();
                self.stats.leases_released_after_idle += released as u64;
                self.importer.release_all_imports_gpu_idle();
                self.waits_in_flight.clear();
                self.bench.release_all_gpu_idle(self.viewfinder.headroom_mut());
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
        let Self {
            importer,
            export,
            waits_in_flight,
            viewfinder,
            bench,
            ..
        } = self;
        if gpu_wait.is_err() {
            // Импортёр, семафор экспорта, семафоры ожидания и объекты стенда V4 может ещё использовать
            // GPU: не уничтожаем.
            std::mem::forget(importer);
            std::mem::forget(export);
            std::mem::forget(waits_in_flight);
            std::mem::forget(bench);
        }
        CameraShutdown {
            diagnostics,
            viewfinder,
        }
    }

    /// Завершены сабмиты до `completed`: импорты, семафоры ожидания и лизинги, ждущие завершения своей
    /// серии, освобождаются (без ожиданий GPU); залипший семафор экспорта заменяется, если пора.
    fn retire_completed_work(&mut self, completed: SubmissionSerial) {
        self.importer.retire_completed(completed);
        self.waits_in_flight.retain(|(serial, _)| *serial > completed);
        self.stats.leases_released_by_completion += self.ledger.release_retiring_completed(completed) as u64;
        self.bench.retire_completed(completed, self.viewfinder.headroom_mut());
        self.maintain_export_semaphore(completed);
    }

    /// Счётчики видоискателя и «изображение актуально» по итогу принятого сабмита.
    fn note_submitted_viewfinder(&mut self, slot: usize, content: &Content, recorded: Recorded) {
        let slot_frame = match (content, recorded) {
            (
                Content::IspNew {
                    identity,
                    plan,
                    statistics,
                    exposure,
                    ..
                },
                Recorded::IspNew { processed: true, .. },
            ) => Some(SlotFrame {
                identity: *identity,
                stream: plan.stream,
                statistics: *statistics,
                exposure: *exposure,
            }),
            _ => None,
        };
        self.note_slot_frame(slot, slot_frame);
        let counters = self.viewfinder.counters_mut();
        match recorded {
            Recorded::IspNew { processed, shown } => {
                if processed {
                    counters.frames_processed += 1;
                    if !shown
                        && matches!(content, Content::IspNew { plan, .. } if plan.presentation == IspPresentation::HideUntilColorKnown)
                    {
                        counters.frames_hidden_cold_start += 1;
                    }
                }
                self.isp_image_shown = processed && shown;
            }
            Recorded::Recompose { shown } => {
                if shown {
                    counters.recomposed += 1;
                }
                self.isp_image_shown = shown;
            }
            Recorded::Mosaic | Recorded::Black => self.isp_image_shown = false,
        }
    }

    /// Счётчики и «последний показанный» по итогу принятого сабмита.
    fn note_submitted_content(
        &mut self,
        content: &Content,
        recorded: Recorded,
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
        let Some(present_id) = present_id else { return };
        let new_frame_shown = match recorded {
            Recorded::Mosaic => change == FrameChange::Replace,
            Recorded::IspNew { processed, shown } => processed && shown,
            Recorded::Recompose { .. } | Recorded::Black => false,
        };
        let redrawn = match recorded {
            Recorded::Mosaic => change == FrameChange::Keep && matches!(content, Content::Mosaic { .. }),
            Recorded::Recompose { shown } => shown,
            Recorded::IspNew { .. } | Recorded::Black => false,
        };
        match new_lease {
            Some(lease) if new_frame_shown => {
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
            _ if redrawn => self.stats.camera_frame_redraws += 1,
            _ => {}
        }
    }

    /// Итог экспорта fence: отдаём уходящие кадры или переходим в режим «ждать завершения серии».
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

/// Отказ создания сессии: причина и статистика видоискателя, которую надо вернуть владельцу.
pub struct CameraSessionFailure {
    /// Почему сессия не создана.
    pub error: CameraSessionError,
    /// Статистика видоискателя (владелец — бэкенд).
    pub viewfinder: ViewfinderStats,
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
    /// Настройки AWB отвергнуты.
    #[error(transparent)]
    WhiteBalance(#[from] super::white_balance::WhiteBalanceSetupError),
    /// Настройки AE отвергнуты.
    #[error(transparent)]
    AutoExposure(#[from] AutoExposureSetupError),
}
