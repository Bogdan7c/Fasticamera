//! Мост «итог снимка со вспышкой от камеры → рендер» (сессия 14d).
//!
//! Движок камеры сообщает итог снимка со вспышкой наблюдателю ([`FlashStillObserver`]) на своём потоке. Если вспышка
//! НЕ снята, рендер ждёт кадр, которого не будет, поэтому приложение немедленно передаёт ему
//! `StillCommand::FlashFailed`: рендер даёт единственный итог `NotSecured(FlashNotCaptured(причина))`, не дожидаясь
//! срока `flash_frame_wait_ms`. Удачный итог (`Fired`) рендеру не нужен: кадр он находит в кольце сам по метке запроса.
//!
//! # Поздняя привязка
//! Движок камеры создаётся РАНЬШЕ службы снимков (камера нужна ей для каталога), а наблюдателя движок получает при
//! сборке. Поэтому мост создаётся вместе с ящиками связи камеры без цели ([`FlashStillRelay::new`]); службу снимков
//! подключает [`FlashStillRelay::bind_render`] при её запуске. Итог без цели не теряется молча: он считается
//! ([`FlashRelayCounters::unbound`]) и пишется в лог — но снимка без запущенной службы быть не может, так что
//! ненулевое значение значит ошибку сборки.
//!
//! Наблюдатель не блокирует (контракт `FlashStillObserver`): замки короткие, отправка рендеру — кладёт команду в ящик.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use camera_core::flash_status::{FlashStillObserver, FlashStillReport};
use render_loop::still::StillCommand;
use serde::Serialize;

use crate::flash_camera_port::still_of_flash_shot;
use crate::still_render_port::{StillCommandDelivery, StillRenderPort};

const LOG_TARGET: &str = "fasticamera::still::flash";

/// Сколько последних итогов камеры помнит мост, пока владелец не задал ёмкость. `[estimate]` 16: сценарию нужны
/// единицы итогов, а процесс живёт неделями; боевая сборка передаёт ёмкость журнала снимков из конфига.
const DEFAULT_REPORT_CAPACITY: usize = 16;

/// Учёт моста.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct FlashRelayCounters {
    /// Итоги `Fired` (кадр со вспышкой камерой снят).
    pub fired: u64,
    /// Итоги `Failed`, переданные рендеру.
    pub failed_routed: u64,
    /// Итоги `Failed`, пришедшие, когда служба снимков ещё не подключена (ошибка сборки, в норме 0).
    pub unbound: u64,
    /// Итоги `Failed`, которых рендер не принял (завершён или не поднимался): снимок дойдёт до срока службы.
    pub render_refused: u64,
}

#[derive(Default)]
struct CounterCells {
    fired: AtomicU64,
    failed_routed: AtomicU64,
    unbound: AtomicU64,
    render_refused: AtomicU64,
}

fn bump(cell: &AtomicU64) {
    cell.fetch_add(1, Ordering::Relaxed);
}

/// Наблюдатель итогов вспышки с поздно привязываемым рендером. Клоны `Arc` делят состояние.
pub struct FlashStillRelay {
    render: Mutex<Option<Arc<dyn StillRenderPort>>>,
    reports: Mutex<VecDeque<FlashStillReport>>,
    report_capacity: usize,
    counters: CounterCells,
}

impl FlashStillRelay {
    /// `report_capacity` — сколько последних итогов камеры помнить для сценариев (не меньше 1).
    pub fn new(report_capacity: usize) -> Self {
        Self {
            render: Mutex::new(None),
            reports: Mutex::new(VecDeque::new()),
            report_capacity: report_capacity.max(1),
            counters: CounterCells::default(),
        }
    }

    /// Подключает поток рендера: с этого момента срыв вспышки уходит ему командой. Повторный вызов заменяет цель
    /// (перезапуск службы снимков).
    pub fn bind_render(&self, render: Arc<dyn StillRenderPort>) {
        *lock(&self.render) = Some(render);
    }

    /// Последние итоги камеры по порядку (старые вытеснены).
    pub fn reports(&self) -> Vec<FlashStillReport> {
        lock(&self.reports).iter().copied().collect()
    }

    pub fn counters(&self) -> FlashRelayCounters {
        let load = |cell: &AtomicU64| cell.load(Ordering::Relaxed);
        FlashRelayCounters {
            fired: load(&self.counters.fired),
            failed_routed: load(&self.counters.failed_routed),
            unbound: load(&self.counters.unbound),
            render_refused: load(&self.counters.render_refused),
        }
    }

    fn remember(&self, report: &FlashStillReport) {
        let mut reports = lock(&self.reports);
        if reports.len() >= self.report_capacity {
            reports.pop_front();
        }
        reports.push_back(*report);
    }

    /// Передаёт срыв рендеру. Рендер игнорирует команду, пришедшую раньше своего `Begin`, поэтому порядок
    /// «`Begin`, затем команда камере» (его держит сервис снимка) обязателен.
    fn route_failure(&self, report: &FlashStillReport, failure: camera_contract::flash::FlashStillFailure) {
        let Some(render) = lock(&self.render).clone() else {
            bump(&self.counters.unbound);
            tracing::error!(target: LOG_TARGET, ?report, "итог вспышки пришёл, а служба снимков не подключена");
            return;
        };
        let id = still_of_flash_shot(report.shot());
        match render.send_still_command(StillCommand::FlashFailed { id, failure }) {
            StillCommandDelivery::Delivered => bump(&self.counters.failed_routed),
            refused => {
                bump(&self.counters.render_refused);
                tracing::warn!(target: LOG_TARGET, ?id, ?failure, ?refused, "рендер не принял срыв вспышки");
            }
        }
    }
}

impl Default for FlashStillRelay {
    fn default() -> Self {
        Self::new(DEFAULT_REPORT_CAPACITY)
    }
}

impl std::fmt::Debug for FlashStillRelay {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("FlashStillRelay")
    }
}

impl FlashStillObserver for FlashStillRelay {
    fn flash_still_finished(&self, report: &FlashStillReport) {
        self.remember(report);
        match report {
            FlashStillReport::Fired { shot, .. } => {
                bump(&self.counters.fired);
                tracing::info!(target: LOG_TARGET, ?shot, ?report, "камера сняла кадр со вспышкой");
            }
            FlashStillReport::Failed { shot, failure } => {
                tracing::warn!(target: LOG_TARGET, ?shot, ?failure, "камера не сняла кадр со вспышкой");
                self.route_failure(report, *failure);
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[path = "flash_still_relay_tests.rs"]
mod tests;
