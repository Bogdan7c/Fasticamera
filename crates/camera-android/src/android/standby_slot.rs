//! Ячейки тёплых камер бэкенда (13b): у каждой свои устройство, сессия и припаркованные читатели.
//!
//! Ячейка устроена **так же, как основная линия бэкенда** (устройство → сессия → читатели), поэтому
//! обмен ролей — это обмен тремя полями целиком (`std::mem::swap`), а закрытие ячейки идёт в том
//! же порядке, что и закрытие основной камеры.

use camera_core::backend_standby::StandbySlotId;

use super::device::OpenedDevice;
use super::outputs::ParkedOutputs;
use super::session::ActiveSession;

/// Ресурсы одной ячейки. Порядок полей = порядок запасного разрушения (как у основной линии):
/// сессия, затем читатели, затем устройство.
#[derive(Default)]
pub(super) struct StandbyLine {
    pub session: Option<ActiveSession>,
    /// Читатели закрытой сессии: живут, пока устройство не отпустило их окна.
    pub parked_outputs: Option<ParkedOutputs>,
    pub device: Option<OpenedDevice>,
}

impl StandbyLine {
    /// Закрывает ячейку: сессия (с остановкой приёма кадров) → устройство → читатели.
    /// Идемпотентно. `reap` читателей — забота вызывающего (бэкенд делает его на каждом вызове).
    pub(super) fn close(&mut self) {
        if let Some(session) = self.session.take() {
            self.parked_outputs = Some(session.into_parked_outputs());
        }
        self.device = None;
        self.parked_outputs = None;
    }

    /// Занята ли ячейка (есть устройство или сессия).
    pub(super) fn is_open(&self) -> bool {
        self.device.is_some() || self.session.is_some()
    }
}

/// Все ячейки; их число задаёт конфиг (`standby_capacity`).
pub(super) struct StandbySlots {
    lines: Vec<StandbyLine>,
}

impl StandbySlots {
    pub(super) fn new(capacity: usize) -> Self {
        Self {
            lines: (0..capacity).map(|_| StandbyLine::default()).collect(),
        }
    }

    pub(super) fn capacity(&self) -> usize {
        self.lines.len()
    }

    /// Ячейка по номеру; `None` — номер за пределами ёмкости.
    pub(super) fn line_mut(&mut self, slot: StandbySlotId) -> Option<&mut StandbyLine> {
        self.lines.get_mut(slot.index())
    }

    /// Закрывает все ячейки (остановка бэкенда).
    pub(super) fn close_all(&mut self) {
        for line in &mut self.lines {
            line.close();
        }
    }
}
