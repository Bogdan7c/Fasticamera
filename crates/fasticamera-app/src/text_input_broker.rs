//! Очередь запросов текста: кто спросил, какой диалог открыт и куда отдать ответ (решение 197).
//!
//! Платформа умеет показать ОДИН диалог, а просить текст могут разные места приложения. Брокер — единственный
//! владелец «открытого запроса» и правил протокола (`shell_contract::text_input`):
//! * [`TextInputBroker::request`] — поставить запрос; если диалог уже открыт, старый запрос вытесняется
//!   (получает `Superseded`), новый показывается;
//! * [`TextInputBroker::deliver_platform_result`] — принять ответ платформы (его маршрутизатор берёт из
//!   `ShellEvent::TextInput`): ответ на открытый запрос нормализуется и уходит приёмнику, устаревший
//!   (не тот номер) отбрасывается со счётчиком;
//! * [`TextInputBroker::platform_refused`] — платформа отказалась выполнить команду показа
//!   (`PlatformCommandFailed`): открытый запрос получает `Unavailable`, а не висит вечно.
//!
//! Куда идут ответы — решает [`TextResultSink`]: экран камеры подключает свой приёмник отдельным шагом
//! (достаточно превратить [`TextInputResult`] в действие экрана). Пока приёмника нет, ответы считаются и
//! логируются, а не теряются молча.
//!
//! Потоки: `request` зовёт поток экрана/рендера, ответы приходят на потоке маршрутизатора. Замок держится только
//! на время неблокирующей постановки команды платформе (`try_send`) и обновления состояния; приёмник вызывается
//! уже БЕЗ замка, чтобы он мог безопасно вызвать брокер повторно.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use shell_contract::commands::CommandOutcome;
use shell_contract::text_input::{
    TextInputOutcome, TextInputResult, TextInputService, TextPurpose, TextRequest, TextRequestId,
};

/// Получатель итогов запросов текста. Реализуется замыканием `Fn(TextInputResult)`.
///
/// Вызывается на потоке маршрутизатора (или на потоке, который вытеснил запрос): реализация обязана быть
/// быстрой и неблокирующей (положить действие в очередь экрана).
pub trait TextResultSink: Send + Sync {
    fn deliver(&self, result: TextInputResult);
}

impl<F> TextResultSink for F
where
    F: Fn(TextInputResult) + Send + Sync,
{
    fn deliver(&self, result: TextInputResult) {
        self(result);
    }
}

/// Что произошло с запросом при постановке.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRequestOutcome {
    /// Команда принята платформой; других открытых запросов не было.
    Shown,
    /// Команда принята; ранее открытый запрос `previous` вытеснен (его приёмник уже получил `Superseded`).
    ShownReplacing { previous: TextRequestId },
    /// Activity нет: диалога не будет и ответа не придёт. Ранее открытый запрос не тронут.
    NotAttached,
    /// Очередь команд платформы полна: диалога не будет и ответа не придёт. Ранее открытый запрос не тронут.
    Backpressure,
}

/// Что брокер сделал с ответом платформы.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultDelivery {
    /// Ответ принадлежит открытому запросу и отдан приёмнику.
    Forwarded,
    /// Приёмника нет: ответ на открытый запрос посчитан и залогирован, запрос закрыт.
    NoSink,
    /// Устаревший ответ (запрос уже вытеснен, закрыт или чужой номер): отброшен.
    Stale,
}

/// Счётчики брокера для диагностики и тестов.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextInputStats {
    /// Ответы платформы, отброшенные как устаревшие.
    pub stale_results: u64,
    /// Ответы, у которых не нашлось приёмника.
    pub results_without_sink: u64,
    /// «Введено» с пустым после очистки текстом (отдан как `Cancelled`).
    pub empty_entries: u64,
}

#[derive(Default)]
struct BrokerState {
    /// Запрос, диалог которого сейчас на экране (или ещё в очереди платформы).
    open: Option<TextRequest>,
    sink: Option<Arc<dyn TextResultSink>>,
    stats: TextInputStats,
}

struct BrokerShared {
    shell: Arc<dyn TextInputService>,
    next_id: AtomicU64,
    state: Mutex<BrokerState>,
}

/// Дешёвый клон-ручка: делит состояние между потоком экрана (запросы) и потоком маршрутизатора (ответы).
#[derive(Clone)]
pub struct TextInputBroker {
    shared: Arc<BrokerShared>,
}

impl TextInputBroker {
    /// `shell` — команда показа диалога (на телефоне `AndroidShellServices`, в тестах fake).
    pub fn new(shell: Arc<dyn TextInputService>) -> Self {
        Self {
            shared: Arc::new(BrokerShared {
                shell,
                // Номера с 1: 0 остаётся «ничего не просили» для отладки.
                next_id: AtomicU64::new(1),
                state: Mutex::new(BrokerState::default()),
            }),
        }
    }

    /// Подключает приёмник итогов (заменяет прежний).
    pub fn set_sink(&self, sink: Arc<dyn TextResultSink>) {
        self.state().sink = Some(sink);
    }

    /// Строит запрос с новым монотонным номером.
    pub fn new_request(&self, purpose: TextPurpose, initial_text: impl Into<String>, max_chars: u32) -> TextRequest {
        TextRequest {
            id: TextRequestId(self.shared.next_id.fetch_add(1, Ordering::Relaxed)),
            purpose,
            initial_text: initial_text.into(),
            max_chars,
        }
    }

    /// Ставит запрос платформе. Открытый ранее запрос вытесняется ТОЛЬКО если платформа приняла новую команду:
    /// при отказе (`NotAttached`, `Backpressure`) старый диалог остаётся как был.
    pub fn request(&self, request: TextRequest) -> TextRequestOutcome {
        let (outcome, superseded_sink) = {
            let mut state = self.state();
            // try_send внутри: замок держится недолго и атомарно связывает «команда принята» с «запрос открыт».
            match self.shared.shell.request_text(&request) {
                CommandOutcome::NotAttached => return TextRequestOutcome::NotAttached,
                CommandOutcome::Backpressure => return TextRequestOutcome::Backpressure,
                CommandOutcome::Queued => {}
            }
            let previous = state.open.replace(request);
            match previous {
                Some(previous) => (
                    TextRequestOutcome::ShownReplacing { previous: previous.id },
                    state.sink.clone().map(|sink| (sink, previous.id)),
                ),
                None => (TextRequestOutcome::Shown, None),
            }
        };
        // Вытесненному — честный итог, уже без замка.
        if let Some((sink, previous_id)) = superseded_sink {
            sink.deliver(TextInputResult {
                id: previous_id,
                outcome: TextInputOutcome::Superseded,
            });
        }
        outcome
    }

    /// Ответ платформы (`ShellEvent::TextInput`). Нормализует введённый текст по правилам открытого запроса.
    pub fn deliver_platform_result(&self, result: TextInputResult) -> ResultDelivery {
        let (final_result, sink) = {
            let mut state = self.state();
            let matches_open = state.open.as_ref().is_some_and(|open| open.id == result.id);
            if !matches_open {
                state.stats.stale_results += 1;
                tracing::warn!(target: "fasticamera::text_input", id = result.id.0, "устаревший ответ диалога отброшен");
                return ResultDelivery::Stale;
            }
            let open = state.open.take();
            let final_result = match (&result.outcome, open) {
                (TextInputOutcome::Entered(raw_text), Some(open)) => match open.normalize_entry(raw_text) {
                    Some(text) => TextInputResult {
                        id: result.id,
                        outcome: TextInputOutcome::Entered(text),
                    },
                    None => {
                        // Kotlin не даёт сохранить пустое, так что сюда попадаем только при нарушении контракта.
                        state.stats.empty_entries += 1;
                        tracing::warn!(target: "fasticamera::text_input", id = result.id.0, "пустой ввод отдан как отмена");
                        TextInputResult {
                            id: result.id,
                            outcome: TextInputOutcome::Cancelled,
                        }
                    }
                },
                _ => result,
            };
            let sink = state.sink.clone();
            if sink.is_none() {
                state.stats.results_without_sink += 1;
                tracing::warn!(target: "fasticamera::text_input", "приёмник ответов не подключён: ответ диалога не доставлен");
            }
            (final_result, sink)
        };
        match sink {
            Some(sink) => {
                sink.deliver(final_result);
                ResultDelivery::Forwarded
            }
            None => ResultDelivery::NoSink,
        }
    }

    /// Платформа не выполнила команду показа (`PlatformCommandFailed(RequestTextInput)`): открытый запрос
    /// получает `Unavailable`. Без открытого запроса — no-op (отказ относился к уже вытесненному).
    pub fn platform_refused(&self) -> ResultDelivery {
        let open_id = self.state().open.as_ref().map(|open| open.id);
        match open_id {
            Some(id) => self.deliver_platform_result(TextInputResult {
                id,
                outcome: TextInputOutcome::Unavailable,
            }),
            None => ResultDelivery::Stale,
        }
    }

    /// Номер открытого запроса (для диагностики и тестов).
    pub fn open_request_id(&self) -> Option<TextRequestId> {
        self.state().open.as_ref().map(|open| open.id)
    }

    pub fn stats(&self) -> TextInputStats {
        self.state().stats
    }

    /// Замок отравлен только паникой приёмника в другом потоке; состояние при этом целое, продолжаем.
    fn state(&self) -> MutexGuard<'_, BrokerState> {
        self.shared
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[cfg(test)]
#[path = "text_input_broker_tests.rs"]
mod tests;
