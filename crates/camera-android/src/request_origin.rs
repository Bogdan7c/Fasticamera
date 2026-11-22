//! Опознание запросов: «какой запрос породил этот результат» (сессия 08).
//!
//! Каждый запрос получает через `ACaptureRequest_setUserContext` **целочисленный ключ**, а не
//! указатель на данные: NDK хранит и возвращает его в колбэках как `void*`, мы его никогда не
//! разыменовываем. Значит висячего указателя быть не может в принципе, а время жизни записей
//! определяет только эта таблица. Таблица живёт в контексте результатов сессии и освобождается
//! вместе с устройством — после `ACameraDevice_close`, когда колбэки запросов уже не придут.
//!
//! Правила ключей:
//! * [`REPEATING_KEY`] зарезервирован для повторяющегося запроса и в таблице не хранится;
//! * `0` (null) — контекст не передан: **не** «повторяющийся», а [`OriginLookup::NoContext`]. Если
//!   бы null означал повтор, контекст, потерянный на пути в колбэк, выглядел бы как успех;
//! * разовые запросы (триггер, кадры серии, кадр со вспышкой) получают уникальные ключи начиная с
//!   [`FIRST_ONE_SHOT_KEY`] и регистрируются **до** отправки: колбэк на потоке looper-а может
//!   сработать раньше, чем вернётся вызов отправки;
//! * запись снимается, когда пришёл результат или провал кадра ([`RequestOriginTable::settle`]),
//!   либо отправка не удалась ([`RequestOriginTable::unregister`]); таблица ограничена, при
//!   переполнении вытесняется самая старая запись со счётом, а её результат станет `Unknown`.
//!
//! Запасной путь, если на телефоне контекст не доживёт до колбэков (проверяет smoke `3a`):
//! сопоставлять по номеру кадра из `onCaptureStarted` — отправка возвращает `captureSequenceId`,
//! а номера кадров серии идут подряд. Здесь он не реализован.

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, PoisonError};

use camera_contract::three_a::RequestOrigin;

/// Ключ повторяющегося запроса.
pub const REPEATING_KEY: usize = 1;

/// Первый ключ разовых запросов (0 — «нет контекста», 1 — повторяющийся).
pub const FIRST_ONE_SHOT_KEY: usize = 2;

/// Ключ запроса, как он лежит в `void*` контекста.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestKey(usize);

impl RequestKey {
    /// Ключ повторяющегося запроса.
    pub const REPEATING: RequestKey = RequestKey(REPEATING_KEY);

    /// Число для `void*` контекста.
    pub fn as_raw(self) -> usize {
        self.0
    }
}

/// Итог поиска происхождения по значению контекста из колбэка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginLookup {
    /// Запрос опознан.
    Known(RequestOrigin),
    /// Контекст пуст (null): NDK ничего не вернул.
    NoContext,
    /// Ключ незнаком: запись уже снята или вытеснена.
    Unregistered,
}

impl OriginLookup {
    /// Происхождение для события: неопознанное — `Unknown` (счётчик ведёт вызывающий).
    pub fn origin(self) -> RequestOrigin {
        match self {
            OriginLookup::Known(origin) => origin,
            OriginLookup::NoContext | OriginLookup::Unregistered => RequestOrigin::Unknown,
        }
    }
}

/// Итог регистрации разового запроса.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Registration {
    pub key: RequestKey,
    /// Таблица была полна: вытеснена самая старая запись (её результат станет `Unknown`).
    pub evicted_oldest: bool,
}

#[derive(Debug)]
struct TableState {
    next_key: usize,
    entries: VecDeque<(usize, RequestOrigin)>,
}

/// Таблица происхождений разовых запросов сессии.
#[derive(Debug)]
pub struct RequestOriginTable {
    capacity: usize,
    state: Mutex<TableState>,
}

impl RequestOriginTable {
    /// `capacity` — сколько разовых запросов могут быть «в полёте» одновременно (минимум 1).
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            state: Mutex::new(TableState {
                next_key: FIRST_ONE_SHOT_KEY,
                entries: VecDeque::new(),
            }),
        }
    }

    fn state(&self) -> MutexGuard<'_, TableState> {
        // Паника другого колбэка под замком не должна глушить опознание следующих результатов.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Регистрирует происхождение разового запроса и выдаёт его ключ.
    pub fn register(&self, origin: RequestOrigin) -> Registration {
        let mut state = self.state();
        let key = state.next_key;
        // Ключи не переиспользуют зарезервированные 0 и 1 даже при (недостижимом) переполнении.
        state.next_key = key.checked_add(1).unwrap_or(FIRST_ONE_SHOT_KEY);
        let evicted_oldest = state.entries.len() >= self.capacity;
        if evicted_oldest {
            state.entries.pop_front();
        }
        state.entries.push_back((key, origin));
        Registration {
            key: RequestKey(key),
            evicted_oldest,
        }
    }

    /// Находит происхождение по значению контекста из колбэка (запись остаётся).
    pub fn resolve(&self, raw_context: usize) -> OriginLookup {
        match raw_context {
            0 => OriginLookup::NoContext,
            REPEATING_KEY => OriginLookup::Known(RequestOrigin::Repeating),
            key => self
                .state()
                .entries
                .iter()
                .find(|(entry_key, _)| *entry_key == key)
                .map_or(OriginLookup::Unregistered, |(_, origin)| OriginLookup::Known(*origin)),
        }
    }

    /// Снимает запись разового запроса после его результата или провала. Ключи, которых нет
    /// (повторяющийся, пустой, уже снятый), молча не трогают таблицу — это не ошибка.
    pub fn settle(&self, raw_context: usize) {
        self.state().entries.retain(|(entry_key, _)| *entry_key != raw_context);
    }

    /// Снимает запись, если отправка запроса не удалась. Тот же смысл, что `settle`, отдельное имя —
    /// чтобы на месте вызова было видно, зачем.
    pub fn unregister(&self, key: RequestKey) {
        self.settle(key.as_raw());
    }

    /// Сколько разовых запросов сейчас ждут результата.
    pub fn pending(&self) -> usize {
        self.state().entries.len()
    }
}

#[cfg(test)]
#[path = "request_origin_tests.rs"]
mod tests;
