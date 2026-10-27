//! Часы кадров: на что поток рендера опирается, выбирая момент следующего кадра.
//!
//! Поток зависит от этого небольшого трейта, а не от `shell-contract::vsync`: тесты
//! подставляют fake-часы, а оболочка — настоящую подписку на vsync.
//!
//! Настоящий источник — [`super::vsync_clock::VsyncFrameClock`] (адаптер подписки на vsync
//! оболочки), он переводит `VsyncWait::{Tick, TimedOut, Stopped}` в [`FrameWait`] один к одному.

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;
use shell_contract::vsync::FrameTimelines;

/// Один тик «пора рисовать кадр».
///
/// Плоские `expected_present`/`deadline` — предпочитаемая платформой линия кадра; все линии —
/// в `timelines` (по ним поток выбирает линию, см. `present_target`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameTick {
    /// Порядковый номер тика источника (растёт; пропуски видны по `skipped_ticks`).
    pub sequence: u64,
    /// Когда кадр, нарисованный по этому тику, ожидается на экране.
    pub expected_present: MonotonicNanos,
    /// Срок, к которому кадр надо сдать композитору, чтобы он попал в `expected_present`.
    pub deadline: MonotonicNanos,
    /// Сколько тиков пропущено с прошлого полученного (поток не успел или спал).
    pub skipped_ticks: u64,
    /// Все линии кадра этого vsync (предпочитаемая среди них).
    pub timelines: FrameTimelines,
}

/// Итог ожидания следующего кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::large_enum_variant,
    reason = "тик с линиями кадра (~250 байт) возвращается по значению раз на vsync; Box выделял бы память на каждом vsync"
)]
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
