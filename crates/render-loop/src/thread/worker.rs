//! Тело потока рендера: цикл «ящик -> машина состояний -> бэкенд -> кадр».
//!
//! Владение: этот объект единолично владеет бэкендом, часами кадров, приёмником ящика,
//! машиной состояний и surface, ещё не переданной бэкенду. Снаружи видны только
//! [`RenderStatus`] (копия под коротким замком) и почтовый ящик.
//!
//! Гарантия по квитанциям: КАЖДЫЙ `SurfaceDestroyed`, попавший в ящик, заканчивается
//! либо подтверждением (после отпускания окна), либо уничтожением запроса
//! (`DroppedWithoutConfirmation`) — даже при панике бэкенда или остановке потока.

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Instant;

use shell_contract::commands::{FrameRate, ScreenWake, ShellCommands};
use shell_contract::performance_hint::PerformanceHintService;
use shell_contract::surface::{NativeSurface, SurfaceGeneration, SurfaceReleaseRequest, SurfaceSize};

use super::adpf::{AdpfReporter, PresentedFrameWork};
use super::frame_clock::{FrameClock, FrameTick, FrameWait};
use super::monotonic_clock::{MonotonicTimer, SystemMonotonicTimer};
use super::present_target::TickPlan;
use super::present_target_stats::PresentTargetTracker;
use super::status::{BackendHealth, FaultInfo, FaultKind, RenderState, RenderStatus};
use crate::backend::{BackendDiagnostics, BackendError, DisplayBackend, DisplayInfo, FrameTiming, PresentOutcome};
use crate::config::RenderLoopConfig;
use crate::mailbox::{InboxEvent, RenderInboxReceiver, StampedEvent};
use crate::pacing::{PacingParams, PacingTracker};
use crate::surface_state::{SurfaceAction, SurfacePhase, SurfaceStateMachine};
use crate::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};

mod frame_accounting;

const LOG_TARGET: &str = "fasticamera::render";

/// Квитанция вместе с моментом, когда оболочка её отправила.
struct PendingRelease {
    request: SurfaceReleaseRequest,
    enqueued_at: Instant,
}

/// Данные одного события, которые действия могут потребить.
#[derive(Default)]
struct ActionContext {
    /// Новая surface из `SurfaceCreated`, пока она не сохранена в `unattached_surface`.
    incoming_surface: Option<NativeSurface>,
    /// Квитанция из `SurfaceDestroyed`.
    release: Option<PendingRelease>,
}

pub(super) struct RenderWorker<B: DisplayBackend, C: FrameClock> {
    config: RenderLoopConfig,
    backend: B,
    clock: C,
    receiver: RenderInboxReceiver,
    machine: SurfaceStateMachine,
    /// Surface, принятая из ящика, но ещё не отданная бэкенду (ждёт размера).
    unattached_surface: Option<NativeSurface>,
    pacing: PacingTracker,
    /// Выбор линии кадра, план позднего захвата и их учёт (сессия 07).
    present_target: PresentTargetTracker,
    /// Отчёты системе о работе кадров (ADPF, P13 сессии 07); сессию создаёт на этом потоке.
    adpf: AdpfReporter,
    status: RenderStatus,
    shared_status: Arc<Mutex<RenderStatus>>,
    next_frame_index: u64,
    next_diagnostics_at: Instant,
    /// Предыдущий кадр закончился ошибкой: повторы подряд не засоряют лог.
    previous_frame_failed: bool,
    /// Когда поток последний раз САМ запросил пересоздание цепочки (после `NeedsRecreate`).
    /// Нужен для паузы между повторами, пока пересоздание откладывается (нулевой размер окна).
    last_self_requested_recreate: Option<Instant>,
    /// Среди ещё не обработанных событий текущего пакета есть `SurfaceDestroyed`. Тогда долгие
    /// ожидания GPU для уходящей surface (пересоздание цепочки) бессмысленны: они съедают бюджет
    /// квитанции оболочки, а `detach` всё равно уничтожит цепочку.
    destroy_queued_in_batch: bool,
}

/// Что делать после одной итерации цикла.
enum LoopStep {
    Continue,
    Stop,
}

impl<B: DisplayBackend, C: FrameClock> RenderWorker<B, C> {
    pub(super) fn new(
        config: RenderLoopConfig,
        backend: B,
        clock: C,
        receiver: RenderInboxReceiver,
        shared_status: Arc<Mutex<RenderStatus>>,
        performance_hints: Arc<dyn PerformanceHintService>,
    ) -> Self {
        let pacing = PacingTracker::new(PacingParams::from_config(&config));
        let status = RenderStatus::initial(config.expected_refresh_period());
        // Задуманные линии ждут записей о показе столько же, сколько статус хранит сами записи.
        let present_target = PresentTargetTracker::new(&config.present_target, config.recent_presentations_capacity);
        let adpf = AdpfReporter::new(config.adpf.clone(), performance_hints);
        Self {
            config,
            backend,
            clock,
            receiver,
            machine: SurfaceStateMachine::new(),
            unattached_surface: None,
            pacing,
            present_target,
            adpf,
            status,
            shared_status,
            next_frame_index: 0,
            next_diagnostics_at: Instant::now(),
            previous_frame_failed: false,
            last_self_requested_recreate: None,
            destroy_queued_in_batch: false,
        }
    }

    /// Точка входа потока. Возвращается только после остановки.
    ///
    /// Каждая итерация цикла идёт под перехватом паники: паника вне вызова бэкенда (часы
    /// кадров, пейсинг, машина состояний, команды оболочки) не должна убить поток молча, оставив
    /// в статусе `Presenting`, а экран навсегда включённым. Она переводит поток в `Faulted`
    /// (причина `WorkerPanic`), снимает блокировку гашения экрана и дальше поток продолжает
    /// только подтверждать квитанции освобождения, пока его не остановят.
    pub(super) fn run(mut self, commands: &dyn ShellCommands) {
        self.request_preferred_frame_rate(commands);
        self.publish();
        loop {
            match catch_unwind(AssertUnwindSafe(|| self.run_iteration(commands))) {
                Ok(LoopStep::Continue) => {}
                Ok(LoopStep::Stop) => break,
                Err(payload) => self.fault_after_loop_panic(payload.as_ref(), commands),
            }
        }
        self.shutdown(commands);
    }

    /// Одна итерация: забрать ящик, обработать события, нарисовать кадр, опубликовать статус.
    fn run_iteration(&mut self, commands: &dyn ShellCommands) -> LoopStep {
        // Без показа спим на ящике сколько нужно; во время показа только заглядываем в него.
        let batch = if self.machine.should_present() {
            self.receiver.take_pending()
        } else {
            self.receiver.wait_for_events(None)
        };
        // Для каждого события знаем, лежит ли после него в пакете уничтожение surface.
        let destroy_after: Vec<bool> = destroy_follows_flags(&batch.events);
        for (stamped, destroy_follows) in batch.events.into_iter().zip(destroy_after) {
            self.destroy_queued_in_batch = destroy_follows;
            self.handle_event(stamped);
        }
        self.destroy_queued_in_batch = false;
        if batch.stop_requested {
            return LoopStep::Stop;
        }
        if self.machine.should_present() {
            self.present_next_frame();
        }
        self.publish();
        self.sync_screen_wake(commands);
        LoopStep::Continue
    }

    /// Паника в цикле: поток неисправен, но статус честный, экран отпущен, квитанции живы.
    fn fault_after_loop_panic(&mut self, payload: &(dyn Any + Send), commands: &dyn ShellCommands) {
        self.destroy_queued_in_batch = false;
        self.enter_fault(FaultKind::WorkerPanic, panic_message(payload));
        self.publish();
        // Команда оболочки сама могла быть причиной паники: пробуем ещё раз, но не падаем.
        if catch_unwind(AssertUnwindSafe(|| self.sync_screen_wake(commands))).is_err() {
            tracing::error!(target: LOG_TARGET, "снять блокировку гашения экрана после неисправности не удалось");
        }
    }

    /// Ждёт ли уничтожение surface: в пакете после текущего события или уже в ящике. Если да,
    /// долгие ожидания GPU начинать нельзя — иначе они сложатся с `detach` и выйдут за бюджет
    /// квитанции (`release_receipt_budget_ms`). Пропущенное ожидание безопасно: цепочка
    /// всё равно будет уничтожена в `detach`.
    fn teardown_is_imminent(&self) -> bool {
        self.destroy_queued_in_batch || self.receiver.has_pending_surface_destroyed()
    }

    /// Приводит блокировку гашения экрана в соответствие с состоянием показа. Команда
    /// уходит ТОЛЬКО при смене желаемого режима (начали / закончили показ: detach, пауза,
    /// неисправность, остановка), а не на каждом кадре. Вызывается после `publish`, поэтому
    /// `status.state` уже актуален. Итог команды (в том числе отказ) виден в статусе; при
    /// отказе повторов нет, чтобы не засорять очередь команд оболочки.
    fn sync_screen_wake(&mut self, commands: &dyn ShellCommands) {
        if !self.config.keep_screen_on_while_presenting {
            return;
        }
        let wanted = if self.status.state == RenderState::Presenting {
            ScreenWake::KeepOn
        } else {
            ScreenWake::AllowSleep
        };
        if wanted == self.status.screen_wake.requested {
            return;
        }
        let outcome = commands.keep_screen_on(wanted);
        tracing::info!(target: LOG_TARGET, ?wanted, ?outcome, "изменён режим блокировки гашения экрана");
        self.status.screen_wake.requested = wanted;
        self.status.screen_wake.last_outcome = Some(outcome);
        self.status.screen_wake.commands_sent += 1;
        self.store_status();
    }

    fn request_preferred_frame_rate(&mut self, commands: &dyn ShellCommands) {
        match FrameRate::new(self.config.preferred_frame_rate_hz) {
            Ok(rate) => {
                // Без surface платформа лишь запоминает пожелание, оболочка применит его
                // к следующей surface сама — повторять запрос не нужно.
                let outcome = commands.set_preferred_frame_rate(rate);
                tracing::info!(target: LOG_TARGET, ?outcome, hertz = rate.hertz(), "запрошена частота кадров");
                self.status.frame_rate_request = Some(outcome);
            }
            // validate() это исключает; но молча не игнорируем.
            Err(error) => tracing::error!(target: LOG_TARGET, %error, "частота кадров из конфига неверна"),
        }
    }

    // ---- события ящика ----

    fn handle_event(&mut self, stamped: StampedEvent) {
        let StampedEvent { event, enqueued_at } = stamped;
        let mut context = ActionContext::default();
        let actions = match event {
            InboxEvent::SurfaceCreated(surface) => {
                let actions = self.machine.on_created(surface.generation());
                context.incoming_surface = Some(surface);
                actions
            }
            InboxEvent::SurfaceChanged { generation, size } => self.machine.on_changed(generation, size),
            InboxEvent::SurfaceDestroyed(request) => {
                let actions = self.machine.on_destroyed(request.generation());
                context.release = Some(PendingRelease { request, enqueued_at });
                actions
            }
            InboxEvent::Paused => {
                self.machine.on_paused();
                Vec::new()
            }
            InboxEvent::Resumed => {
                self.machine.on_resumed();
                Vec::new()
            }
            InboxEvent::Viewfinder(command) => {
                self.apply_viewfinder_command(command);
                Vec::new()
            }
            InboxEvent::PresentTarget(command) => {
                // Политика линии кадра — состояние потока (не бэкенда): применяется сразу, между кадрами.
                let outcome = self.present_target.apply_command(command);
                self.status.present_target_commands.note(command, outcome);
                self.status.present_target = self.present_target.snapshot();
                Vec::new()
            }
            InboxEvent::Adpf(command) => {
                self.apply_adpf_command(command);
                Vec::new()
            }
        };
        self.execute_actions(actions, &mut context);
        // Новая surface принята машиной (её не отвергли): ждёт размера.
        if let Some(surface) = context.incoming_surface.take() {
            self.unattached_surface = Some(surface);
        }
        // Запрос, не использованный действиями, не должен остаться висеть: уничтожение
        // квитанции сообщает оболочке `DroppedWithoutConfirmation`. Для destroyed машина
        // всегда выдаёт ConfirmRelease, так что сюда попадать нечему — это страховка.
        drop(context.release.take());
    }

    /// Команда видоискателю: бэкенд применяет её между кадрами, итог — в статусе. Неисправный
    /// бэкенд не вызывается: команда учитывается как неисполнимая.
    fn apply_viewfinder_command(&mut self, command: ViewfinderCommand) {
        let outcome = self
            .guarded(|backend| backend.apply_viewfinder_command(command))
            .unwrap_or(ViewfinderCommandOutcome::Unsupported);
        self.status.viewfinder_commands.note(command, outcome);
    }

    fn execute_actions(&mut self, actions: Vec<SurfaceAction>, context: &mut ActionContext) {
        for action in actions {
            self.execute_action(action, context);
        }
    }

    fn execute_action(&mut self, action: SurfaceAction, context: &mut ActionContext) {
        match action {
            SurfaceAction::Attach { generation, size } => self.attach_surface(generation, size),
            SurfaceAction::Resize { size, .. } => {
                // Смена размера пришла от оболочки: это новая информация, пауза между повторами не нужна.
                self.last_self_requested_recreate = None;
                self.resize_surface(size);
            }
            SurfaceAction::Detach { generation } => self.detach_surface(generation),
            SurfaceAction::DiscardUnattached { generation } => {
                tracing::debug!(target: LOG_TARGET, generation = generation.value(), "surface без размера отброшена");
                self.unattached_surface = None;
            }
            SurfaceAction::RejectIncoming { generation } => {
                tracing::warn!(target: LOG_TARGET, generation = generation.value(), "входящая surface отвергнута");
                context.incoming_surface = None;
            }
            SurfaceAction::ConfirmRelease { generation } => self.confirm_release(generation, context),
        }
    }

    fn attach_surface(&mut self, generation: SurfaceGeneration, size: SurfaceSize) {
        let Some(surface) = self.unattached_surface.take() else {
            // Машина считает, что surface ждёт размера, а её нет: нарушение инварианта.
            self.record_backend_error("attach", "surface для присоединения отсутствует".to_owned());
            self.machine.on_attach_failed(generation);
            return;
        };
        match self.guarded(|backend| backend.attach(surface, size)) {
            Some(Ok(())) => {
                self.pacing.mark_discontinuity();
                self.machine.on_attach_succeeded(generation, size);
                self.status.surfaces_attached += 1;
                tracing::info!(target: LOG_TARGET, generation = generation.value(), ?size, "surface присоединена");
            }
            Some(Err(error)) => {
                self.machine.on_attach_failed(generation);
                self.record_backend_error("attach", error.to_string());
            }
            // Бэкенд недоступен (паника): surface уже поглощена вызовом и уничтожена.
            None => {}
        }
    }

    fn resize_surface(&mut self, size: SurfaceSize) {
        if self.teardown_is_imminent() {
            tracing::debug!(target: LOG_TARGET, "пересоздание цепочки пропущено: surface уже уничтожается");
            return;
        }
        // Записи показов старой цепочки исчезнут вместе с ней: забираем их до пересоздания.
        self.drain_past_presentations();
        match self.guarded(|backend| backend.resize(size)) {
            Some(Ok(())) => {
                self.status.recreations += 1;
                // Номера показов новой цепочки не продолжают старые.
                self.pacing.mark_discontinuity();
            }
            Some(Err(error)) => {
                self.record_backend_error("resize", error.to_string());
                self.handle_surface_lost();
            }
            None => {}
        }
    }

    fn detach_surface(&mut self, generation: SurfaceGeneration) {
        // Последние записи показов забираем, пока цепочка жива: после detach их уже не будет.
        self.drain_past_presentations();
        // Возвращённая surface уничтожается на выходе из блока: окно отпущено ДО подтверждения.
        if let Some(Some(surface)) = self.guarded(DisplayBackend::detach) {
            drop(surface);
        }
        tracing::info!(target: LOG_TARGET, generation = generation.value(), "surface отсоединена");
        // После отсоединения снимаем счётчики: по ним видно, что GPU-объекты освобождены.
        let failures_before = self.status.backend.detach_failures;
        self.poll_diagnostics();
        self.report_new_detach_failures(failures_before);
    }

    fn confirm_release(&mut self, generation: SurfaceGeneration, context: &mut ActionContext) {
        let Some(pending) = context.release.take() else {
            tracing::error!(target: LOG_TARGET, generation = generation.value(), "подтверждать нечего");
            return;
        };
        pending.request.confirm_released();
        self.status.releases_confirmed += 1;
        self.status.last_release_latency = Some(pending.enqueued_at.elapsed());
        tracing::info!(
            target: LOG_TARGET,
            generation = generation.value(),
            latency_us = pending.enqueued_at.elapsed().as_micros(),
            "освобождение surface подтверждено"
        );
    }

    // ---- кадры ----

    fn present_next_frame(&mut self) {
        let wait = self.clock.wait_next_frame(self.config.vsync_wait_timeout());
        match wait {
            FrameWait::Tick(tick) => self.render_frame(tick),
            // Тика нет — цикл вернётся к ящику, ничего страшного.
            FrameWait::TimedOut => {}
            FrameWait::Stopped => self.enter_fault(FaultKind::ClockStopped, "источник vsync остановлен".to_owned()),
        }
    }

    fn render_frame(&mut self, tick: FrameTick) {
        // acquire ждёт GPU до `acquire_timeout`; при лежащем в ящике уничтожении это ожидание
        // отдало бы бюджет квитанции. Кадр пропускаем: цикл сразу вернётся к ящику.
        if self.teardown_is_imminent() {
            return;
        }
        // Начало периода работы кадра для ADPF (часы не прочитались — кадр без отчёта).
        let frame_started = SystemMonotonicTimer.now().ok();
        let target = match self.plan_present_target(self.next_frame_index, &tick) {
            TickPlan::Present { target, .. } => target,
            // Сход на ближнюю линию (решение 54): на этом тике не рисуем, не показываем и камеру не
            // опрашиваем — кадр камеры останется в буфере видоискателя до следующего тика.
            TickPlan::SkipForDepthReduction => return,
        };
        let timing = FrameTiming {
            frame_index: self.next_frame_index,
            vsync_sequence: tick.sequence,
            skipped_vsyncs: tick.skipped_ticks,
            target,
        };
        self.next_frame_index += 1;
        let Some(outcome) = self.guarded(|backend| backend.render_and_present(timing)) else {
            return;
        };
        let frame_finished = SystemMonotonicTimer.now().ok();
        let cpu_timing = self.account_frame_cpu_timing();
        let frame_failed = matches!(outcome, PresentOutcome::Failed(_));
        match outcome {
            PresentOutcome::Presented => {
                self.status.frames_presented += 1;
                self.present_target.record_presented(timing.frame_index, &timing.target);
                let work =
                    PresentedFrameWork::measure(timing.frame_index, frame_started, frame_finished, cpu_timing.as_ref());
                if let Some(work) = work {
                    self.adpf.frame_presented(work, self.pacing.refresh_period());
                }
            }
            PresentOutcome::NeedsRecreate => self.recreate_swapchain(),
            PresentOutcome::SurfaceLost => self.handle_surface_lost(),
            PresentOutcome::TimedOut => self.status.acquire_timeouts += 1,
            PresentOutcome::Failed(error) => self.record_frame_failure(&error),
        }
        self.previous_frame_failed = frame_failed;
        // После учёта показа: время GPU может прийти и для кадра, показанного только что.
        self.account_frame_gpu_timings();
        // Каждый кадр, а не по таймеру: драйвер хранит лишь короткую историю времён показа,
        // и при редком опросе записи теряются (это давало ложные «пропуски» в пейсинге).
        self.drain_past_presentations();
        self.poll_diagnostics_if_due();
    }

    /// Пересоздание по запросу бэкенда (`NeedsRecreate`). Повторы реже `recreate_retry_interval`
    /// не выполняются: пока размер окна нулевой, бэкенд откладывает пересоздание и просит его
    /// снова на каждом тике, а каждая попытка — вызовы драйвера и ожидание GPU.
    fn recreate_swapchain(&mut self) {
        let SurfacePhase::Attached { size, .. } = self.machine.phase() else {
            return;
        };
        let now = Instant::now();
        let retry_interval = self.config.recreate_retry_interval();
        if let Some(previous) = self.last_self_requested_recreate
            && now.saturating_duration_since(previous) < retry_interval
        {
            return;
        }
        self.last_self_requested_recreate = Some(now);
        self.resize_surface(size);
    }

    fn handle_surface_lost(&mut self) {
        let actions = self.machine.on_surface_lost();
        self.execute_actions(actions, &mut ActionContext::default());
    }

    fn record_frame_failure(&mut self, error: &BackendError) {
        self.status.backend_errors += 1;
        self.status.last_backend_error = Some(error.to_string());
        if !self.previous_frame_failed {
            tracing::warn!(target: LOG_TARGET, %error, "кадр не показан");
        }
    }

    fn record_backend_error(&mut self, operation: &str, reason: String) {
        tracing::error!(target: LOG_TARGET, operation, %reason, "ошибка бэкенда");
        self.status.backend_errors += 1;
        self.status.last_backend_error = Some(format!("{operation}: {reason}"));
    }

    // ---- диагностика ----

    fn poll_diagnostics_if_due(&mut self) {
        if Instant::now() >= self.next_diagnostics_at {
            self.poll_diagnostics();
        }
    }

    fn poll_diagnostics(&mut self) {
        self.next_diagnostics_at = Instant::now() + self.config.diagnostics_poll_interval();
        let Some(diagnostics) = self.guarded(DisplayBackend::diagnostics) else {
            return;
        };
        // Период экрана спрашиваем у бэкенда на каждом опросе: режим дисплея меняется на ходу
        // (OxygenOS поднимает 60 -> 120 Гц после первого касания).
        self.adopt_reported_refresh_cycle(diagnostics.display.as_ref());
        self.status.pacing = self.pacing.snapshot();
        self.status.present_target = self.present_target.snapshot();
        self.status.adpf = self.adpf.snapshot();
        self.status.display = diagnostics.display.clone();
        self.adopt_backend_health(&diagnostics);
    }

    /// Переносит счётчики бэкенда (объекты GPU, validation, неудачные detach) в статус.
    fn adopt_backend_health(&mut self, diagnostics: &BackendDiagnostics) {
        self.status.backend = BackendHealth::from_diagnostics(diagnostics);
    }

    /// Громко сообщает о новых неудачных detach: окно уже возвращено и квитанция подтверждается
    /// (оболочка не должна зависнуть), но Vulkan-объекты утекли, и это не должно быть тихим.
    fn report_new_detach_failures(&self, failures_before: u64) {
        let failures_now = self.status.backend.detach_failures;
        if failures_now > failures_before {
            tracing::error!(
                target: LOG_TARGET,
                detach_failures = failures_now,
                "detach не освободил GPU-объекты: ANativeWindow возвращён и квитанция подтверждена, но VkSurfaceKHR/swapchain утекли и могут удерживать подключение производителя к окну"
            );
        }
    }

    /// Пейсинг судит по РЕАЛЬНОМУ периоду экрана, если бэкенд его знает: система может
    /// держать 90 Гц, хотя просили 120, и тогда сравнение с 120 Гц дало бы ложные пропуски.
    /// Пока период неизвестен (или цепочки нет, как после `detach`), остаётся последнее
    /// известное значение: изначально это запасной период из `preferred_frame_rate_hz`.
    fn adopt_reported_refresh_cycle(&mut self, display: Option<&DisplayInfo>) {
        let Some(period) = display.and_then(|info| info.refresh_cycle) else {
            return;
        };
        // Дрожание измерения (единицы мкс) период не меняет: смена логируется только настоящая.
        if self.pacing.set_refresh_period(period) {
            tracing::info!(target: LOG_TARGET, period_us = period.as_micros(), "пейсинг переключён на реальный период экрана");
        }
        // Отчёт — по периоду, который реально судит пейсинг (а не по последнему шумному замеру).
        self.status.pacing_refresh_period = self.pacing.refresh_period();
    }

    // ---- неисправность и остановка ----

    /// Вызывает бэкенд, перехватывая панику. `None` — бэкенд недоступен: он уже неисправен
    /// или упал прямо сейчас (тогда поток переходит в Faulted).
    fn guarded<T>(&mut self, call: impl FnOnce(&mut B) -> T) -> Option<T> {
        if self.status.fault.is_some() {
            return None;
        }
        match catch_unwind(AssertUnwindSafe(|| call(&mut self.backend))) {
            Ok(value) => Some(value),
            Err(payload) => {
                self.enter_fault(FaultKind::BackendPanic, panic_message(payload.as_ref()));
                None
            }
        }
    }

    /// Переводит поток в неисправное состояние: бэкенд больше не вызывается как исправный,
    /// но запросы освобождения по-прежнему подтверждаются (чтобы оболочка не ждала таймаут).
    fn enter_fault(&mut self, kind: FaultKind, message: String) {
        if self.status.fault.is_some() {
            return;
        }
        tracing::error!(target: LOG_TARGET, ?kind, %message, "поток рендера неисправен");
        self.status.fault = Some(FaultInfo { kind, message });
        self.machine.on_fault();
        self.unattached_surface = None;
        // Последняя попытка отпустить окно. Бэкенд после паники мог остаться в
        // неконсистентном состоянии, поэтому ещё одна паника здесь допустима и гасится.
        let detach_attempt = catch_unwind(AssertUnwindSafe(|| self.backend.detach()));
        let detach_panicked = detach_attempt.is_err();
        if detach_panicked {
            tracing::error!(target: LOG_TARGET, "detach после неисправности тоже упал");
        }
        self.account_detach_after_fault(detach_panicked);
    }

    /// Учёт detach, выполненного на пути неисправности. Обычный опрос диагностики после
    /// неисправности отключён (`guarded`), поэтому без этого шага неудачный detach (утечка
    /// Vulkan-объектов) остался бы невидимым в статусе.
    fn account_detach_after_fault(&mut self, detach_panicked: bool) {
        let failures_before = self.status.backend.detach_failures;
        match catch_unwind(AssertUnwindSafe(|| self.backend.diagnostics())) {
            Ok(diagnostics) => self.adopt_backend_health(&diagnostics),
            Err(_) => tracing::error!(target: LOG_TARGET, "диагностика бэкенда после неисправности тоже упала"),
        }
        if detach_panicked {
            // Бэкенд не смог сообщить итог сам: упавший detach считается неудачным.
            self.status.backend.detach_failures += 1;
        }
        self.report_new_detach_failures(failures_before);
    }

    /// Завершение: отпускает текущую surface. Оставшиеся в ящике запросы доуничтожит
    /// `Drop` приёмника (они увидят `DroppedWithoutConfirmation`).
    fn shutdown(mut self, commands: &dyn ShellCommands) {
        let leftover = self.receiver.take_pending();
        for stamped in leftover.events {
            self.handle_event(stamped);
        }
        let actions = self.machine.on_shutdown();
        self.execute_actions(actions, &mut ActionContext::default());
        self.unattached_surface = None;
        self.publish_final();
        // Остановка — выход из показа: экран снова можно гасить.
        self.sync_screen_wake(commands);
    }

    // ---- публикация статуса ----

    fn publish(&mut self) {
        self.refresh_derived_status(false);
        self.store_status();
    }

    fn publish_final(&mut self) {
        self.refresh_derived_status(true);
        self.store_status();
    }

    fn refresh_derived_status(&mut self, stopped: bool) {
        self.status.state = if stopped {
            RenderState::Stopped
        } else if self.status.fault.is_some() {
            RenderState::Faulted
        } else {
            match self.machine.phase() {
                SurfacePhase::NoSurface => RenderState::WaitingForSurface,
                SurfacePhase::AwaitingSize { .. } => RenderState::AwaitingSize,
                SurfacePhase::Attached { .. } if self.machine.is_paused() => RenderState::Paused,
                SurfacePhase::Attached { .. } => RenderState::Presenting,
                SurfacePhase::Unusable { .. } => RenderState::SurfaceUnusable,
                SurfacePhase::Faulted => RenderState::Faulted,
            }
        };
        self.status.generation = self.machine.current_generation();
        self.status.stale_changes = self.machine.stale_changes();
    }

    fn store_status(&self) {
        let mut shared = self.shared_status.lock().unwrap_or_else(PoisonError::into_inner);
        *shared = self.status.clone();
    }
}

/// Для каждого события пакета: есть ли после него (строго позже) `SurfaceDestroyed`.
fn destroy_follows_flags(events: &[StampedEvent]) -> Vec<bool> {
    let mut flags = vec![false; events.len()];
    let mut destroy_seen_later = false;
    for (index, stamped) in events.iter().enumerate().rev() {
        flags[index] = destroy_seen_later;
        if matches!(stamped.event, InboxEvent::SurfaceDestroyed(_)) {
            destroy_seen_later = true;
        }
    }
    flags
}

/// Достаёт текст из полезной нагрузки паники (`&str` или `String`).
fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "паника без текстового сообщения".to_owned()
    }
}
