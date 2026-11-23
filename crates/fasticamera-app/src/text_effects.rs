//! Исполнение просьбы экрана «спроси имя» (`Effect::RequestText`, решение 197, 14b).
//!
//! Экран просит текст эффектом, а не вызовом: он не знает ни брокера, ни платформы. Здесь просьба получает
//! номер ([`TextInputBroker::new_request`]), уходит платформе ([`TextInputBroker::request`]), а исход
//! возвращается экрану ОБРАТНЫМ ДЕЙСТВИЕМ — тем же механизмом, что отказ сервиса снимка:
//! * диалог показан (`Shown`, `ShownReplacing`) → `TextRequestIssued { id }`: экран запоминает номер и
//!   узнает свой ответ среди прочих;
//! * диалога не будет (`NotAttached`, `Backpressure`) или брокера нет вовсе → `TextRequestNotSent`: экран не
//!   зависает в ожидании ответа, которого не придёт.
//!
//! Ответ платформы придёт позже и другим путём: брокер отдаст его приёмнику, который кладёт
//! `ScreenAction::TextInputFinished` в канал входа экрана (см. `camera_screen_service`). Исходы считаются по
//! видам и не сливаются в один счётчик.

use camera_screen::ScreenAction;
use look_contract::PRESET_NAME_MAX_CHARS;
use shell_contract::text_input::{TextInputResult, TextPurpose, TextRequestId};

use crate::camera_screen_input::{InputDelivery, ScreenInputInjector};
use crate::text_input_broker::{TextInputBroker, TextRequestOutcome, TextResultSink};

const LOG_TARGET: &str = "fasticamera::text_input";

/// Что стало с просьбами экрана спросить текст.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextRequestCounters {
    /// Платформа приняла команду; других открытых диалогов не было.
    pub shown: u64,
    /// Платформа приняла команду, а открытый прежде диалог вытеснен.
    pub shown_replacing: u64,
    /// Activity нет: диалога не будет.
    pub not_attached: u64,
    /// Очередь команд платформы полна: диалога не будет.
    pub backpressure: u64,
    /// Брокера в сборке нет (ПК-сценарии, тесты экрана): спросить некого.
    pub without_broker: u64,
}

/// Исполнитель просьб о тексте: брокер (если он есть) и счётчики исходов.
#[derive(Default)]
pub struct TextEffects {
    /// `None` — сборка без диалога ввода: просьба честно получает «не отправлено».
    broker: Option<TextInputBroker>,
    counters: TextRequestCounters,
}

impl TextEffects {
    /// Исполнитель поверх брокера запросов текста.
    pub fn new(broker: Option<TextInputBroker>) -> Self {
        Self {
            broker,
            counters: TextRequestCounters::default(),
        }
    }

    /// Счётчики на данный момент.
    pub fn counters(&self) -> TextRequestCounters {
        self.counters
    }

    /// Предел длины ответа по назначению (в символах). Оба назначения сейчас — имя пресета: предел задаёт словарь
    /// вида (`PresetName`), так что диалог не пропустит строку, которую библиотека всё равно отвергла бы.
    fn max_chars_for(purpose: TextPurpose) -> u32 {
        match purpose {
            TextPurpose::NewPresetName | TextPurpose::RenamePreset => {
                u32::try_from(PRESET_NAME_MAX_CHARS).unwrap_or(u32::MAX)
            }
        }
    }

    /// Ставит просьбу и возвращает обратное действие для экрана (см. доку модуля).
    pub fn request(&mut self, purpose: TextPurpose, initial_text: String) -> ScreenAction {
        let Some(broker) = &self.broker else {
            self.counters.without_broker += 1;
            tracing::warn!(target: LOG_TARGET, purpose = purpose.name(), "брокера запросов текста нет: диалог не показан");
            return ScreenAction::TextRequestNotSent;
        };
        let request = broker.new_request(purpose, initial_text, Self::max_chars_for(purpose));
        let id: TextRequestId = request.id;
        match broker.request(request) {
            TextRequestOutcome::Shown => {
                self.counters.shown += 1;
                ScreenAction::TextRequestIssued { id }
            }
            TextRequestOutcome::ShownReplacing { previous } => {
                self.counters.shown_replacing += 1;
                tracing::info!(target: LOG_TARGET, previous = previous.0, id = id.0, "новый диалог вытеснил открытый");
                ScreenAction::TextRequestIssued { id }
            }
            TextRequestOutcome::NotAttached => {
                self.counters.not_attached += 1;
                tracing::warn!(target: LOG_TARGET, purpose = purpose.name(), "Activity нет: диалог не показан");
                ScreenAction::TextRequestNotSent
            }
            TextRequestOutcome::Backpressure => {
                self.counters.backpressure += 1;
                tracing::warn!(target: LOG_TARGET, purpose = purpose.name(), "очередь команд платформы полна: диалог не показан");
                ScreenAction::TextRequestNotSent
            }
        }
    }
}

/// Приёмник итогов брокера: превращает ответ диалога в действие экрана `TextInputFinished` и кладёт его в канал
/// входа ЭТОГО экрана. Новый экран (переустановка) ставит свой приёмник, поэтому ответ всегда идёт живому экрану.
///
/// Вызывается на потоке маршрутизатора (или на том, что вытеснил запрос): `inject` — `try_send`, ничего не ждёт.
/// Переполнение очереди и «экрана уже нет» считаются самим каналом (`overflowed_actions`, `receiver_gone`) и
/// пишутся в лог: ответ не теряется молча.
pub struct TextResultToScreen {
    injector: ScreenInputInjector,
}

impl TextResultToScreen {
    /// Приёмник, доставляющий в канал экрана, чей писатель — `injector`.
    pub fn new(injector: ScreenInputInjector) -> Self {
        Self { injector }
    }
}

impl TextResultSink for TextResultToScreen {
    fn deliver(&self, result: TextInputResult) {
        let id = result.id;
        match self.injector.inject(ScreenAction::TextInputFinished(result)) {
            InputDelivery::Queued => {}
            InputDelivery::Overflowed => {
                tracing::warn!(target: LOG_TARGET, id = id.0, "очередь входа экрана полна: ответ диалога потерян");
            }
            InputDelivery::ReceiverGone => {
                tracing::warn!(target: LOG_TARGET, id = id.0, "экрана уже нет: ответ диалога некому отдать");
            }
        }
    }
}

#[cfg(test)]
#[path = "text_effects_tests.rs"]
mod tests;
