//! «Тёплые» камеры в движке (13b): ячейки, спрос экрана, пауза до закрытия, повышение до основной.
//!
//! # Кто чем владеет
//! * [`StandbyEngine`] (этот файл) — состояние: ячейки, спрос, счётчики. Чистая логика без бэкенда;
//! * `sync` — доведение ячеек до спроса (открыть, сконфигурировать, закрыть) вызовами бэкенда;
//! * `request` — повторяющийся запрос тёплой (экспозиция основной, пересчитанная на тёплую);
//! * `promote` — повышение: второй способ исполнить смену объектива (обмен ролей вместо close/open);
//! * `events` — маршрутизация событий бэкенда по поколениям тёплых.
//!
//! # Инварианты
//! 1. Отказ тёплой (open/configure/request, `DeviceError`, `Disconnected`, закрытие сессии)
//!    переводит **только её ячейку** в `Failed`; основную камеру и её состояние он не трогает.
//! 2. `Failed` не повторяется сам: ячейка очищается, когда спрос закончился и тёплые закрыты
//!    (то есть «до нового спроса»).
//! 3. Поколения устройств и сессий тёплых берутся из тех же счётчиков, что у основной
//!    (`devices_opened`, `sessions_configured`), поэтому уникальны.
//! 4. Результаты кадров тёплой не попадают ни в темп, ни в 3A, ни в учёт смены объектива.
//! 5. Тёплые поднимаются после основной и никогда не задерживают её доведение.

mod events;
mod promote;
mod request;
mod sync;

use std::time::{Duration, Instant};

use camera_contract::ids::CameraId;
use camera_contract::zoom::StandbyDemand;

use super::{LiveSession, OpenDevice};
use crate::backend_standby::StandbySlotId;
use crate::standby_status::{
    StandbyCounters, StandbyFailureReason, StandbySlotState, StandbySlotStatus, StandbyStatus,
};
use request::IssuedRequest;

/// Что экран просит сейчас и когда тёплые можно закрывать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Demand {
    /// Тёплые не нужны (и ничего не ждёт закрытия).
    Off,
    /// Экран просит держать соседей тёплыми.
    Wanted,
    /// Экран перестал просить: тёплые остаются до этого момента (паузу задаёт конфиг).
    ReleaseAt(Instant),
}

/// Где ячейка на пути «закрыта → стримит».
#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Idle,
    /// Устройство открыто, сессии нет.
    Opening,
    /// Сессия сконфигурирована, запроса нет.
    Configured,
    /// Запрос стоит, первого результата ещё не было.
    Warming,
    /// Первый результат пришёл (или это бывшая основная): можно повышать.
    Ready,
    Failed(StandbyFailureReason),
}

/// Одна ячейка тёплой камеры.
struct Slot {
    id: StandbySlotId,
    /// Камера ячейки (для `Failed` остаётся, чтобы снимок говорил, что отказало).
    camera: Option<CameraId>,
    device: Option<OpenDevice>,
    session: Option<LiveSession>,
    phase: Phase,
    /// Запрос, который сейчас стоит у тёплой (для решения «пересчитывать ли»).
    issued: Option<IssuedRequest>,
}

impl Slot {
    fn idle(id: StandbySlotId) -> Self {
        Self {
            id,
            camera: None,
            device: None,
            session: None,
            phase: Phase::Idle,
            issued: None,
        }
    }

    /// Держит ли ячейка открытые ресурсы бэкенда.
    fn holds_resources(&self) -> bool {
        self.device.is_some() || self.session.is_some()
    }

    /// Отпустить ресурсы в состоянии движка (бэкенду `close_standby` говорит вызывающий) и
    /// стать свободной.
    fn become_idle(&mut self) {
        *self = Self::idle(self.id);
    }
}

/// Все тёплые камеры движка.
pub(super) struct StandbyEngine {
    slots: Vec<Slot>,
    demand: Demand,
    counters: StandbyCounters,
}

impl StandbyEngine {
    pub(super) fn new() -> Self {
        Self {
            slots: Vec::new(),
            demand: Demand::Off,
            counters: StandbyCounters::default(),
        }
    }

    // ---- Спрос ----

    /// Экран сообщил спрос. `true`, если что-то изменилось (нужно довести ячейки).
    ///
    /// `NotNeeded` при действующем спросе не закрывает камеры сразу, а ставит срок через `delay`;
    /// повторное `NotNeeded` срок не продлевает (иначе поток намерений отодвигал бы его вечно).
    pub(super) fn set_demand(&mut self, demand: StandbyDemand, now: Instant, delay: Duration) -> bool {
        let next = match (demand, self.demand) {
            (StandbyDemand::KeepNeighbourWarm, _) => Demand::Wanted,
            (StandbyDemand::NotNeeded, Demand::Wanted) => Demand::ReleaseAt(now + delay),
            (StandbyDemand::NotNeeded, unchanged) => unchanged,
        };
        let changed = next != self.demand;
        self.demand = next;
        changed
    }

    /// Закончилась ли пауза до закрытия.
    pub(super) fn release_is_due(&self, now: Instant) -> bool {
        matches!(self.demand, Demand::ReleaseAt(deadline) if now >= deadline)
    }

    /// Когда нужно проснуться, чтобы закрыть тёплые.
    pub(super) fn next_deadline(&self) -> Option<Instant> {
        match self.demand {
            Demand::ReleaseAt(deadline) => Some(deadline),
            Demand::Off | Demand::Wanted => None,
        }
    }

    fn wants_neighbours(&self) -> bool {
        self.demand == Demand::Wanted
    }

    fn has_demand(&self) -> bool {
        self.demand != Demand::Off
    }

    /// Спрос закончен: отказы забываются — следующий спрос начнёт с чистого листа.
    /// Ресурсы к этому моменту уже закрыты вызывающим.
    pub(super) fn end_demand(&mut self) {
        self.demand = Demand::Off;
        for slot in &mut self.slots {
            if matches!(slot.phase, Phase::Failed(_)) {
                slot.become_idle();
            }
        }
    }

    // ---- Ячейки ----

    /// Ячеек не меньше `capacity` (лишние не убираются: ёмкость за жизнь процесса не меняется).
    fn ensure_slots(&mut self, capacity: usize) {
        while self.slots.len() < capacity {
            let index = u8::try_from(self.slots.len()).unwrap_or(u8::MAX);
            self.slots.push(Slot::idle(StandbySlotId::new(index)));
        }
    }

    fn slot_mut(&mut self, id: StandbySlotId) -> Option<&mut Slot> {
        self.slots.get_mut(id.index())
    }

    /// Ячейка, которая занята этой камерой (в любом состоянии, включая `Failed`).
    fn slot_holding(&self, camera: &CameraId) -> Option<&Slot> {
        self.slots.iter().find(|slot| slot.camera.as_ref() == Some(camera))
    }

    fn free_slot(&self) -> Option<StandbySlotId> {
        self.slots
            .iter()
            .find(|slot| slot.phase == Phase::Idle)
            .map(|slot| slot.id)
    }

    /// Готовая к повышению ячейка этой камеры.
    pub(super) fn ready_slot_for(&self, camera: &CameraId) -> Option<StandbySlotId> {
        self.slots
            .iter()
            .find(|slot| slot.phase == Phase::Ready && slot.camera.as_ref() == Some(camera))
            .map(|slot| slot.id)
    }

    /// Снимок для статуса.
    pub(super) fn status(&self) -> StandbyStatus {
        StandbyStatus {
            slots: self
                .slots
                .iter()
                .map(|slot| StandbySlotStatus {
                    slot: slot.id,
                    camera: slot.camera.clone(),
                    state: match &slot.phase {
                        Phase::Idle => StandbySlotState::Idle,
                        Phase::Opening => StandbySlotState::Opening,
                        Phase::Configured => StandbySlotState::Configured,
                        Phase::Warming => StandbySlotState::Warming,
                        Phase::Ready => StandbySlotState::Ready,
                        Phase::Failed(reason) => StandbySlotState::Failed { reason: reason.clone() },
                    },
                })
                .collect(),
            counters: self.counters,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DELAY: Duration = Duration::from_millis(1_500);

    #[test]
    fn demand_off_to_wanted_to_release_with_a_fixed_deadline() {
        let start = Instant::now();
        let mut standby = StandbyEngine::new();
        assert!(
            !standby.set_demand(StandbyDemand::NotNeeded, start, DELAY),
            "спроса не было"
        );
        assert!(standby.set_demand(StandbyDemand::KeepNeighbourWarm, start, DELAY));
        assert!(
            !standby.set_demand(StandbyDemand::KeepNeighbourWarm, start, DELAY),
            "тот же спрос"
        );
        assert!(standby.set_demand(StandbyDemand::NotNeeded, start, DELAY));
        assert_eq!(standby.next_deadline(), Some(start + DELAY));
        // Повторное «не нужно» срок не двигает.
        let later = start + Duration::from_millis(400);
        assert!(!standby.set_demand(StandbyDemand::NotNeeded, later, DELAY));
        assert_eq!(standby.next_deadline(), Some(start + DELAY));
        assert!(!standby.release_is_due(start + Duration::from_millis(1_499)));
        assert!(standby.release_is_due(start + DELAY));
    }

    #[test]
    fn new_demand_during_the_pause_cancels_the_release() {
        let start = Instant::now();
        let mut standby = StandbyEngine::new();
        standby.set_demand(StandbyDemand::KeepNeighbourWarm, start, DELAY);
        standby.set_demand(StandbyDemand::NotNeeded, start, DELAY);
        assert!(standby.set_demand(StandbyDemand::KeepNeighbourWarm, start, DELAY));
        assert_eq!(standby.next_deadline(), None);
        assert!(standby.wants_neighbours());
    }

    #[test]
    fn end_demand_clears_failures_but_keeps_counters() {
        let start = Instant::now();
        let mut standby = StandbyEngine::new();
        standby.ensure_slots(1);
        standby.counters.failures = 3;
        standby.slots[0].camera = Some(camera_contract::fixtures::camera_id("3"));
        standby.slots[0].phase = Phase::Failed(StandbyFailureReason::DeviceDisconnected);
        standby.set_demand(StandbyDemand::KeepNeighbourWarm, start, DELAY);
        standby.end_demand();
        assert!(!standby.has_demand());
        assert_eq!(standby.status().slots[0].state, StandbySlotState::Idle);
        assert_eq!(standby.status().slots[0].camera, None);
        assert_eq!(standby.counters.failures, 3);
    }
}
