//! Часы кадров: на что поток рендера опирается, выбирая момент следующего кадра.
//!
//! Поток зависит от этого небольшого трейта, а не от `shell-contract::vsync`: тесты
//! подставляют fake-часы, а оболочка — настоящую подписку на vsync.
//!
//! Настоящий источник — [`super::vsync_clock::VsyncFrameClock`] (адаптер подписки на vsync
//! оболочки), он переводит `VsyncWait::{Tick, TimedOut, Stopped}` в [`FrameWait`] один к одному.

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;

/// Один тик «пора рисовать кадр».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTick {
    /// Порядковый номер тика источника (растёт; пропуски видны по `skipped_ticks`).
    pub sequence: u64,
    /// Когда кадр, нарисованный по этому тику, ожидается на экране.
    pub expected_present: MonotonicNanos,
    /// Срок, к которому кадр надо сдать композитору, чтобы он попал в `expected_present`.
    /// Сейчас поток его только несёт дальше (бюджет кадра будет читать будущий планировщик).
    pub deadline: MonotonicNanos,
    /// Сколько тиков пропущено с прошлого полученного (поток не успел или спал).
    pub skipped_ticks: u64,
}

/// Итог ожидания следующего кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameWait {
    /// Пришёл тик.
    Tick(FrameTick),
    /// За отведённое время тика нет: поток проверяет почтовый ящик и ждёт снова.
    TimedOut,
    /// Источник тиков остановлен навсегда: показывать кадры больше нечем.
    Stopped,
}

/// Источник ритма кадров. Вызывается только из потока рендера.
pub trait FrameClock: Send + 'static {
    /// Ждёт следующий тик не дольше `timeout`.
    fn wait_next_frame(&mut self, timeout: Duration) -> FrameWait;
}
