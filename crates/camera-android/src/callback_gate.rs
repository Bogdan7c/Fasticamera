//! «Ворота» колбэка: гарантия, что после закрытия ни один колбэк не находится внутри вызовов NDK.
//!
//! Зачем (взаимоблокировка `AImageReader_delete`, подтверждено по AOSP `NdkImageReader.cpp`):
//! `AImageReader::close()` берёт внутренний замок `mLock` и **под ним** останавливает поток
//! колбэков (`mCbLooper->stop()` → `requestExitAndWait`, строки ~380–397). Наш слушатель на этом
//! потоке зовёт `AImageReader_acquireNextImage`, который тоже берёт `mLock`. Если `delete` вызван,
//! пока колбэк уже прошёл проверку «закрыто» и вошёл в NDK, поток камеры ждёт колбэк, а колбэк
//! ждёт замок, который держит поток камеры, — вечная взаимоблокировка.
//!
//! Решение: колбэк держит ворота на всё время работы с NDK ([`CallbackGate::admit`]), а закрытие
//! ([`CallbackGate::close`]) берёт те же ворота, ставит флаг и отпускает. Значит:
//! * `close` ждёт, пока колбэк, уже вошедший внутрь, выйдет (он не ждёт ничего, что держит
//!   закрывающий: `mLock` на этот момент ещё свободен, `AImageReader_delete` ещё не вызван);
//! * после возврата `close` ни один колбэк не внутри, а новые сразу получают
//!   [`GateAdmission::Closed`] и в NDK не заходят;
//! * только после этого можно звать `AImageReader_delete`.
//!
//! Модуль нейтральный (без NDK): гонка проверяется на ПК потоками.

use std::sync::{Mutex, MutexGuard, PoisonError};

/// Ворота: флаг «закрыто» под замком, который колбэк держит на время своей работы.
#[derive(Debug, Default)]
pub struct CallbackGate {
    closed: Mutex<bool>,
}

/// Результат попытки колбэка войти.
#[derive(Debug)]
pub enum GateAdmission<'gate> {
    /// Вошёл: ворота удерживаются, пока жив пропуск.
    Admitted(GatePass<'gate>),
    /// Уже закрыто: в NDK заходить нельзя.
    Closed,
}

/// Пропуск внутрь ворот. Пока он жив, [`CallbackGate::close`] ждёт.
#[derive(Debug)]
pub struct GatePass<'gate> {
    // Поле нужно только ради удержания замка.
    _guard: MutexGuard<'gate, bool>,
}

impl CallbackGate {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, bool> {
        // Паника колбэка (она ловится выше) отравляет замок; флаг при этом цел.
        self.closed.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Колбэк просится внутрь. Блокируется только на время параллельного `close` (микросекунды).
    pub fn admit(&self) -> GateAdmission<'_> {
        let guard = self.lock();
        if *guard {
            GateAdmission::Closed
        } else {
            GateAdmission::Admitted(GatePass { _guard: guard })
        }
    }

    /// Закрыть ворота. Возвращается, когда ни один колбэк не внутри; повторный вызов безвреден.
    pub fn close(&self) {
        *self.lock() = true;
    }
}

#[cfg(test)]
#[path = "callback_gate_tests.rs"]
mod tests;
