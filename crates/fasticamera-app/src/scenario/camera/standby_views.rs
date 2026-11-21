//! Срез наблюдения для исполнителя `standby` (13b): то, что он знает о движке. Сам `CameraStatus` тяжёлый, а тесту
//! незачем собирать его целиком, поэтому исполнитель получает только этот срез.

use std::sync::Arc;

use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::zoom::ZoomCatalog;
use camera_core::lens_status::{LensCounters, LensSwitchState};
use camera_core::standby_status::{StandbyCounters, StandbySlotState, StandbySlotStatus};
use camera_core::{CameraState, CameraStatus};

/// Объектив, который сейчас отдаёт кадры.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLensView {
    pub role: LensRole,
    pub camera: CameraId,
    pub facing: LensFacing,
}

/// Что исполнителю нужно знать о движке.
#[derive(Debug, Clone)]
pub struct StandbyEngineView {
    /// Поток идёт.
    pub streaming: bool,
    pub catalog: Arc<ZoomCatalog>,
    pub active: Option<ActiveLensView>,
    /// Роль, на которой смена объектива завершена (`Steady`); `None` — идёт смена или кадров ещё нет.
    pub steady_role: Option<LensRole>,
    /// Ячейки тёплых камер (пусто, пока тёплые не понадобились).
    pub slots: Vec<StandbySlotStatus>,
    pub standby_counters: StandbyCounters,
    pub lens_counters: LensCounters,
    /// События бэкенда прошлых поколений, отброшенные движком как устаревшие.
    pub stale_backend_events: u64,
    /// Результаты кадров, вытесненные из переполненной очереди бэкенда.
    pub dropped_capture_events: u64,
}

impl StandbyEngineView {
    pub fn of(status: &CameraStatus) -> Self {
        Self {
            streaming: matches!(status.state, CameraState::Streaming { .. }),
            catalog: Arc::clone(&status.lens.zoom_catalog),
            active: status.lens.active.as_ref().map(|active| ActiveLensView {
                role: active.role,
                camera: active.camera.clone(),
                facing: active.facing,
            }),
            steady_role: match status.lens.switch {
                LensSwitchState::Steady { role } => Some(role),
                LensSwitchState::Idle | LensSwitchState::Switching { .. } => None,
            },
            slots: status.lens.standby.slots.clone(),
            standby_counters: status.lens.standby.counters,
            lens_counters: status.lens.counters,
            stale_backend_events: status.counters.stale_backend_events,
            dropped_capture_events: status.counters.dropped_capture_events,
        }
    }

    /// Ячейка, в которой тёплая готова к повышению: камера этой ячейки.
    pub fn ready_standby_camera(&self) -> Option<&CameraId> {
        self.slots
            .iter()
            .find(|slot| slot.state == StandbySlotState::Ready)
            .and_then(|slot| slot.camera.as_ref())
    }

    /// Описание первой ячейки в `Failed` (причина — в JSON), если такая есть.
    pub fn failed_slot(&self) -> Option<String> {
        self.slots.iter().find_map(|slot| match &slot.state {
            StandbySlotState::Failed { reason } => Some(format!(
                "ячейка {} (камера {}): {}",
                slot.slot.index(),
                slot.camera.as_ref().map_or_else(|| "?".to_owned(), ToString::to_string),
                serde_json::to_string(reason).unwrap_or_else(|_| "причина не сериализована".to_owned())
            )),
            _ => None,
        })
    }

    /// Держит ли какая-нибудь ячейка ресурсы камеры: открыта, греется или готова (не свободна и не в отказе).
    pub fn any_slot_holds_camera(&self) -> bool {
        self.slots.iter().any(|slot| {
            matches!(
                slot.state,
                StandbySlotState::Opening
                    | StandbySlotState::Configured
                    | StandbySlotState::Warming
                    | StandbySlotState::Ready
            )
        })
    }
}
