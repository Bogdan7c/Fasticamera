//! Подставные отчёты и наблюдения для тестов сценария `standby`: «здоровый» отчёт, который проходит вердикт, и
//! точечные порчи одного значения.

use camera_android::standby_diagnostics::StandbySlotSnapshot;
use camera_core::lens_status::LensSwitchMethod;

use super::standby_report::{EngineTotals, StandbyObservationReport, StreamTotals};
use super::standby_runner::{FpsRecord, Leg, StandbyRecord, SwapRecord};

pub fn healthy_swap(leg: Leg, to_role: &'static str) -> SwapRecord {
    SwapRecord {
        leg,
        to_role,
        method: LensSwitchMethod::PromotedStandby,
        observed_ms: 120,
        engine_elapsed_ms: 100,
    }
}

pub fn window(name: &'static str, primary_fps: f64, standby_fps: f64, both: bool) -> FpsRecord {
    FpsRecord {
        name,
        window_ms: 2_000,
        primary_fps,
        standby_fps,
        both_cameras_running: both,
    }
}

/// Здоровый прогон: два обмена повышением, обе камеры по 59–60 к/с, тёплые закрыты, дом — `wide`.
pub fn healthy_report() -> StandbyObservationReport {
    StandbyObservationReport {
        run_failure: None,
        stage: "done",
        record: StandbyRecord {
            home_camera: Some("2".to_owned()),
            home_role: Some("wide"),
            neighbour_camera: Some("3".to_owned()),
            neighbour_role: Some("ultra_wide"),
            time_to_ready_ms: Some(260),
            swaps: vec![
                healthy_swap(Leg::ToNeighbour, "ultra_wide"),
                healthy_swap(Leg::BackHome, "wide"),
            ],
            windows: vec![
                window("alone", 60.0, 0.0, false),
                window("both_running", 59.8, 59.5, true),
                window("after_to_neighbour", 59.7, 59.6, true),
                window("after_back_home", 59.9, 59.4, true),
            ],
            release_ms: Some(1_700),
        },
        stream: StreamTotals {
            standby_streams_begun: 1,
            standby_frames_returned: 400,
            standby_promotions: 2,
            frames_released_by_promotion: 6,
            ..StreamTotals::default()
        },
        engine: EngineTotals {
            standby_opened: 1,
            promotions: 2,
            switches_completed: 2,
            ..EngineTotals::default()
        },
        backend: vec![StandbySlotSnapshot {
            slot: 0,
            sessions_started: 1,
            results_offered: 300,
            capture_failures: 0,
            open_failures: 0,
            configure_failures: 0,
            swaps: 2,
            swap_refusals: 0,
        }],
        final_role: Some("wide"),
    }
}
