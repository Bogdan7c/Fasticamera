//! Повтор строки результата наблюдающего smoke-сценария (решение владельца 58, сессия 07a).
//!
//! Зачем: на телефоне Android молча выбрасывает записи logcat нашего процесса во всплеске записи
//! (замер 07a: ~0,3 с без единой нашей строки, пока остальные процессы пишут). `__android_log_write`
//! при этом сообщает об успехе, так что приложение потерю не видит. Если в такую дыру попадала
//! единственная строка `FASTICAMERA_RESULT`, скрипт ждал её до таймаута («нет результата»), хотя
//! сценарий честно закончился.
//!
//! Решение: после итога сценария маршрутизатор ещё несколько раз повторяет ту же строку,
//! разнесённо во времени — пачка подряд утонула бы в той же дыре. Скрипт берёт первую дошедшую
//! копию (повторы для него идемпотентны). Повтор отменяется, как только пришёл новый сценарий:
//! чужой результат не должен попасть в ожидание следующего запуска.
//!
//! Повторяется только строка результата. Строки стадий (`FASTICAMERA_STAGE`, в том числе
//! `detail_*`) не повторяются: на стадию драйвер отвечает действием на телефоне (скриншот, касание,
//! HOME), и повтор выполнил бы его дважды — это известное ограничение.
//!
//! Владение: расписание ([`ResultEcho`]) — у маршрутизатора; байты строки — у вывода сценариев
//! ([`crate::scenario::observing::ScenarioOutput::repeat_last_result`]), чтобы повтор был
//! байт-в-байт той строкой, что уже ушла.

use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Настройки повтора (секция `scenarios.result_echo`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ResultEchoConfig {
    /// Сколько раз повторить строку после первой. 0 — без повторов (поведение до 07a).
    /// 5: при интервале 1 с копии покрывают 5 с после итога — на порядок больше наблюдавшейся
    /// дыры (~0,3 с), но заметно меньше времени до следующего запуска скрипта.
    pub repeats: u32,
    /// Пауза между копиями, мс. 1000: дальше длины наблюдавшейся дыры, так что две копии в одну
    /// дыру не попадают; тик маршрутизатора (50 мс) даёт эту точность с запасом.
    pub interval_ms: u64,
}

impl Default for ResultEchoConfig {
    fn default() -> Self {
        Self {
            repeats: 5,
            interval_ms: 1000,
        }
    }
}

/// Неверные настройки повтора.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ResultEchoConfigError {
    /// При нулевом интервале все повторы ушли бы одной пачкой и утонули бы в той же дыре.
    #[error("result_echo.interval_ms должно быть больше нуля, когда repeats > 0")]
    ZeroInterval,
}

impl ResultEchoConfig {
    pub fn validate(&self) -> Result<(), ResultEchoConfigError> {
        if self.repeats > 0 && self.interval_ms == 0 {
            return Err(ResultEchoConfigError::ZeroInterval);
        }
        Ok(())
    }

    fn interval(&self) -> Duration {
        Duration::from_millis(self.interval_ms)
    }
}

/// Что делать на этом тике.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EchoDue {
    /// Пора выдать очередную копию строки результата.
    Repeat,
    /// Повтор запланирован, но срок ещё не наступил.
    NotYet,
    /// Повторять нечего: итога не было, повторы исчерпаны или отменены.
    Idle,
}

/// Запланированные повторы последнего итога.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PendingEcho {
    remaining: u32,
    next_due: Instant,
}

/// Расписание повторов строки результата. Владелец — маршрутизатор событий.
#[derive(Debug)]
pub struct ResultEcho {
    config: ResultEchoConfig,
    pending: Option<PendingEcho>,
}

impl ResultEcho {
    pub fn new(config: ResultEchoConfig) -> Self {
        Self { config, pending: None }
    }

    /// Итог сценария только что выдан: запланировать его повторы (прежнее расписание заменяется).
    pub fn arm(&mut self, emitted_at: Instant) {
        self.pending = (self.config.repeats > 0).then(|| PendingEcho {
            remaining: self.config.repeats,
            next_due: emitted_at + self.config.interval(),
        });
    }

    /// Пришёл новый сценарий: повторы прежнего итога больше не нужны и опасны.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// Решение на тик: пора ли выдать копию. `Repeat` расходует один повтор.
    ///
    /// Следующий срок считается от фактического момента повтора, а не от прежнего срока:
    /// после долгого тика копии не выдаются пачкой «наверстать», они остаются разнесёнными.
    pub fn take_due(&mut self, now: Instant) -> EchoDue {
        let Some(pending) = self.pending.as_mut() else {
            return EchoDue::Idle;
        };
        if now < pending.next_due {
            return EchoDue::NotYet;
        }
        pending.remaining -= 1;
        pending.next_due = now + self.config.interval();
        if pending.remaining == 0 {
            self.pending = None;
        }
        EchoDue::Repeat
    }
}

#[cfg(test)]
#[path = "result_echo_tests.rs"]
mod tests;
