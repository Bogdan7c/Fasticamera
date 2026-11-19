//! Исполнитель эффектов экрана камеры (сессия 12): превращает намерения [`Effect`] в вызовы сервисов.
//!
//! Экран ничего не вызывает сам — он отдаёт `Effect`, а этот модуль знает, кому они адресованы:
//! * `Camera` → ручка движка ([`CameraIntentPort`]);
//! * `Haptic` → [`HapticsService`];
//! * `OpenSystemGallery` → [`ShellCommands::open_system_gallery`];
//! * `Viewfinder` → команда рендеру `SetFraming` через [`RenderControl`] (решение 93: место кадра решает экран);
//! * `Capture` → [`CaptureService`] (фейк до сессии 14).
//!
//! Исполнитель живёт на ПОТОКЕ РЕНДЕРА (внутри источника UI) и ничего не ждёт: всё — постановка в ограниченные
//! очереди. Желаемая компенсация экспозиции — исключение из «потеряно навсегда»: экран уже показывает новое EV,
//! поэтому при `QueueFull` последнее значение досылается на следующих кадрах, пока не примется (последнее
//! побеждает; [`EffectExecutor::resend_pending_exposure_bias`]). Фокус и блокировка не досылаются: к моменту
//! повтора они устарели бы (точку уже сменили или отпустили). Любой отказ — счётчик по виду ([`EffectCounters`]), не паника и не тишина. Запрос снимка, который
//! сервис не принял, превращается в обратное действие экрану `CaptureFinished(Failed)`: иначе затвор остался бы
//! в «снимаю» навсегда ([`EffectExecutor::execute`] возвращает такие действия вызывающему).

use std::sync::Arc;

use camera_contract::control::ExposureBias;
use camera_core::{CameraHandle, CommandId, CommandRejected};
use camera_screen::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequest};
use camera_screen::{Effect, FramingIntent, ScreenAction, ScreenCameraIntent};
use render_loop::viewfinder::{FramingDestination, NormalizedScreenRect, ViewfinderCommand, ViewfinderFraming};
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::{CommandOutcome, ShellCommands};
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticsService};

use crate::camera_screen_capture::{CaptureService, SubmitOutcome, SubmitRejection};
use crate::viewfinder_control::{ControlDelivery, RenderAdjustment, RenderControl};

const LOG_TARGET: &str = "fasticamera::camera_screen";

/// Итог доставки намерения камере. Не `Result`: «движка нет» — не ошибка очереди.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraIntentOutcome {
    /// Команда принята очередью движка.
    Sent(CommandId),
    /// Очередь движка полна.
    QueueFull,
    /// Поток движка остановлен.
    EngineStopped,
    /// Движка камеры в приложении нет (не поднялся).
    NoEngine,
}

/// Куда экран шлёт намерения камере. Узкий контракт: исполнитель не знает про `CameraHandle`.
pub trait CameraIntentPort: Send {
    fn deliver(&self, intent: ScreenCameraIntent) -> CameraIntentOutcome;
}

/// Порт на ручку настоящего движка: имена намерений совпадают с методами `CameraHandle`.
pub struct HandleCameraPort {
    handle: CameraHandle,
}

impl HandleCameraPort {
    pub fn new(handle: CameraHandle) -> Self {
        Self { handle }
    }
}

impl CameraIntentPort for HandleCameraPort {
    fn deliver(&self, intent: ScreenCameraIntent) -> CameraIntentOutcome {
        let sent = match intent {
            ScreenCameraIntent::FocusAndExposeAt(point) => self.handle.focus_and_expose_at(point),
            ScreenCameraIntent::LockFocusAndExposureAt(point) => self.handle.lock_focus_and_exposure_at(point),
            ScreenCameraIntent::ResetPointOfInterest => self.handle.reset_point_of_interest(),
            ScreenCameraIntent::SetExposureBias(bias) => self.handle.set_exposure_bias(bias),
        };
        match sent {
            Ok(command) => CameraIntentOutcome::Sent(command),
            Err(CommandRejected::QueueFull) => CameraIntentOutcome::QueueFull,
            Err(CommandRejected::EngineStopped) => CameraIntentOutcome::EngineStopped,
        }
    }
}

/// Порт, когда движка нет: каждое намерение — честное `NoEngine`.
pub struct AbsentCameraPort;

impl CameraIntentPort for AbsentCameraPort {
    fn deliver(&self, _intent: ScreenCameraIntent) -> CameraIntentOutcome {
        CameraIntentOutcome::NoEngine
    }
}

/// Счётчики намерений камере.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CameraIntentCounters {
    pub sent: u64,
    pub queue_full: u64,
    pub engine_stopped: u64,
    pub no_engine: u64,
    /// Досылки желаемого EV, принятые очередью движка (в `sent` не входят: это повтор, а не новое намерение).
    pub exposure_resends_accepted: u64,
    /// Досылки, снова упёршиеся в полную очередь (значение остаётся ждать).
    pub exposure_resends_queue_full: u64,
    /// Досылки, от которых отказались навсегда: движок остановлен или его нет.
    pub exposure_resends_abandoned: u64,
}

/// Счётчики хаптики по видам итога.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HapticCounters {
    pub queued: u64,
    pub unsupported: u64,
    pub not_attached: u64,
    pub backpressure: u64,
}

/// Счётчики команд оболочке (галерея).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ShellCommandCounters {
    pub queued: u64,
    pub not_attached: u64,
    pub backpressure: u64,
}

/// Счётчики команд рендеру «место видоискателя».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FramingCounters {
    /// Команда лежит в ящике потока рендера.
    pub delivered: u64,
    /// Поток рендера уже завершён.
    pub render_gone: u64,
    /// Рендер не поднимался.
    pub render_unavailable: u64,
    /// Прямоугольник экрана не прошёл проверку типа рендера (команда не отправлена).
    pub invalid_rect: u64,
}

/// Счётчики запросов снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaptureCounters {
    pub accepted: u64,
    pub rejected_busy: u64,
    pub rejected_unavailable: u64,
}

/// Все счётчики исполнителя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct EffectCounters {
    pub camera: CameraIntentCounters,
    pub haptics: HapticCounters,
    pub gallery: ShellCommandCounters,
    pub framing: FramingCounters,
    pub capture: CaptureCounters,
}

/// Сервисы, которым адресованы эффекты (собираются композицией один раз на экран).
pub struct EffectTargets {
    pub camera: Box<dyn CameraIntentPort>,
    pub haptics: Arc<dyn HapticsService>,
    pub commands: Arc<dyn ShellCommands>,
    pub render: Arc<dyn RenderControl>,
    pub capture: Arc<dyn CaptureService>,
}

/// Исполнитель эффектов.
pub struct EffectExecutor {
    targets: EffectTargets,
    counters: EffectCounters,
    /// Время показа текущего кадра UI (часы потока рендера): им фейк-сервис снимка отсчитывает задержку ответа.
    frame_time: MonotonicNanos,
    /// Последнее желаемое EV, которое камера ещё не приняла из-за полной очереди. Новое значение заменяет старое.
    pending_exposure_bias: Option<ExposureBias>,
}

impl EffectExecutor {
    pub fn new(targets: EffectTargets) -> Self {
        Self {
            targets,
            counters: EffectCounters::default(),
            frame_time: MonotonicNanos::from_nanos(0),
            pending_exposure_bias: None,
        }
    }

    /// Начало кадра UI: запоминает время показа и даёт сервису снимка выдать созревшие ответы.
    /// Вызывается ДО разбора входа кадра, чтобы ответ `CaptureFinished` попал в этот же кадр.
    pub fn begin_frame(&mut self, now: MonotonicNanos) {
        self.frame_time = now;
        self.targets.capture.poll_completions(now);
    }

    /// Ждёт ли желаемое EV досылки (диагностика и тесты).
    pub fn pending_exposure_bias(&self) -> Option<ExposureBias> {
        self.pending_exposure_bias
    }

    /// Досылает последнее желаемое EV, не принятое из-за полной очереди. Вызывается на каждом кадре ДО новых
    /// эффектов, поэтому более свежее значение из этого же кадра всегда идёт позже и побеждает.
    pub fn resend_pending_exposure_bias(&mut self) {
        let Some(bias) = self.pending_exposure_bias else {
            return;
        };
        let counters = &mut self.counters.camera;
        match self.targets.camera.deliver(ScreenCameraIntent::SetExposureBias(bias)) {
            CameraIntentOutcome::Sent(_) => {
                counters.exposure_resends_accepted += 1;
                self.pending_exposure_bias = None;
            }
            // Очередь всё ещё полна: значение остаётся ждать следующего кадра.
            CameraIntentOutcome::QueueFull => counters.exposure_resends_queue_full += 1,
            // Движка нет и не будет: ждать бессмысленно, но отказ не тихий.
            CameraIntentOutcome::EngineStopped | CameraIntentOutcome::NoEngine => {
                counters.exposure_resends_abandoned += 1;
                self.pending_exposure_bias = None;
                tracing::warn!(target: LOG_TARGET, ?bias, "движка камеры нет: досылка EV отменена");
            }
        }
    }

    pub fn counters(&self) -> EffectCounters {
        self.counters
    }

    /// Исполняет эффекты по порядку. Возвращает действия, которые нужно вернуть экрану (отвергнутый снимок).
    pub fn execute(&mut self, effects: Vec<Effect>) -> Vec<ScreenAction> {
        let mut feedback = Vec::new();
        for effect in effects {
            match effect {
                Effect::Camera(intent) => self.deliver_camera_intent(intent),
                Effect::Haptic(pattern) => self.play_haptic(pattern),
                Effect::OpenSystemGallery => self.open_gallery(),
                Effect::Viewfinder(intent) => self.place_viewfinder(intent),
                Effect::Capture(request) => feedback.extend(self.submit_capture(request)),
            }
        }
        feedback
    }

    fn deliver_camera_intent(&mut self, intent: ScreenCameraIntent) {
        let counters = &mut self.counters.camera;
        // Только EV досылается: остальные намерения при отказе устаревают (см. доку модуля).
        let exposure_bias = match intent {
            ScreenCameraIntent::SetExposureBias(bias) => Some(bias),
            ScreenCameraIntent::FocusAndExposeAt(_)
            | ScreenCameraIntent::LockFocusAndExposureAt(_)
            | ScreenCameraIntent::ResetPointOfInterest => None,
        };
        match self.targets.camera.deliver(intent) {
            CameraIntentOutcome::Sent(_) => {
                counters.sent += 1;
                // Свежее значение принято: ждавшее старое устарело.
                if exposure_bias.is_some() {
                    self.pending_exposure_bias = None;
                }
            }
            CameraIntentOutcome::QueueFull => {
                counters.queue_full += 1;
                if let Some(bias) = exposure_bias {
                    self.pending_exposure_bias = Some(bias);
                    tracing::warn!(target: LOG_TARGET, ?intent, "очередь движка полна: EV будет досылаться");
                } else {
                    tracing::warn!(target: LOG_TARGET, ?intent, "очередь движка полна: намерение экрана потеряно");
                }
            }
            CameraIntentOutcome::EngineStopped => {
                counters.engine_stopped += 1;
                if exposure_bias.is_some() {
                    self.pending_exposure_bias = None;
                }
                tracing::warn!(target: LOG_TARGET, ?intent, "движок камеры остановлен: намерение экрана потеряно");
            }
            CameraIntentOutcome::NoEngine => {
                counters.no_engine += 1;
                if exposure_bias.is_some() {
                    self.pending_exposure_bias = None;
                }
            }
        }
    }

    fn play_haptic(&mut self, pattern: HapticPattern) {
        let counters = &mut self.counters.haptics;
        match self.targets.haptics.play(pattern) {
            HapticOutcome::Queued(_) => counters.queued += 1,
            HapticOutcome::Unsupported => counters.unsupported += 1,
            HapticOutcome::NotAttached => counters.not_attached += 1,
            HapticOutcome::Backpressure => counters.backpressure += 1,
        }
    }

    fn open_gallery(&mut self) {
        let counters = &mut self.counters.gallery;
        match self.targets.commands.open_system_gallery() {
            CommandOutcome::Queued => counters.queued += 1,
            CommandOutcome::NotAttached => counters.not_attached += 1,
            CommandOutcome::Backpressure => counters.backpressure += 1,
        }
    }

    fn place_viewfinder(&mut self, intent: FramingIntent) {
        let counters = &mut self.counters.framing;
        let framing = match framing_of(intent) {
            Ok(framing) => framing,
            Err(error) => {
                counters.invalid_rect += 1;
                tracing::error!(target: LOG_TARGET, %error, ?intent, "место видоискателя от экрана неверно: команда не отправлена");
                return;
            }
        };
        // Напрямую через `adjust`, а не через `*_for_scenario`: место кадра — настройка продукта, а не сценария,
        // конец сценария её не откатывает (решение 93: место живёт в бэкенде рендера).
        let delivery = self
            .targets
            .render
            .adjust(RenderAdjustment::Viewfinder(ViewfinderCommand::SetFraming(framing)));
        match delivery {
            ControlDelivery::Delivered => counters.delivered += 1,
            ControlDelivery::RenderGone => counters.render_gone += 1,
            ControlDelivery::RenderUnavailable { .. } => counters.render_unavailable += 1,
        }
    }

    /// Запрос снимка; отвергнутый — действие `CaptureFinished(Failed)` обратно экрану.
    fn submit_capture(&mut self, request: ShotRequest) -> Option<ScreenAction> {
        let counters = &mut self.counters.capture;
        let rejection = match self.targets.capture.submit(request, self.frame_time) {
            SubmitOutcome::Accepted => {
                counters.accepted += 1;
                return None;
            }
            SubmitOutcome::Rejected(rejection) => rejection,
        };
        let reason = match rejection {
            SubmitRejection::Busy => {
                counters.rejected_busy += 1;
                CaptureFailureReason::Processing
            }
            SubmitRejection::ServiceUnavailable => {
                counters.rejected_unavailable += 1;
                CaptureFailureReason::Camera
            }
        };
        tracing::warn!(target: LOG_TARGET, ?rejection, "сервис снимка отверг запрос: экрану возвращён провал");
        Some(ScreenAction::CaptureFinished {
            request: request.request,
            outcome: CaptureOutcome::Failed(reason),
        })
    }
}

/// Намерение экрана → команда рендеру. У экрана свой тип прямоугольника, у рендера — проверяемый: перевод с ошибкой.
fn framing_of(intent: FramingIntent) -> Result<ViewfinderFraming, render_loop::viewfinder::ScreenRectError> {
    let rect = intent.destination;
    Ok(ViewfinderFraming {
        crop: intent.crop,
        destination: FramingDestination::ScreenRect(NormalizedScreenRect::new(
            rect.left,
            rect.top,
            rect.width,
            rect.height,
        )?),
    })
}

#[cfg(test)]
#[path = "camera_screen_effects_tests.rs"]
mod tests;
