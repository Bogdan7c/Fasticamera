//! Пауза слежения за белым на время раздела калибровки: «желаемое против принятого» (решение 289).
//!
//! # Почему не эффект экрана
//! Одноразовое «поставь паузу» / «сними паузу» ломается на любой потере: не принятая очередью команда «сними паузу»
//! оставила бы белое замороженным навсегда. Поэтому экран не шлёт эту команду, а ОТВЕЧАЕТ на запрос
//! `ScreenState::white_tracking_hold()` (желание режима), а приложение каждый кадр сверяет желание с тем, что движок
//! уже принял, и досылает разницу, пока она не принята. Это ровно то, что делает досылка EV и ручного белого, только
//! для уровня, а не для значения.
//!
//! # Правила
//! * «Принято» — команда принята очередью движка (`Sent`): пауза — уровень движка, команда идемпотентна и работает без
//!   потока и в фоне, поэтому ответа «выполнено» ждать не нужно.
//! * Любой отказ (`QueueFull`, `EngineStopped`, `NoEngine`) оставляет разницу непринятой и повторяется на следующем кадре
//!   со своим счётчиком; журнал пишется один раз на серию отказов, а не на каждый кадр.
//! * Начальное принятое — «неизвестно», а не «работает»: экран может быть пересоздан, пока движок ещё держит паузу от
//!   прежнего экрана, и первый кадр обязан сообщить движку своё желание.

use camera_screen::{ScreenCameraIntent, WhiteTrackingHold};

use crate::camera_screen_effects::{CameraIntentOutcome, CameraIntentPort};

const LOG_TARGET: &str = "fasticamera::camera_screen::white";

/// Счётчики доставки паузы слежения.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WhiteHoldCounters {
    /// Желания, принятые очередью движка (включая самое первое и повторы после отказов).
    pub accepted: u64,
    /// Отказы полной очереди: желание повторится на следующем кадре.
    pub queue_full: u64,
    /// Отказы «движок остановлен»: желание повторится (движок может вернуться), но ждать нечего.
    pub engine_stopped: u64,
    /// Отказы «движка нет»: то же.
    pub no_engine: u64,
}

/// Желаемое и принятое состояние паузы.
#[derive(Debug, Default)]
pub(crate) struct WhiteHoldLink {
    /// Что движок уже принял; `None` — ещё ничего не сообщали (экран мог смениться при живом движке).
    acknowledged: Option<WhiteTrackingHold>,
    /// Серия отказов уже записана в журнал: повтор на каждом кадре журнал не засоряет.
    failure_logged: bool,
    counters: WhiteHoldCounters,
}

impl WhiteHoldLink {
    pub(crate) fn counters(&self) -> WhiteHoldCounters {
        self.counters
    }

    /// Что движок принял последним (диагностика и тесты); `None` — ничего.
    pub(crate) fn acknowledged(&self) -> Option<WhiteTrackingHold> {
        self.acknowledged
    }

    /// Каждый кадр: приводит принятое к желаемому. Совпало — ничего не шлёт и почти ничего не стоит.
    pub(crate) fn drive(&mut self, desired: WhiteTrackingHold, camera: &dyn CameraIntentPort) {
        if self.acknowledged == Some(desired) {
            return;
        }
        match camera.deliver(ScreenCameraIntent::HoldWhiteTracking(desired)) {
            CameraIntentOutcome::Sent(_) => {
                self.counters.accepted += 1;
                self.acknowledged = Some(desired);
                self.failure_logged = false;
            }
            CameraIntentOutcome::QueueFull => {
                self.counters.queue_full += 1;
                self.note_failure(desired, "очередь движка полна");
            }
            CameraIntentOutcome::EngineStopped => {
                self.counters.engine_stopped += 1;
                self.note_failure(desired, "движок остановлен");
            }
            CameraIntentOutcome::NoEngine => {
                self.counters.no_engine += 1;
                self.note_failure(desired, "движка нет");
            }
        }
    }

    /// Отказ: разница остаётся непринятой; в журнал — один раз на серию.
    fn note_failure(&mut self, desired: WhiteTrackingHold, reason: &'static str) {
        if !self.failure_logged {
            self.failure_logged = true;
            tracing::warn!(target: LOG_TARGET, ?desired, reason, "пауза слежения за белым не принята: повтор на следующем кадре");
        }
    }
}
