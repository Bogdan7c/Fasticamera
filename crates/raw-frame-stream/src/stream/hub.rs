//! Хаб потока: один замок на всё состояние и общие счётчики.
//!
//! # Порядок замков
//! `StreamHub::state` (хаб) → `SharedFrame::state` (внутри `RawFrameLease::share`).
//! Обратного порядка нет: лизинг при возврате хаб не трогает. Замок камеры (`AImageReader`)
//! под замком хаба не берётся никогда: возвраты производителю копятся в [`Deferred`]
//! и выполняются после отпускания замка хаба.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use super::deferred::Deferred;
use super::state::HubState;
use crate::config::RawFrameStreamConfig;
use crate::counters::StreamAccounting;

pub(super) struct StreamHub {
    state: Mutex<HubState>,
    pub(super) accounting: Arc<StreamAccounting>,
}

impl StreamHub {
    pub(super) fn new(config: RawFrameStreamConfig) -> Self {
        let accounting = Arc::new(StreamAccounting::default());
        Self {
            state: Mutex::new(HubState::new(config, Arc::clone(&accounting))),
            accounting,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HubState> {
        // Под замком нет многошаговых «полусостояний», опасных после паники в чужом потоке:
        // продолжаем с тем, что есть, а не роняем камеру.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Выполняет `operation` под замком, а накопленные возвраты — уже без него.
    pub(super) fn with_state<R>(&self, operation: impl FnOnce(&mut HubState, &mut Deferred) -> R) -> R {
        let mut deferred = Deferred::default();
        let outcome = {
            let mut state = self.lock();
            operation(&mut state, &mut deferred)
        };
        deferred.finish();
        outcome
    }
}
