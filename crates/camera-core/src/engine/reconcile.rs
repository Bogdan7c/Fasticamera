//! Доведение реального состояния до желаемого.
//!
//! [`CameraEngine::reconcile`] вызывается после каждой команды, события или таймера.
//! Он сравнивает [`Desired`] с тем, что реально открыто, и делает недостающие шаги
//! синхронными вызовами бэкенда по порядку: открыть устройство → сконфигурировать сессию →
//! поставить повторяющийся запрос (или снять его). Шаг, который не удался, записывает
//! состояние ошибки и возвращает [`Halt`] — дальше не идём.
//!
//! Идемпотентность: когда всё уже сделано, `reconcile` ничего не вызывает и не пишет событий.
//! Поэтому повторный `open` и `apply_controls` без изменений не трогают бэкенд.

use std::time::Instant;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::capabilities::CameraDescriptor;
use camera_contract::ids::CameraId;
use camera_contract::stream::{ResolvedStream, StreamMode};

use super::frame_rate_fallback::fallback_for;
use super::{CameraEngine, Halt, LOG_TARGET, LiveSession, OpenDevice, RepeatingRequest};
use crate::backend::{ConfigureFailure, DeviceGeneration, OpenFailure, RequestFailure, SessionGeneration};
use crate::event::CameraEvent;
use crate::lens_status::LensSwitchFailure;
use crate::state::{CameraState, ClosedReason};
use crate::status::AppPresence;

/// Что должно быть открыто сейчас: камера, поток под неё и запрос.
pub(super) struct Plan {
    pub(super) camera: CameraId,
    pub(super) stream: Option<ResolvedStream>,
}

impl CameraEngine {
    /// Доведение заперто: окончательная ошибка, фон, ожидание повода повтора, нет каталога.
    pub(super) fn reconcile_is_blocked(&self) -> bool {
        self.poisoned
            || self.is_fatal()
            || self.presence == AppPresence::Background
            || self.recovery.waiting
            || !self.catalog_loaded
    }

    /// Разрешить желаемое состояние в план для текущего каталога.
    fn current_plan(&self) -> Option<Plan> {
        let target = self.desired.target.as_ref()?;
        let descriptor = match self.catalog.resolve(target) {
            Ok(descriptor) => descriptor,
            Err(unavailable) => {
                tracing::warn!(target: LOG_TARGET, %unavailable, "желаемая камера пропала из каталога");
                return None;
            }
        };
        let stream = self.resolved_stream_for(descriptor);
        Some(Plan {
            camera: descriptor.id.clone(),
            stream,
        })
    }

    /// Поток, который желаемый режим даёт для камеры `descriptor` (эффективная частота камеры,
    /// самый большой RAW). Один источник и для основной, и для тёплой камеры: иначе сессия
    /// тёплой после повышения отличалась бы от плана основной, и её пришлось бы пересоздавать.
    pub(super) fn resolved_stream_for(&self, descriptor: &CameraDescriptor) -> Option<ResolvedStream> {
        self.desired.mode.and_then(|requested| {
            // Эффективный режим этой камеры: запрошенная частота, сниженная до её потолка.
            let mode = self.frame_rate_limits.effective_mode(&descriptor.id, requested);
            let option = descriptor.capabilities.largest_raw_stream(mode.raw_format)?;
            Some(resolved_stream(&descriptor.id, &mode, option.size))
        })
    }

    /// Довести реальное состояние до желаемого.
    pub(super) fn reconcile(&mut self, now: Instant) {
        if self.reconcile_is_blocked() {
            return;
        }
        let Some(plan) = self.current_plan() else {
            self.release_unwanted_camera();
            return;
        };
        if self.drive_to_plan(&plan, now).is_ok() {
            self.complete_recovery_if_pending();
        }
        // Тёплые — после основной: они никогда не задерживают и не ломают её (сбои тёплой
        // остаются в её ячейке).
        self.sync_standby(now);
        // Новая камера отказала посреди смены объектива: цель уже возвращена, довести до неё.
        self.run_pending_lens_revert(now);
    }

    /// Цель вернули на другую камеру посреди перестройки: показанное `Reconfiguring` должно
    /// называть поток новой цели, а не отменённой. Флаг `reconfiguring` остаётся у `reconcile`.
    pub(super) fn retarget_reconfiguring_state(&mut self) {
        if !self.reconfiguring {
            return;
        }
        if let Some(Plan {
            stream: Some(target), ..
        }) = self.current_plan()
        {
            self.transition(CameraState::Reconfiguring { target });
        }
    }

    /// Ничего не желаем: устройство закрыть, состояние — «закрыто».
    fn release_unwanted_camera(&mut self) {
        self.release_platform();
        self.reconfiguring = false;
        self.transition(CameraState::Closed {
            reason: ClosedReason::NotRequested,
        });
    }

    fn drive_to_plan(&mut self, plan: &Plan, now: Instant) -> Result<(), Halt> {
        // Смена камеры вторым способом: тёплая камера уже стримит — обмен ролей вместо close/open.
        self.step_promote_standby(plan);
        self.step_release_wrong_device(plan);
        self.step_open_device(plan, now)?;
        self.step_session(plan, now)?;
        self.step_repeating_request(now)
    }

    /// Открыта другая камера — отпустить; шёл поток — это перестройка.
    fn step_release_wrong_device(&mut self, plan: &Plan) {
        let is_wrong = self.device.as_ref().is_some_and(|device| device.camera != plan.camera);
        if !is_wrong {
            return;
        }
        let was_streaming = self.session.as_ref().is_some_and(|session| session.repeating.is_some());
        self.release_platform();
        self.begin_reconfiguring_if(was_streaming, plan);
    }

    /// Поток шёл и перестраивается: показать `Reconfiguring` один раз, без промежуточных шагов.
    fn begin_reconfiguring_if(&mut self, was_streaming: bool, plan: &Plan) {
        if !was_streaming {
            return;
        }
        if let Some(target) = &plan.stream {
            self.reconfiguring = true;
            self.transition(CameraState::Reconfiguring { target: target.clone() });
        }
    }

    /// Промежуточное состояние, которое во время перестройки не показывается.
    fn transition_unless_reconfiguring(&mut self, state: CameraState) {
        if !self.reconfiguring {
            self.transition(state);
        }
    }

    fn step_open_device(&mut self, plan: &Plan, now: Instant) -> Result<(), Halt> {
        if self.device.is_some() {
            return Ok(());
        }
        self.transition_unless_reconfiguring(CameraState::Opening {
            camera: plan.camera.clone(),
        });
        self.counters.devices_opened += 1;
        self.availability.note_open_attempt();
        let generation = DeviceGeneration(self.counters.devices_opened);
        match self.backend.open_device(&plan.camera, generation) {
            Ok(()) => {
                self.device = Some(OpenDevice {
                    camera: plan.camera.clone(),
                    generation,
                });
                self.transition_unless_reconfiguring(CameraState::Opened {
                    camera: plan.camera.clone(),
                });
                Ok(())
            }
            Err(OpenFailure::Unavailable { reason }) => Err(self.fail_recoverable(reason, now)),
            Err(OpenFailure::Backend(failure)) => Err(self.fail_with_backend_failure(&failure, now)),
        }
    }

    /// Сессия должна совпадать с желаемым потоком: лишнюю закрыть, недостающую создать.
    fn step_session(&mut self, plan: &Plan, now: Instant) -> Result<(), Halt> {
        self.close_session_if_outdated(plan);
        let Some(stream) = &plan.stream else {
            return Ok(());
        };
        if self.session.is_some() {
            return Ok(());
        }
        let Some(requested) = self.desired.mode else {
            return Ok(());
        };
        // Поток плана уже построен по эффективному режиму; режим для настройки берём от него же.
        let mode = self.frame_rate_limits.effective_mode(&plan.camera, requested);
        if mode != requested {
            self.events.push(CameraEvent::StreamLimitedByCamera {
                camera: plan.camera.clone(),
                requested,
                effective: mode,
            });
        }
        self.configure_session(stream, mode, now)
    }

    fn close_session_if_outdated(&mut self, plan: &Plan) {
        let outdated = match (&self.session, &plan.stream) {
            (Some(session), Some(stream)) => &session.configured.stream != stream,
            (Some(_), None) => true,
            (None, _) => false,
        };
        if !outdated {
            return;
        }
        let was_streaming = self.session.as_ref().is_some_and(|session| session.repeating.is_some());
        self.backend.close_session();
        self.session = None;
        self.end_three_a_stream();
        self.begin_reconfiguring_if(was_streaming, plan);
    }

    fn configure_session(&mut self, stream: &ResolvedStream, mode: StreamMode, now: Instant) -> Result<(), Halt> {
        self.transition_unless_reconfiguring(CameraState::Configuring { stream: stream.clone() });
        self.counters.sessions_configured += 1;
        let generation = SessionGeneration(self.counters.sessions_configured);
        match self.backend.configure_session(stream, generation) {
            Ok(configured) => {
                self.cadence.begin_session(generation, mode.frame_rate.frame_duration());
                self.carry_exposure_memory_to(&stream.camera);
                self.events.push(CameraEvent::StreamConfigured {
                    stream: configured.clone(),
                });
                self.session = Some(LiveSession {
                    generation,
                    configured,
                    repeating: None,
                });
                Ok(())
            }
            Err(ConfigureFailure::DeviceDisconnected) => {
                Err(self.fail_recoverable(CameraUnavailableReason::Evicted, now))
            }
            Err(ConfigureFailure::Backend(failure)) => Err(self.fail_with_backend_failure(&failure, now)),
            Err(rejection @ (ConfigureFailure::Unsupported(_) | ConfigureFailure::RejectedByHal { .. })) => {
                match fallback_for(mode, &rejection, self.config.stream.frame_rate_fallback) {
                    Some(fallback) => self.retry_with_fallback(stream, mode, fallback, rejection, now),
                    None => Err(self.reject_stream_mode(mode, rejection)),
                }
            }
        }
    }

    /// Откат на запасную частоту (решение 23): желаемый режим заменяется на `fallback`,
    /// наружу уходит `StreamDegraded` и в лог — предупреждение, затем одна повторная настройка.
    /// Цикла нет: `fallback_for` откатывает только строго вниз, а у запасного режима частота
    /// уже не выше запасной. Если и запасной режим не прошёл — обычный отказ по нему.
    fn retry_with_fallback(
        &mut self,
        stream: &ResolvedStream,
        requested: StreamMode,
        fallback: StreamMode,
        failure: ConfigureFailure,
        now: Instant,
    ) -> Result<(), Halt> {
        tracing::warn!(
            target: LOG_TARGET,
            %failure,
            requested_fps = requested.frame_rate.frames_per_second(),
            fallback_fps = fallback.frame_rate.frames_per_second(),
            "частота недостижима, откат на запасную"
        );
        // Желаемый режим остаётся запрошенным: потолок запоминается за этой камерой, и
        // эффективный режим (план, темп, status) для неё теперь запасной. Другая камера
        // по-прежнему получит запрошенную частоту.
        self.frame_rate_limits
            .remember_ceiling(stream.camera.clone(), fallback.frame_rate);
        self.events.push(CameraEvent::StreamDegraded {
            requested,
            fallback,
            failure,
        });
        let degraded_stream = ResolvedStream {
            frame_rate: fallback.frame_rate,
            ..stream.clone()
        };
        self.configure_session(&degraded_stream, fallback, now)
    }

    /// Режим невозможен на этой камере: не недоступность, повторять не нужно. Желаемые режим
    /// и поток сбрасываются, устройство остаётся открытым.
    fn reject_stream_mode(&mut self, mode: StreamMode, failure: ConfigureFailure) -> Halt {
        // Идёт смена объектива: виновата новая камера, а желаемые режим и поток нужны камере, на
        // которую возвращаемся, — их нельзя стирать. Сначала возврат, потом обычная ветка.
        if let Some(halt) = self.try_revert_lens_switch(LensSwitchFailure::StreamRejected {
            failure: failure.clone(),
        }) {
            tracing::warn!(target: LOG_TARGET, %failure, "новая камера не приняла режим потока");
            self.events
                .push(CameraEvent::StreamConfigurationFailed { mode, failure });
            return halt;
        }
        tracing::warn!(target: LOG_TARGET, %failure, "режим потока не сконфигурировался");
        self.desired.mode = None;
        self.desired.streaming = None;
        self.reconfiguring = false;
        self.events
            .push(CameraEvent::StreamConfigurationFailed { mode, failure });
        if let Some(device) = &self.device {
            let camera = device.camera.clone();
            self.transition(CameraState::Opened { camera });
        }
        // Дальше идти нечего: желаемое выполнено в том смысле, что требований к потоку больше нет.
        self.complete_recovery_if_pending();
        Halt
    }

    /// Повторяющийся запрос должен совпадать с желаемым (поток и план управления 3A).
    ///
    /// План строится из намерения 3A на каждом `reconcile` (чистая функция исполнителей);
    /// запрос перестраивается, только если план отличается от уже применённого.
    fn step_repeating_request(&mut self, now: Instant) -> Result<(), Halt> {
        let Some(session) = &self.session else {
            return self.settle_opened();
        };
        let current = session.repeating;
        let Some(intent) = self.desired.streaming else {
            return match current {
                Some(_) => self.stop_repeating(now),
                None => self.settle_opened(),
            };
        };
        let planned = self.plan_three_a();
        let wanted = RepeatingRequest {
            intent,
            controls: planned.controls,
        };
        if current != Some(wanted) {
            self.start_repeating(wanted, now)?;
        }
        self.finish_planned_request(planned, now)
    }

    /// Повторяющегося запроса нет: состояние «устройство открыто».
    fn settle_opened(&mut self) -> Result<(), Halt> {
        self.reconfiguring = false;
        if let Some(device) = &self.device {
            let camera = device.camera.clone();
            self.transition(CameraState::Opened { camera });
        }
        Ok(())
    }

    fn start_repeating(&mut self, request: RepeatingRequest, now: Instant) -> Result<(), Halt> {
        match self.backend.set_repeating_request(request.intent, &request.controls) {
            Ok(()) => {
                let Some(session) = &mut self.session else {
                    return Ok(());
                };
                session.repeating = Some(request);
                let stream = session.configured.clone();
                self.reconfiguring = false;
                self.transition(CameraState::Streaming { stream });
                // Кадры пойдут на экран: объектив с этого момента «показан» (первый показ — не смена).
                self.note_lens_stream_started();
                Ok(())
            }
            Err(failure) => Err(self.fail_from_request_failure(failure, now)),
        }
    }

    fn stop_repeating(&mut self, now: Instant) -> Result<(), Halt> {
        match self.backend.stop_repeating() {
            Ok(()) => {
                if let Some(session) = &mut self.session {
                    session.repeating = None;
                }
                self.end_three_a_stream();
                self.settle_opened()
            }
            Err(failure) => Err(self.fail_from_request_failure(failure, now)),
        }
    }

    /// Отказ повторяющегося запроса: отключение — вытеснение, остальное — сбой устройства.
    pub(super) fn fail_from_request_failure(&mut self, failure: RequestFailure, now: Instant) -> Halt {
        match failure {
            RequestFailure::DeviceDisconnected => self.fail_recoverable(CameraUnavailableReason::Evicted, now),
            RequestFailure::NoSession => {
                // Бэкенд говорит «сессии нет», а движок считал, что есть: рассинхрон, лечится переоткрытием.
                tracing::error!(target: LOG_TARGET, "рассинхрон движка и бэкенда: нет активной сессии");
                self.fail_recoverable(CameraUnavailableReason::DeviceFailure, now)
            }
            RequestFailure::Backend(failure) => self.fail_with_backend_failure(&failure, now),
        }
    }

    /// Недоступность по ошибке устройства (код HAL → причина).
    pub(super) fn fail_from_device_error(&mut self, error: crate::backend::DeviceErrorKind, now: Instant) {
        let reason = Self::unavailable_reason_for(error);
        let _ = self.fail_recoverable(reason, now);
    }
}

/// Режим, разрешённый для камеры: размер буфера — самый большой публичный RAW этого формата.
fn resolved_stream(
    camera: &CameraId,
    mode: &StreamMode,
    buffer_size: camera_contract::geometry::PixelSize,
) -> ResolvedStream {
    ResolvedStream {
        camera: camera.clone(),
        raw_format: mode.raw_format,
        buffer_size,
        readout: mode.readout,
        frame_rate: mode.frame_rate,
    }
}
