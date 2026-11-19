//! Сервис экрана камеры на стороне маршрутизатора (сессия 12): установка, снятие, маршрутизация входа, диагностика.
//!
//! # Роли
//! * ВЛАДЕЕТ жизненным циклом экрана: когда источник UI поставлен на поток рендера, а когда снят;
//! * принимает от маршрутизатора касания, клавиши и ориентацию и кладёт их в канал входа
//!   ([`crate::camera_screen_input`]); если экрана нет — отбрасывает со счётчиком;
//! * собирает источник ([`CameraScreenSource`]) из сервисов оболочки, камеры и рендера;
//! * отдаёт сценариям только диагностику (чтение), а не управление экраном.
//!
//! # Жизненный цикл
//! ```text
//!  NotInstalledYet ──install──▶ Installed ──suspend_for_scenario──▶ SuspendedForScenario
//!                                  ▲                                        │
//!                                  └─────────── resume_after_scenario ◀─────┘
//! ```
//! * `install` — при запуске маршрутизатора (постоянная установка, БЕЗ учёта сценария: см.
//!   `GpuServices::install_product_ui_overlay`);
//! * на время ЛЮБОГО сценария, кроме объявившего [`CameraScreenPolicy::KeepRunning`], экран снимается:
//!   прежние телефонные эталоны (`glass`, `viewfinder`, `3a`, скриншоты) остаются сравнимыми;
//! * в `EventRouter::finish` экран ставится ЗАНОВО (свежий `CameraScreen`; состояние экрана между сценариями
//!   не сохраняется). Сценарий `glass` ставит свой источник — после его конца рендер снимает его, и экран возвращается.
//!
//! # Рендер снял экран сам
//! Рендер вправе снять источник без ведома сервиса (паника источника, отказ создания рендера UI): получатель
//! канала входа уничтожается ([`ReceiverState::Gone`]). Сервис проверяет это на каждом тике маршрутизатора
//! ([`CameraScreenService::maintain`]), переходит в явный статус [`CameraScreenStatus::DroppedByRenderer`] и через
//! паузу из конфига ставит СВЕЖИЙ экран. Попытки ограничены (`camera_screen.reinstall`); после исчерпания
//! статус [`CameraScreenStatus::ReinstallExhausted`] (отказ) — вечный цикл «упал — поставил» батарею не жжёт.
//! Тот же тик досылает долг разрыва касаний ([`CameraScreenService::maintain`], см. `camera_screen_input`).
//!
//! Остальные исходы ([`CameraScreenStatus`]) — явные: `Disabled` (выключен конфигом), `BuildFailed`,
//! `RenderUnavailable` (GPU не поднялся — запасной путь `HoldAndConfirmRoute`), `RenderGone`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera_screen::shutter::ShotRequest;
use camera_screen::{CameraScreen, ScreenAction, ScreenBuildError, ScreenTouch};
use shell_contract::hardware_key::HardwareKeyEvent;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchSample;
use ui_kit::tokens::UiKitTokens;

use crate::camera_screen_capture::{CaptureCompletionSlot, CaptureService, FakeCaptureCounters, FakeCaptureService};
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_effects::{
    AbsentCameraPort, CameraIntentPort, EffectExecutor, EffectTargets, HandleCameraPort,
};
use crate::camera_screen_facts::{AbsentEngineFactsSource, CameraFactsSource, EngineFactsSource};
use crate::camera_screen_gravity::start_gravity_feed;
use crate::camera_screen_input::{
    InputDelivery, ReceiverState, ScreenInputSender, ScreenInputStats, screen_input_channel, volume_key_action,
};
use crate::camera_screen_source::{CameraScreenSource, GravityLink, ScreenBoard, ScreenReport, ScreenSourceParts};
use crate::camera_service::CameraServices;
use crate::gpu_services::{GpuServices, RenderObservation};
use crate::scenario::observing::ShellServices;
use crate::viewfinder_control::ControlDelivery;

const LOG_TARGET: &str = "fasticamera::camera_screen";

/// Причина снятия экрана рендером: последняя ошибка UI-прохода из диагностики рендера, если она доступна.
fn renderer_drop_reason(gpu: &GpuServices) -> String {
    const UNKNOWN: &str = "причина не сообщена рендером";
    match gpu.render.observe() {
        RenderObservation::Live { status, .. } => status
            .backend
            .viewfinder
            .and_then(|viewfinder| viewfinder.ui_overlay.last_error)
            .unwrap_or_else(|| UNKNOWN.to_owned()),
        RenderObservation::Unavailable { reason } => format!("рендер недоступен: {reason}"),
    }
}

/// Что сценарий хочет от экрана на время своей работы.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraScreenPolicy {
    /// Экран снимается на время сценария (по умолчанию: сценарии меряют и снимают «чистый» видоискатель).
    #[default]
    SuspendForScenario,
    /// Экран остаётся на месте и продолжает получать вход (сценарий `screen`, проверяющий сам экран).
    KeepRunning,
}

/// Состояние экрана. Не `bool`: «выключен», «не собран», «нет рендера» и «снят на сценарий» — разные ответы.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CameraScreenStatus {
    /// Экран выключен конфигом (`camera_screen.enabled = false`).
    Disabled,
    /// Установка ещё не запрашивалась (маршрутизатор не запущен).
    NotInstalledYet,
    /// Источник отправлен в ящик потока рендера. Применил ли его рендер, видно в `UiOverlayDiagnostics`.
    Installed,
    /// Экран снят на время сценария; после его конца будет поставлен заново.
    SuspendedForScenario { scenario: &'static str },
    /// Экран не собран (неверный конфиг или токены).
    BuildFailed { error: String },
    /// Рендер не поднимался (GPU или vsync): экрана нет, касания отбрасываются со счётчиком.
    RenderUnavailable { reason: String },
    /// Поток рендера уже завершён: источник потерян.
    RenderGone,
    /// Рендер снял источник экрана сам (паника, отказ создания рендера UI); свежий экран будет поставлен
    /// после паузы. `attempt` — номер ближайшей попытки переустановки (с 1).
    DroppedByRenderer { reason: String, attempt: u32 },
    /// Рендер снимал экран столько раз подряд, что попытки (`camera_screen.reinstall.max_attempts`) исчерпаны:
    /// экрана нет до следующего сценария или перезапуска. Явный отказ, а не молчаливая пропажа.
    ReinstallExhausted { reason: String, attempts: u32 },
}

impl CameraScreenStatus {
    /// Принимает ли экран вход сейчас.
    pub fn is_installed(&self) -> bool {
        matches!(self, Self::Installed)
    }
}

/// Сколько входа отброшено, потому что экрана не было (по статусу видно, почему).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DroppedInputCounters {
    pub touches: u64,
    /// Клавиши и ориентация.
    pub actions: u64,
}

/// Счётчики восстановления экрана после снятия его рендером.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RecoveryCounters {
    /// Сколько раз рендер снял экран сам.
    pub renderer_drops: u64,
    /// Сколько раз экран поставлен заново после такого снятия (успешная доставка в рендер).
    pub reinstalls: u64,
    /// Сколько раз попытки исчерпались (статус `ReinstallExhausted`).
    pub exhausted: u64,
}

/// Учёт текущей серии падений (сбрасывается началом сценария и «здоровой» жизнью экрана).
#[derive(Debug, Default)]
struct RecoveryPlan {
    /// Сколько раз подряд рендер снимал экран, не дав ему «здоровой» жизни.
    consecutive_drops: u32,
    /// Когда можно ставить свежий экран; `None` — ничего не запланировано.
    retry_at: Option<Instant>,
    /// Когда последний раз экран был поставлен (для «здоровой» жизни).
    installed_at: Option<Instant>,
}

/// Сервисы окружения, из которых собирается экран (маршрутизатор передаёт ссылки на свои поля).
pub struct ScreenEnvironment<'a> {
    pub services: &'a ShellServices,
    pub gpu: &'a GpuServices,
    pub camera: &'a CameraServices,
    /// Известная маршрутизатору ориентация телефона: экран узнаёт её сразу при установке.
    pub orientation: Option<DeviceOrientation>,
}

/// Установленный экран: писатель канала входа и доска отчёта.
struct InstalledScreen {
    sender: ScreenInputSender,
}

/// Сервис экрана камеры.
pub struct CameraScreenService {
    config: CameraScreenConfig,
    status: CameraScreenStatus,
    installed: Option<InstalledScreen>,
    /// Доска отчёта последнего источника: остаётся после снятия, пока не поставлен новый.
    board: Option<Arc<ScreenBoard>>,
    /// Статистика канала последнего экрана на момент снятия (после снятия канал уже недоступен).
    last_input_stats: ScreenInputStats,
    capture: Arc<FakeCaptureService>,
    completion_slot: Arc<CaptureCompletionSlot>,
    dropped: DroppedInputCounters,
    installs: u64,
    recovery: RecoveryPlan,
    recovery_counters: RecoveryCounters,
}

impl CameraScreenService {
    pub fn new(config: CameraScreenConfig) -> Self {
        let completion_slot = Arc::new(CaptureCompletionSlot::default());
        let capture = Arc::new(FakeCaptureService::new(
            config.fake_capture_delay(),
            config.fake_capture_journal_capacity,
            Arc::clone(&completion_slot),
        ));
        let status = if config.enabled {
            CameraScreenStatus::NotInstalledYet
        } else {
            CameraScreenStatus::Disabled
        };
        Self {
            config,
            status,
            installed: None,
            board: None,
            last_input_stats: ScreenInputStats::default(),
            capture,
            completion_slot,
            dropped: DroppedInputCounters::default(),
            installs: 0,
            recovery: RecoveryPlan::default(),
            recovery_counters: RecoveryCounters::default(),
        }
    }

    // ---- Диагностика (для сценариев и тестов) -------------------------------------------------------------

    pub fn status(&self) -> &CameraScreenStatus {
        &self.status
    }

    /// Последний отчёт экрана с потока рендера; `None` — кадров ещё не было.
    pub fn report(&self) -> Option<ScreenReport> {
        self.board.as_ref().and_then(|board| board.latest())
    }

    /// Счётчики канала входа: живого экрана или (после снятия) последнего.
    pub fn input_stats(&self) -> ScreenInputStats {
        self.installed
            .as_ref()
            .map_or(self.last_input_stats, |installed| installed.sender.stats())
    }

    /// Вход, отброшенный потому, что экрана не было.
    pub fn dropped_input(&self) -> DroppedInputCounters {
        self.dropped
    }

    /// Запросы снимка, принятые фейк-сервисом, по порядку.
    pub fn capture_journal(&self) -> Vec<ShotRequest> {
        self.capture.journal()
    }

    pub fn capture_counters(&self) -> FakeCaptureCounters {
        self.capture.counters()
    }

    /// Сколько раз экран ставился (свежий `CameraScreen` на каждую установку).
    pub fn installs(&self) -> u64 {
        self.installs
    }

    /// Счётчики восстановления после снятия экрана рендером.
    pub fn recovery(&self) -> RecoveryCounters {
        self.recovery_counters
    }

    // ---- Жизненный цикл -------------------------------------------------------------------------------------

    /// Ставит экран. Безопасно вызывать повторно: уже установленный экран не пересоздаётся.
    pub fn install(&mut self, env: &ScreenEnvironment<'_>) {
        if !self.config.enabled || self.installed.is_some() {
            return;
        }
        let (source, sender, board) = match self.build_source(env) {
            Ok(built) => built,
            Err(error) => {
                tracing::error!(target: LOG_TARGET, %error, "экран камеры не собран");
                self.status = CameraScreenStatus::BuildFailed {
                    error: error.to_string(),
                };
                return;
            }
        };
        self.completion_slot.attach(sender.injector());
        match env.gpu.install_product_ui_overlay(Box::new(source)) {
            ControlDelivery::Delivered => {
                self.installs += 1;
                self.recovery.installed_at = Some(Instant::now());
                self.board = Some(board);
                self.installed = Some(InstalledScreen { sender });
                self.status = CameraScreenStatus::Installed;
                // Ориентацию экран узнаёт сразу, а не при следующем повороте телефона.
                if let Some(orientation) = env.orientation {
                    self.route_orientation(orientation);
                }
                tracing::info!(target: LOG_TARGET, installs = self.installs, "экран камеры поставлен");
            }
            ControlDelivery::RenderGone => {
                self.completion_slot.detach();
                tracing::error!(target: LOG_TARGET, "поток рендера завершён: экран камеры не поставлен");
                self.status = CameraScreenStatus::RenderGone;
            }
            ControlDelivery::RenderUnavailable { reason } => {
                self.completion_slot.detach();
                tracing::warn!(target: LOG_TARGET, %reason, "рендер недоступен: экрана камеры нет, касания отбрасываются");
                self.status = CameraScreenStatus::RenderUnavailable { reason };
            }
        }
    }

    /// Сценарий начался: снять экран, если сценарий не просит оставить.
    pub fn suspend_for_scenario(&mut self, scenario: &'static str, policy: CameraScreenPolicy, gpu: &GpuServices) {
        if policy == CameraScreenPolicy::KeepRunning {
            return;
        }
        if self.installed.is_none() {
            // Экран уже снят рендером и ждёт переустановки (или отказ): на время сценария ничего не ставим,
            // свежий экран вернёт `resume_after_scenario`.
            if matches!(
                self.status,
                CameraScreenStatus::DroppedByRenderer { .. } | CameraScreenStatus::ReinstallExhausted { .. }
            ) {
                self.recovery = RecoveryPlan::default();
                self.status = CameraScreenStatus::SuspendedForScenario { scenario };
            }
            return;
        }
        self.recovery = RecoveryPlan::default();
        self.remember_input_stats();
        self.installed = None;
        self.completion_slot.detach();
        let delivery = gpu.remove_product_ui_overlay();
        if !delivery.is_delivered() {
            tracing::warn!(target: LOG_TARGET, scenario, ?delivery, "снятие экрана на время сценария не доставлено");
        }
        self.status = CameraScreenStatus::SuspendedForScenario { scenario };
    }

    /// Сценарий закончился любым путём: вернуть экран (свежий). Не трогает экран, который сценарий не снимал.
    pub fn resume_after_scenario(&mut self, env: &ScreenEnvironment<'_>) {
        if matches!(self.status, CameraScreenStatus::SuspendedForScenario { .. }) {
            self.recovery = RecoveryPlan::default();
            self.status = CameraScreenStatus::NotInstalledYet;
            self.install(env);
        }
    }

    /// Тик маршрутизатора: досылает долг разрыва касаний и следит за экраном, снятым рендером.
    ///
    /// Дешёвый вызов (слабая ссылка и флаг): вызывается на каждом шаге маршрутизатора, в том числе на пустых.
    pub fn maintain(&mut self, now: Instant, env: &ScreenEnvironment<'_>) {
        if let Some(installed) = &mut self.installed {
            if installed.sender.receiver_state() == ReceiverState::Gone {
                self.note_dropped_by_renderer(now, env.gpu);
            } else {
                // Результат не нужен: долг, не влезший сейчас, будет досылаться на следующих тиках, а счётчики
                // (`gaps_queued`, `overflowed_touches`) канал ведёт сам.
                let _flush = installed.sender.flush_owed_gap();
            }
        }
        self.reinstall_if_due(now, env);
    }

    /// Экран снят рендером: запомнить статистику, решить, пробовать ли ещё.
    fn note_dropped_by_renderer(&mut self, now: Instant, gpu: &GpuServices) {
        self.remember_input_stats();
        self.installed = None;
        self.completion_slot.detach();
        self.recovery_counters.renderer_drops += 1;
        // Экран, проживший «здоровый» срок, — новый сбой, а не повтор прежнего.
        let lived_healthy = self.recovery.installed_at.is_some_and(|installed_at| {
            now.saturating_duration_since(installed_at) >= self.config.reinstall.healthy_after()
        });
        if lived_healthy {
            self.recovery.consecutive_drops = 0;
        }
        self.recovery.consecutive_drops += 1;
        self.recovery.installed_at = None;
        let attempts = self.recovery.consecutive_drops;
        let reason = renderer_drop_reason(gpu);
        if attempts > self.config.reinstall.max_attempts {
            self.recovery.retry_at = None;
            self.recovery_counters.exhausted += 1;
            tracing::error!(target: LOG_TARGET, %reason, attempts = attempts - 1, "рендер снимает экран камеры снова и снова: попытки исчерпаны");
            self.status = CameraScreenStatus::ReinstallExhausted {
                reason,
                attempts: attempts - 1,
            };
            return;
        }
        self.recovery.retry_at = Some(now + self.config.reinstall.pause());
        tracing::warn!(target: LOG_TARGET, %reason, attempt = attempts, "рендер снял экран камеры: поставлю свежий после паузы");
        self.status = CameraScreenStatus::DroppedByRenderer {
            reason,
            attempt: attempts,
        };
    }

    /// Пауза вышла: ставит свежий экран на место снятого.
    fn reinstall_if_due(&mut self, now: Instant, env: &ScreenEnvironment<'_>) {
        let Some(retry_at) = self.recovery.retry_at else {
            return;
        };
        if now < retry_at || !matches!(self.status, CameraScreenStatus::DroppedByRenderer { .. }) {
            return;
        }
        self.recovery.retry_at = None;
        self.status = CameraScreenStatus::NotInstalledYet;
        self.install(env);
        if self.status.is_installed() {
            self.recovery_counters.reinstalls += 1;
        }
    }

    fn remember_input_stats(&mut self) {
        if let Some(installed) = &self.installed {
            self.last_input_stats = installed.sender.stats();
        }
    }

    // ---- Маршрутизация входа ----------------------------------------------------------------------------------

    pub fn route_touch(&mut self, sample: TouchSample) {
        self.send_touch(ScreenTouch::Sample(sample));
    }

    /// Разрыв потока касаний (`ShellEvent::TouchGap`): жесты экрана сбрасываются.
    pub fn route_touch_gap(&mut self) {
        self.send_touch(ScreenTouch::Gap);
    }

    pub fn route_hardware_key(&mut self, event: HardwareKeyEvent) {
        self.send_action(volume_key_action(event));
    }

    pub fn route_orientation(&mut self, orientation: DeviceOrientation) {
        self.send_action(ScreenAction::DeviceOrientationChanged(orientation));
    }

    fn send_touch(&mut self, touch: ScreenTouch) {
        match &mut self.installed {
            // Переполнение уже посчитано каналом; здесь считаем только «экрана нет».
            Some(installed) => {
                if installed.sender.send_touch(touch) == InputDelivery::ReceiverGone {
                    self.dropped.touches += 1;
                }
            }
            None => self.dropped.touches += 1,
        }
    }

    fn send_action(&mut self, action: ScreenAction) {
        match &self.installed {
            Some(installed) => {
                if installed.sender.send_action(action) == InputDelivery::ReceiverGone {
                    self.dropped.actions += 1;
                }
            }
            None => self.dropped.actions += 1,
        }
    }

    // ---- Сборка ------------------------------------------------------------------------------------------------

    fn build_source(
        &self,
        env: &ScreenEnvironment<'_>,
    ) -> Result<(CameraScreenSource, ScreenInputSender, Arc<ScreenBoard>), ScreenBuildError> {
        let screen = CameraScreen::new(self.config.screen.clone(), UiKitTokens::default(), self.config.motion)?;
        let (sender, receiver) = screen_input_channel(self.config.input_queue_capacity);
        let board = Arc::new(ScreenBoard::default());
        let (camera_port, facts): (Box<dyn CameraIntentPort>, Box<dyn CameraFactsSource>) = match env.camera.running() {
            Ok(running) => (
                Box::new(HandleCameraPort::new(running.handle().clone())),
                Box::new(EngineFactsSource::new(running.handle().clone())),
            ),
            Err(_) => (Box::new(AbsentCameraPort), Box::new(AbsentEngineFactsSource)),
        };
        let gravity = match start_gravity_feed(env.services.motion.start_gravity(), self.config.gravity_every_n_frames)
        {
            Ok(feed) => GravityLink::Active(feed),
            Err(reason) => {
                tracing::warn!(target: LOG_TARGET, %reason, "гравитация недоступна: экран без наклона");
                GravityLink::Unavailable { reason }
            }
        };
        let executor = EffectExecutor::new(EffectTargets {
            camera: camera_port,
            haptics: Arc::clone(&env.services.haptics),
            commands: Arc::clone(&env.services.commands),
            render: env.gpu.render_control(),
            capture: Arc::clone(&self.capture) as Arc<dyn CaptureService>,
        });
        let source = CameraScreenSource::new(ScreenSourceParts {
            screen,
            input: receiver,
            facts,
            gravity,
            executor,
            board: Arc::clone(&board),
            report_min_interval: Duration::from_millis(self.config.report_min_interval_ms),
        });
        Ok((source, sender, board))
    }
}
