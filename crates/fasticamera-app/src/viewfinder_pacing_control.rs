//! Переключение политики подачи кадров видоискателя на ходу (решение 53) для сценариев.
//!
//! Владелец политики — `RawFrameStream` (composition root держит один поток). Сценарий A/B
//! (`viewfinder-latency`) обязан менять политику на время окна и вернуть прежнюю при ЛЮБОМ конце
//! сценария — ровно как настройки рендера (`viewfinder_control`) и проба CPU. Поэтому:
//! * сценарий видит только трейт [`ViewfinderPacingControl`] (два намерения: «какая сейчас» и «поставь»),
//!   а не поток целиком — выдать или вернуть кадры через него нельзя;
//! * [`PacingClaim`] запоминает политику, действовавшую ДО первого вмешательства сценария, и отдаёт её
//!   ровно один раз при возврате.

use std::sync::{Mutex, MutexGuard, PoisonError};

use raw_frame_stream::{RawFrameStream, ViewfinderPacing, ViewfinderPacingSwitch};

/// Что умеет менять политику подачи (на телефоне — `RawFrameStream`, в тестах — запись).
pub trait ViewfinderPacingControl: Send + Sync {
    /// Действующая политика.
    fn pacing(&self) -> ViewfinderPacing;
    /// Переключает политику; `AlreadyActive` — ничего не изменилось.
    fn switch_to(&self, pacing: ViewfinderPacing) -> ViewfinderPacingSwitch;
}

impl ViewfinderPacingControl for RawFrameStream {
    fn pacing(&self) -> ViewfinderPacing {
        self.viewfinder_pacing()
    }

    fn switch_to(&self, pacing: ViewfinderPacing) -> ViewfinderPacingSwitch {
        self.set_viewfinder_pacing(pacing)
    }
}

/// Политика без живого потока (сборка без камеры, тесты): просто помнит выбранное значение,
/// повторяя семантику `Switched`/`AlreadyActive` настоящего потока.
#[derive(Debug)]
pub struct LocalPacing {
    current: Mutex<ViewfinderPacing>,
}

impl LocalPacing {
    pub fn new(initial: ViewfinderPacing) -> Self {
        Self {
            current: Mutex::new(initial),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ViewfinderPacing> {
        // Внутри простое Copy-значение: «отравленный» замок не портит его смысла.
        self.current.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl ViewfinderPacingControl for LocalPacing {
    fn pacing(&self) -> ViewfinderPacing {
        *self.lock()
    }

    fn switch_to(&self, pacing: ViewfinderPacing) -> ViewfinderPacingSwitch {
        let mut current = self.lock();
        if *current == pacing {
            return ViewfinderPacingSwitch::AlreadyActive;
        }
        *current = pacing;
        ViewfinderPacingSwitch::Switched
    }
}

/// Учёт вмешательства сценария: какая политика была ДО него. Владелец — `RawStreamServices`.
#[derive(Debug, Default)]
pub struct PacingClaim {
    original: Mutex<Option<ViewfinderPacing>>,
}

impl PacingClaim {
    fn lock(&self) -> MutexGuard<'_, Option<ViewfinderPacing>> {
        self.original.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Запоминает исходную политику, если сценарий вмешивается впервые (повторные вызовы её
    /// не перезаписывают: «исходная» — это то, что было до сценария, а не до очередного окна).
    pub fn remember_original(&self, original: ViewfinderPacing) {
        self.lock().get_or_insert(original);
    }

    /// Забирает исходную политику (`None` — сценарий политику не трогал). После этого учёт пуст.
    pub fn take_original(&self) -> Option<ViewfinderPacing> {
        self.lock().take()
    }
}

#[cfg(test)]
#[path = "viewfinder_pacing_control_tests.rs"]
mod tests;
