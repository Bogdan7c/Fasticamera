//! Поток `fasticamera-camera`: запуск, цикл и остановка движка.
//!
//! [`CameraRuntime::spawn`] валидирует конфиг, создаёт ящик и запускает поток. На потоке:
//! фабрика создаёт бэкенд (отказ — `Fatal::BackendUnavailable`, но поток живёт и отвечает
//! на команды `EngineFailed`), движок читает каталог и входит в цикл «ждать ящик до срока
//! таймера движка → отдать вход движку → опубликовать события и снимок».
//!
//! Каждая итерация идёт под `catch_unwind`: паника (в бэкенде или в самом движке) переводит
//! движок в `Fatal::EnginePanicked`, закрывает устройство, и дальше поток только отвечает
//! на команды (по образцу `Faulted` в `render-loop`).

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::backend::{BackendEventListener, BackendEventSink, BackendFactory, BackendFailure};
use crate::command::{CameraCommand, CommandId};
use crate::config::{CameraEngineConfig, CameraEngineConfigError};
use crate::engine::CameraEngine;
use crate::handle::{CameraHandle, EngineOutput};
use crate::inbox::{CameraInbox, InboxItem};
use crate::status::AppPresence;

const LOG_TARGET: &str = "fasticamera::camera::runtime";

/// Имя потока камеры (видно в `ps -T`, трассировках и crash-отчётах).
pub const CAMERA_THREAD_NAME: &str = "fasticamera-camera";

/// Почему движок не запустился.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeSpawnError {
    #[error("неверный конфиг движка камеры: {0}")]
    InvalidConfig(#[from] CameraEngineConfigError),
    #[error("не удалось создать поток камеры: {0}")]
    ThreadSpawn(#[source] io::Error),
}

/// Чем кончилась остановка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownReport {
    /// Поток закрыл устройство и вышел.
    Stopped,
    /// Поток не вышел за отведённое время (застрял в вызове платформы); он отпущен и выйдет сам.
    TimedOut,
    /// Поток завершился паникой мимо перехвата.
    Panicked,
}

/// Запущенный поток камеры. Владеет только потоком; общаться с движком — через [`CameraHandle`].
pub struct CameraRuntime {
    inbox: Arc<CameraInbox>,
    join: Option<JoinHandle<()>>,
    /// Закрывается (отправитель сброшен) при выходе потока в любом случае, в том числе паникой.
    finished: Receiver<()>,
    /// Сколько ждать выхода потока в `Drop` без `shutdown` (`runtime.drop_shutdown_timeout_ms`).
    drop_shutdown_timeout: Duration,
}

impl CameraRuntime {
    /// Проверяет конфиг и запускает поток `fasticamera-camera`.
    pub fn spawn(
        config: CameraEngineConfig,
        presence: AppPresence,
        factory: BackendFactory,
    ) -> Result<(CameraRuntime, CameraHandle), RuntimeSpawnError> {
        config.validate()?;
        let drop_shutdown_timeout = Duration::from_millis(config.runtime.drop_shutdown_timeout_ms);
        let inbox = Arc::new(CameraInbox::new(config.queues));
        let output = Arc::new(EngineOutput::new(presence, config.queues.outward_event_capacity));
        let (finished_signal, finished) = mpsc::channel();
        let worker = Worker {
            config,
            presence,
            factory,
            inbox: Arc::clone(&inbox),
            output: Arc::clone(&output),
            finished_signal,
        };
        let join = thread::Builder::new()
            .name(CAMERA_THREAD_NAME.to_owned())
            .spawn(move || worker.run())
            .map_err(RuntimeSpawnError::ThreadSpawn)?;
        let handle = CameraHandle::new(Arc::clone(&inbox), output);
        Ok((
            CameraRuntime {
                inbox,
                join: Some(join),
                finished,
                drop_shutdown_timeout,
            },
            handle,
        ))
    }

    /// Просит поток закрыть устройство и выйти; ждёт не дольше `timeout`.
    pub fn shutdown(mut self, timeout: Duration) -> ShutdownReport {
        self.stop_within(timeout)
    }

    fn stop_within(&mut self, timeout: Duration) -> ShutdownReport {
        let Some(join) = self.join.take() else {
            return ShutdownReport::Stopped;
        };
        self.inbox.request_stop();
        match self.finished.recv_timeout(timeout) {
            Err(RecvTimeoutError::Timeout) => {
                tracing::error!(target: LOG_TARGET, ?timeout, "поток камеры не остановился вовремя, отпускаем его");
                ShutdownReport::TimedOut
            }
            // Сообщений поток не шлёт: `Disconnected` и есть «вышел».
            Ok(()) | Err(RecvTimeoutError::Disconnected) => match join.join() {
                Ok(()) => ShutdownReport::Stopped,
                Err(_) => {
                    tracing::error!(target: LOG_TARGET, "поток камеры завершился паникой мимо перехвата");
                    ShutdownReport::Panicked
                }
            },
        }
    }
}

/// `Drop` без `shutdown` — тоже корректная остановка: просим поток выйти и ждём не дольше
/// `runtime.drop_shutdown_timeout_ms` из конфига. Бесконечного ожидания нет: застрявший в платформе поток отпускается.
impl Drop for CameraRuntime {
    fn drop(&mut self) {
        let report = self.stop_within(self.drop_shutdown_timeout);
        if report != ShutdownReport::Stopped {
            tracing::warn!(target: LOG_TARGET, ?report, "поток камеры остановлен в Drop не чисто");
        }
    }
}

/// Помечает ящик остановленным при любом выходе из потока (в том числе паникой).
struct StoppedOnDrop(Arc<CameraInbox>);

impl Drop for StoppedOnDrop {
    fn drop(&mut self) {
        self.0.mark_stopped();
    }
}

/// Всё, что уходит на поток камеры.
struct Worker {
    config: CameraEngineConfig,
    presence: AppPresence,
    factory: BackendFactory,
    inbox: Arc<CameraInbox>,
    output: Arc<EngineOutput>,
    finished_signal: Sender<()>,
}

impl Worker {
    fn run(self) {
        let Worker {
            config,
            presence,
            factory,
            inbox,
            output,
            finished_signal,
        } = self;
        // Порядок удаления: сначала ящик помечается остановленным, потом сигнал «вышел».
        let _finished = finished_signal;
        let _stopped = StoppedOnDrop(Arc::clone(&inbox));
        let engine = build_engine(config, presence, factory, &inbox);
        let mut state = LoopState {
            engine,
            inbox: &inbox,
            output: &output,
        };
        state.start();
        state.run_loop();
        state.finish();
    }
}

/// Создаёт бэкенд фабрикой (под перехватом паники) и движок над ним.
fn build_engine(
    config: CameraEngineConfig,
    presence: AppPresence,
    factory: BackendFactory,
    inbox: &Arc<CameraInbox>,
) -> CameraEngine {
    let listener: Arc<dyn BackendEventListener> = Arc::clone(inbox) as Arc<dyn BackendEventListener>;
    let sink = BackendEventSink::new(listener);
    match catch_unwind(AssertUnwindSafe(|| factory(sink))) {
        Ok(Ok(backend)) => CameraEngine::new(config, presence, backend),
        Ok(Err(failure)) => CameraEngine::failed_backend(config, presence, failure),
        Err(payload) => CameraEngine::failed_backend(
            config,
            presence,
            BackendFailure {
                operation: "backend_factory",
                code: 0,
                detail: format!("паника при создании бэкенда: {}", panic_message(payload.as_ref())),
            },
        ),
    }
}

/// Текст паники из её полезной нагрузки.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "паника без текста".to_owned()
    }
}

/// Чем кончилась итерация движка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StepResult {
    Completed,
    /// Итерация запаниковала; движок уже переведён в `Fatal::EnginePanicked`.
    Panicked,
}

/// Цикл потока: движок + откуда брать входы + куда публиковать.
struct LoopState<'a> {
    engine: CameraEngine,
    inbox: &'a CameraInbox,
    output: &'a EngineOutput,
}

impl LoopState<'_> {
    fn start(&mut self) {
        // Движок без бэкенда уже в Fatal: каталог читать не у кого.
        if !self.engine.is_fatal() {
            self.guarded(|engine, now| engine.load_catalog(now));
        } else {
            self.publish();
        }
    }

    fn run_loop(&mut self) {
        loop {
            match self.inbox.wait_next(self.engine.next_deadline()) {
                InboxItem::Stop => return,
                InboxItem::Timeout => {
                    self.guarded(|engine, now| engine.on_timer(now));
                }
                InboxItem::BackendEvent(event) => {
                    self.guarded(|engine, now| engine.handle_backend_event(event, now));
                }
                InboxItem::Command { id, command } => self.run_command(id, command),
            }
        }
    }

    fn run_command(&mut self, id: CommandId, command: CameraCommand) {
        let name = command.name();
        let step = self.guarded(|engine, now| engine.handle_command(id, command, now));
        if step == StepResult::Panicked {
            // Паника до публикации итога оставила бы команду без ответа.
            self.engine.answer_if_unanswered(id, name);
            self.publish();
        }
    }

    /// Одна итерация движка под перехватом паники; затем публикация.
    fn guarded(&mut self, step: impl FnOnce(&mut CameraEngine, Instant)) -> StepResult {
        let now = Instant::now();
        let outcome = catch_unwind(AssertUnwindSafe(|| step(&mut self.engine, now)));
        let result = match outcome {
            Ok(()) => StepResult::Completed,
            Err(payload) => {
                self.engine.fail_after_panic(panic_message(payload.as_ref()));
                StepResult::Panicked
            }
        };
        self.publish();
        result
    }

    fn publish(&mut self) {
        let events = self.engine.drain_events();
        self.output.publish(self.engine.status(), events);
    }

    /// Остановка: закрыть устройство (под перехватом паники) и опубликовать итог.
    fn finish(&mut self) {
        let outcome = catch_unwind(AssertUnwindSafe(|| self.engine.shutdown()));
        if let Err(payload) = outcome {
            tracing::error!(
                target: LOG_TARGET,
                message = panic_message(payload.as_ref()),
                "паника при остановке движка камеры"
            );
        }
        self.publish();
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
