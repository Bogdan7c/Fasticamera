//! Исполнитель намерений по белому (white-balance 02): ручные правки, которые досылаются при полной очереди, и
//! команда «взять», которая не досылается.
//!
//! # Зачем отдельно от остальных намерений камере
//! Экран уже показывает новое значение шкалы, поэтому ручная правка, отвергнутая полной очередью, не должна
//! пропасть (образец — досылка EV). Но у белого две оси и «целиком», и правки живут независимо: отложенная
//! температура и отложенный оттенок не затирают друг друга, а `SetWhite` замещает обе.
//!
//! # Правила
//! * Всякая ручная правка сначала ложится в [`PendingWhite`], потом отложенное досылается **по порядку**
//!   (целиком → температура → оттенок). Так до движка правки доходят в порядке нажатий, а более новая
//!   правка оси ложится поверх отложенного `SetWhite`, не меняя порядка.
//! * `QueueFull` — остаток ждёт следующего кадра ([`WhiteEffects::resend_pending`]); последнее побеждает.
//! * `EngineStopped`/`NoEngine` — отложенное сбрасывается: движка нет, белая останется той, что держит он
//!   (шкала экрана встанет по факту).
//! * `TakeWhite` не досылается: к моменту повтора человек мог уже передумать. Недоставленная команда превращается
//!   в обратное действие экрану [`ScreenAction::WhiteTakeNotDelivered`] (тряска и вибрация «ошибка», как у отказа
//!   источника). Принятая команда «взять» сбрасывает отложенное ручное: оно было раньше нажатия, человек передумал.
//!   Недоставленная — отложенного не трогает: рука всё ещё хочет своё.
//! * `FollowWhite` («Auto», WB44) живёт по тем же правилам, что `TakeWhite`: сначала пытаемся доставить отложенную
//!   ручную правку, сама команда не досылается, принятая команда сбрасывает отложенное (иначе досланная позже
//!   старая правка выключила бы слежение, которое человек только что включил). Недоставленная возвращается экрану
//!   действием [`ScreenAction::WhiteFollowNotDelivered`]: только вибрация «ошибка», «Phone» не трясётся —
//!   это не её отказ.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{WhiteAdjustment, WhiteSourceId};
use camera_screen::{ScreenAction, ScreenCameraIntent};

use crate::camera_screen_effects::{CameraIntentOutcome, CameraIntentPort};

const LOG_TARGET: &str = "fasticamera::camera_screen::white";

/// Ручная правка белого, пришедшая с экрана.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ManualWhiteEdit {
    /// Белое целиком (кнопка света): замещает любые отложенные правки.
    Whole(WhiteBalanceSetting),
    /// Одна ось (шкала): вторую ось держит движок.
    Axis(WhiteAdjustment),
}

/// Ручные правки, которые движок ещё не принял. Инвариант: если есть `whole`, осей отдельно нет — они уже
/// вложены в него (правка оси поверх отложенного `SetWhite` меняет его самого).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PendingWhite {
    whole: Option<WhiteBalanceSetting>,
    temperature: Option<ColorTemperature>,
    tint: Option<Tint>,
}

impl PendingWhite {
    /// Отложенного нет.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Отложенное белое целиком (с вложенными поверх правками осей), если есть.
    pub fn whole(&self) -> Option<WhiteBalanceSetting> {
        self.whole
    }

    /// Отложенная правка температуры (отдельно от «целиком»), если есть.
    pub fn temperature(&self) -> Option<ColorTemperature> {
        self.temperature
    }

    /// Отложенная правка оттенка (отдельно от «целиком»), если есть.
    pub fn tint(&self) -> Option<Tint> {
        self.tint
    }

    /// Новая правка ложится поверх отложенного: «целиком» замещает всё, ось заменяет только свою ось.
    fn absorb(&mut self, edit: ManualWhiteEdit) {
        match edit {
            ManualWhiteEdit::Whole(setting) => {
                *self = Self {
                    whole: Some(setting),
                    ..Self::default()
                };
            }
            ManualWhiteEdit::Axis(adjustment) => match (&mut self.whole, adjustment) {
                (Some(whole), adjustment) => *whole = adjustment.applied_to(*whole),
                (None, WhiteAdjustment::Temperature(temperature)) => self.temperature = Some(temperature),
                (None, WhiteAdjustment::Tint(tint)) => self.tint = Some(tint),
            },
        }
    }

    /// Следующее намерение по порядку: целиком, потом температура, потом оттенок.
    fn front(&self) -> Option<ScreenCameraIntent> {
        if let Some(whole) = self.whole {
            return Some(ScreenCameraIntent::SetWhite(whole));
        }
        if let Some(temperature) = self.temperature {
            return Some(ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Temperature(
                temperature,
            )));
        }
        self.tint
            .map(|tint| ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Tint(tint)))
    }

    /// Убирает то, что показывал [`Self::front`] (оно принято движком).
    fn pop_front(&mut self) {
        if self.whole.take().is_some() {
            return;
        }
        if self.temperature.take().is_some() {
            return;
        }
        self.tint = None;
    }
}

/// Счётчики белых намерений. Свои, а не общие камерные: видно, что именно с белым не дошло.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct WhiteIntentCounters {
    /// Намерения (в том числе досланные), принятые очередью движка.
    pub sent: u64,
    /// Из них досланные на следующих кадрах после полной очереди.
    pub resends_accepted: u64,
    /// Отказы полной очереди (ручная правка осталась ждать, `TakeWhite` потерян).
    pub queue_full: u64,
    /// Отказы «движок остановлен» (отложенное сброшено).
    pub engine_stopped: u64,
    /// Отказы «движка нет» (отложенное сброшено).
    pub no_engine: u64,
    /// Команды «взять», которые не дошли до движка и вернулись экрану отказом.
    pub takes_not_delivered: u64,
    /// Команды «следить» («Auto»), которые не дошли до движка и вернулись экрану вибрацией «ошибка».
    pub follows_not_delivered: u64,
}

/// Команда белого, которая не досылается (человек мог передумать): «взять» и «следить». Различие нужно для
/// намерения, обратного действия экрану и счётчика: у каждой свой, остальная логика доставки общая.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UnretriedCommand {
    /// «Взять белое» ([`ScreenCameraIntent::TakeWhite`]).
    Take,
    /// «Следить за белым» ([`ScreenCameraIntent::FollowWhite`]).
    Follow,
}

impl UnretriedCommand {
    fn intent(self, source: WhiteSourceId) -> ScreenCameraIntent {
        match self {
            UnretriedCommand::Take => ScreenCameraIntent::TakeWhite(source),
            UnretriedCommand::Follow => ScreenCameraIntent::FollowWhite(source),
        }
    }

    /// Что вернуть экрану, если команда не дошла.
    fn not_delivered_action(self) -> ScreenAction {
        match self {
            UnretriedCommand::Take => ScreenAction::WhiteTakeNotDelivered,
            UnretriedCommand::Follow => ScreenAction::WhiteFollowNotDelivered,
        }
    }
}

/// Откуда идёт досылка: от этого зависит, в какой счётчик записать принятое.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlushOrigin {
    /// Новая правка этого кадра.
    NewEdit,
    /// Повтор отложенного с прошлых кадров.
    Resend,
}

/// Исполнитель белых намерений: хранит отложенные ручные правки и счётчики.
#[derive(Debug, Default)]
pub(crate) struct WhiteEffects {
    pending: PendingWhite,
    counters: WhiteIntentCounters,
}

impl WhiteEffects {
    pub(crate) fn pending(&self) -> PendingWhite {
        self.pending
    }

    pub(crate) fn counters(&self) -> WhiteIntentCounters {
        self.counters
    }

    /// Ручная правка: ложится поверх отложенного и уходит к движку вместе с ним, по порядку.
    pub(crate) fn run_manual_edit(&mut self, camera: &dyn CameraIntentPort, edit: ManualWhiteEdit) {
        self.pending.absorb(edit);
        self.flush(camera, FlushOrigin::NewEdit);
    }

    /// Каждый кадр ДО новых эффектов: досылает остаток. Более свежая правка этого кадра пойдёт позже и победит.
    pub(crate) fn resend_pending(&mut self, camera: &dyn CameraIntentPort) {
        self.flush(camera, FlushOrigin::Resend);
    }

    /// Команда «взять белое». Не досылается; не дошла — возвращает экрану отказ.
    pub(crate) fn run_take(&mut self, camera: &dyn CameraIntentPort, source: WhiteSourceId) -> Option<ScreenAction> {
        self.run_unretried(camera, UnretriedCommand::Take, source)
    }

    /// Команда «следить за белым» («Auto»). Не досылается; не дошла — возвращает экрану вибрацию «ошибка».
    pub(crate) fn run_follow(&mut self, camera: &dyn CameraIntentPort, source: WhiteSourceId) -> Option<ScreenAction> {
        self.run_unretried(camera, UnretriedCommand::Follow, source)
    }

    /// Общий путь команд, которые не досылаются. `QueueFull` оставляет отложенное ручное как есть (оно досылается
    /// отдельно), остановленный или отсутствующий движок сбрасывает его.
    fn run_unretried(
        &mut self,
        camera: &dyn CameraIntentPort,
        command: UnretriedCommand,
        source: WhiteSourceId,
    ) -> Option<ScreenAction> {
        // Отложенная ручная правка была раньше нажатия: сначала пытаемся доставить её, чтобы порядок сохранился.
        self.flush(camera, FlushOrigin::NewEdit);
        match camera.deliver(command.intent(source)) {
            CameraIntentOutcome::Sent(_) => {
                self.counters.sent += 1;
                // Команда принята, а старая правка так и не дошла: человек передумал, рука не должна лечь
                // поверх взятого белого.
                self.pending = PendingWhite::default();
                None
            }
            CameraIntentOutcome::QueueFull => {
                self.counters.queue_full += 1;
                Some(self.not_delivered(command, "очередь движка полна"))
            }
            CameraIntentOutcome::EngineStopped => {
                self.counters.engine_stopped += 1;
                self.pending = PendingWhite::default();
                Some(self.not_delivered(command, "движок остановлен"))
            }
            CameraIntentOutcome::NoEngine => {
                self.counters.no_engine += 1;
                self.pending = PendingWhite::default();
                Some(self.not_delivered(command, "движка нет"))
            }
        }
    }

    /// Недоставленная команда: счётчик, журнал и обратное действие экрану — свои у каждой команды.
    fn not_delivered(&mut self, command: UnretriedCommand, reason: &'static str) -> ScreenAction {
        match command {
            UnretriedCommand::Take => self.counters.takes_not_delivered += 1,
            UnretriedCommand::Follow => self.counters.follows_not_delivered += 1,
        }
        tracing::warn!(target: LOG_TARGET, ?command, reason, "команда белого не доставлена: экран узнает");
        command.not_delivered_action()
    }

    /// Досылает отложенное по порядку, пока движок принимает. Полная очередь оставляет остаток на следующий
    /// кадр; остановленный или отсутствующий движок сбрасывает всё отложенное.
    fn flush(&mut self, camera: &dyn CameraIntentPort, origin: FlushOrigin) {
        while let Some(intent) = self.pending.front() {
            match camera.deliver(intent) {
                CameraIntentOutcome::Sent(_) => {
                    self.counters.sent += 1;
                    if origin == FlushOrigin::Resend {
                        self.counters.resends_accepted += 1;
                    }
                    self.pending.pop_front();
                }
                CameraIntentOutcome::QueueFull => {
                    self.counters.queue_full += 1;
                    // Повтор того же отказа на каждом кадре в журнал не пишем: счётчик уже растёт.
                    if origin == FlushOrigin::NewEdit {
                        tracing::warn!(target: LOG_TARGET, ?intent, "очередь движка полна: белое будет досылаться");
                    }
                    return;
                }
                CameraIntentOutcome::EngineStopped => {
                    self.counters.engine_stopped += 1;
                    self.abandon_pending(intent, "движок остановлен");
                    return;
                }
                CameraIntentOutcome::NoEngine => {
                    self.counters.no_engine += 1;
                    self.abandon_pending(intent, "движка нет");
                    return;
                }
            }
        }
    }

    fn abandon_pending(&mut self, intent: ScreenCameraIntent, reason: &'static str) {
        self.pending = PendingWhite::default();
        tracing::warn!(target: LOG_TARGET, ?intent, reason, "ручное белое потеряно: белая останется той, что держит движок");
    }
}

#[cfg(test)]
#[path = "camera_white_effects_tests.rs"]
mod tests;
