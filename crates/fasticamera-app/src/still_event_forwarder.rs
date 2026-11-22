//! Приёмник событий проявления, который рендер держит у себя: пересылает их службе записи без ожидания.
//!
//! Рендер зовёт [`StillEventSink::deliver`] со своего потока и не должен ждать (инвариант 4 README). Служба
//! принимает события в ограниченную очередь; когда она полна, повторить без ожидания нельзя — событие
//! уничтожается (рендер уже отдал владение, его поверхность и картинка отпущены), а переполнение считается и
//! здесь, и в `StillServiceStats::rejected_events`.
//!
//! Итог снимка при потерянном событии даёт СРОК службы: снимок без захвата срывается по `capture_timeout_ms`,
//! без итога проявления — по `develop_timeout_ms` (`Failed` ровно один раз, поздние события считаются
//! `late_events` и игнорируются). Дважды экрану итог не доставится: таблица снимает запись на первом итоге.

use std::sync::atomic::{AtomicU64, Ordering};

use photo_output::service::{DevelopSubmitError, StillServiceHandle};
use render_loop::still::StillEventSink;
use serde::Serialize;
use still_contract::develop::DevelopEvent;

const LOG_TARGET: &str = "fasticamera::still::events";

/// Счётчики пересылки событий.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct ForwarderCounters {
    /// События, принятые очередью службы.
    pub forwarded: u64,
    /// События, отвергнутые полной очередью службы (уничтожены; итог снимка даст срок службы).
    pub rejected_queue_full: u64,
    /// События, отвергнутые остановленной службой.
    pub rejected_service_stopped: u64,
}

/// Пересылка событий рендера в службу записи.
pub struct DevelopEventForwarder {
    handle: StillServiceHandle,
    forwarded: AtomicU64,
    rejected_queue_full: AtomicU64,
    rejected_service_stopped: AtomicU64,
}

impl DevelopEventForwarder {
    pub fn new(handle: StillServiceHandle) -> Self {
        Self {
            handle,
            forwarded: AtomicU64::new(0),
            rejected_queue_full: AtomicU64::new(0),
            rejected_service_stopped: AtomicU64::new(0),
        }
    }

    /// Пересылает событие; итог — для вызывающего, которому нужно различать причины (сам приёмник рендера
    /// ответа не возвращает).
    pub fn forward(&self, event: DevelopEvent) -> Result<(), DevelopSubmitError> {
        let id = event.id();
        match self.handle.submit_develop_event(event) {
            Ok(()) => {
                self.forwarded.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(DevelopSubmitError::QueueFull) => {
                self.rejected_queue_full.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(target: LOG_TARGET, ?id, "очередь событий службы снимков полна: событие потеряно, итог даст срок службы");
                Err(DevelopSubmitError::QueueFull)
            }
            Err(DevelopSubmitError::ServiceStopped) => {
                self.rejected_service_stopped.fetch_add(1, Ordering::Relaxed);
                tracing::warn!(target: LOG_TARGET, ?id, "служба снимков остановлена: событие рендера потеряно");
                Err(DevelopSubmitError::ServiceStopped)
            }
        }
    }

    pub fn counters(&self) -> ForwarderCounters {
        ForwarderCounters {
            forwarded: self.forwarded.load(Ordering::Relaxed),
            rejected_queue_full: self.rejected_queue_full.load(Ordering::Relaxed),
            rejected_service_stopped: self.rejected_service_stopped.load(Ordering::Relaxed),
        }
    }
}

impl StillEventSink for DevelopEventForwarder {
    fn deliver(&self, event: DevelopEvent) {
        // Причина отказа уже записана в счётчик и лог внутри `forward`; рендеру ответ не нужен.
        let _delivery = self.forward(event);
    }
}

#[cfg(test)]
#[path = "still_event_forwarder_tests.rs"]
mod tests;
