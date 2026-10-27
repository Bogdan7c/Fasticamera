//! Поток рендера `fasticamera-render`: запуск, дескриптор и остановка.
//!
//! Устройство: [`RenderLoop::spawn`] создаёт почтовый ящик и поток; поток владеет бэкендом,
//! часами кадров и машиной состояний (`worker`). Снаружи остаётся [`RenderLoopHandle`]:
//! `inbox()` для маршрутизатора событий, `status()` для сценариев, `stop()` для завершения.
//!
//! Отпускание surface: оболочка в `surfaceDestroyed` ждёт квитанцию до 500 мс. Поток
//! подтверждает её сразу после `detach` (не позже следующей проверки ящика), а при
//! неисправности или остановке подтверждение/уничтожение запроса гарантировано.

mod adpf;
mod frame_clock;
mod latch_sleep;
mod monotonic_clock;
mod present_depth;
mod present_target;
mod present_target_control;
mod present_target_stats;
mod rolling_durations;
mod status;
mod vsync_clock;
mod worker;

use std::io;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use shell_contract::commands::ShellCommands;
use shell_contract::performance_hint::PerformanceHintService;

pub use adpf::{AdpfCommand, AdpfCommandLog, AdpfCommandOutcome, AdpfDiagnostics, AdpfSessionState};
pub use frame_clock::{FrameClock, FrameTick, FrameWait};
pub use latch_sleep::{SleepUntil, SleepUntilOutcome, sleep_until};
pub use monotonic_clock::{ClockReadError, MonotonicTimer, SystemMonotonicTimer};
pub use present_target::{FrameCostEstimate, GpuCostSource};
pub use present_target_control::{
    PresentTargetCommand, PresentTargetCommandLog, PresentTargetCommandOutcome, PresentTargetCommandRecord,
    PresentTargetSelection,
};
pub use present_target_stats::{PresentTargetDiagnostics, VSYNC_SHIFT_BUCKETS, VsyncShiftHistogram};
pub use rolling_durations::DurationSummary;
pub(crate) use rolling_durations::RollingDurations;
pub use status::{BackendHealth, FaultInfo, FaultKind, RenderState, RenderStatus, ScreenWakeStatus};
pub use vsync_clock::VsyncFrameClock;

use crate::backend::DisplayBackend;
use crate::config::{ConfigError, RenderLoopConfig};
use crate::mailbox::{RenderInbox, render_mailbox};
use worker::RenderWorker;

const LOG_TARGET: &str = "fasticamera::render";

/// Имя потока рендера (видно в `ps -T`, трассировках и crash-отчётах).
pub const RENDER_THREAD_NAME: &str = "fasticamera-render";

/// Из чего собирается поток рендера.
pub struct RenderLoopParts<B: DisplayBackend, C: FrameClock> {
    pub config: RenderLoopConfig,
    pub backend: B,
    pub clock: C,
    /// Команды оболочки (поток просит частоту кадров).
    pub commands: Arc<dyn ShellCommands>,
    /// Служба подсказок производительности (ADPF): поток сам создаёт на ней сессию для себя, если
    /// ADPF включён. Нет платформы — `NoPerformanceHints` (честное «не поддерживается»).
    pub performance_hints: Arc<dyn PerformanceHintService>,
}

/// Почему поток не запустился.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    #[error("неверная конфигурация потока рендера: {0}")]
    InvalidConfig(#[from] ConfigError),
    #[error("не удалось создать поток рендера: {0}")]
    ThreadSpawn(#[source] io::Error),
}

/// Фабрика потока рендера.
pub struct RenderLoop;

impl RenderLoop {
    /// Проверяет конфиг и запускает поток `fasticamera-render`.
    pub fn spawn<B, C>(parts: RenderLoopParts<B, C>) -> Result<RenderLoopHandle, SpawnError>
    where
        B: DisplayBackend,
        C: FrameClock,
    {
        parts.config.validate()?;
        let (inbox, receiver) = render_mailbox();
        let shared_status = Arc::new(Mutex::new(RenderStatus::initial(
            parts.config.expected_refresh_period(),
        )));
        let worker = RenderWorker::new(
            parts.config,
            parts.backend,
            parts.clock,
            receiver,
            Arc::clone(&shared_status),
            parts.performance_hints,
        );
        let commands = parts.commands;
        let join = thread::Builder::new()
            .name(RENDER_THREAD_NAME.to_owned())
            .spawn(move || worker.run(commands.as_ref()))
            .map_err(SpawnError::ThreadSpawn)?;
        Ok(RenderLoopHandle {
            inbox,
            status: shared_status,
            join: Some(join),
        })
    }
}

/// Дескриптор запущенного потока. При `Drop` поток останавливается и присоединяется.
pub struct RenderLoopHandle {
    inbox: RenderInbox,
    status: Arc<Mutex<RenderStatus>>,
    join: Option<JoinHandle<()>>,
}

impl RenderLoopHandle {
    /// Ящик для маршрутизатора событий (дёшево клонируется).
    pub fn inbox(&self) -> RenderInbox {
        self.inbox.clone()
    }

    /// Снимок состояния; не блокирует поток рендера дольше короткого замка.
    pub fn status(&self) -> RenderStatus {
        self.status.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Останавливает поток, дожидается его и возвращает итоговый статус. Поток перед выходом
    /// отсоединяет и подтверждает удерживаемую surface.
    pub fn stop(mut self) -> RenderStatus {
        self.stop_and_join();
        self.status()
    }

    fn stop_and_join(&mut self) {
        let Some(join) = self.join.take() else {
            return;
        };
        self.inbox.request_stop();
        if join.join().is_err() {
            // Паники цикла и бэкенда перехвачены внутри (`worker::run`); сюда попадает только то,
            // что упало вне перехвата (например, сам обработчик неисправности).
            tracing::error!(target: LOG_TARGET, "поток рендера завершился паникой вне бэкенда");
            let mut status = self.status.lock().unwrap_or_else(PoisonError::into_inner);
            status.state = RenderState::Faulted;
            status.fault.get_or_insert(FaultInfo {
                kind: FaultKind::WorkerPanic,
                message: "паника в цикле потока рендера".to_owned(),
            });
        }
    }
}

impl Drop for RenderLoopHandle {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}
