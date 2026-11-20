//! Подставные записи и отчёты для тестов сценария `zoom`: «здоровый» отчёт, который проходит вердикт, и точечные
//! порчи одного значения (тест правила портит РОВНО одно поле и ждёт ровно это нарушение).

use camera_contract::ids::{LensFacing, LensRole};

use super::zoom_record::{HapticWindow, MirrorCheck, SwitchTiming, ZoomStepRecord};
use super::zoom_report::{FinalState, ZoomObservationReport, ZoomTotals};

/// Здоровый шаг со сменой объектива: события за 400 мс, первый живой кадр за 520 мс, 60 к/с, зеркало верное.
pub fn healthy_switch_step(name: &str) -> ZoomStepRecord {
    ZoomStepRecord {
        name: name.to_owned(),
        expected: "to_ultra_wide".to_owned(),
        switch: Some(SwitchTiming {
            from: "wide",
            to: "ultra_wide",
            engine_elapsed_ms: 380,
            observed_ms: 400,
        }),
        first_live_frame_ms: Some(520.0),
        new_camera: Some("3".to_owned()),
        held_frames: 20,
        black_frames_while_picture_expected: 0,
        retargets: 0,
        lens_intents_published: 3,
        measured_fps: Some(59.5),
        expected_fps: Some(60),
        mirror: MirrorCheck::Correct,
        haptics: None,
        screen_ratio: 0.59,
        ring_overran: false,
        failure: None,
        notes: Vec::new(),
    }
}

/// Здоровый шаг без смены объектива (2× — кроп основной камеры).
pub fn healthy_crop_step(name: &str) -> ZoomStepRecord {
    ZoomStepRecord {
        expected: "no_switch".to_owned(),
        switch: None,
        first_live_frame_ms: None,
        new_camera: None,
        held_frames: 0,
        mirror: MirrorCheck::NotApplicable,
        screen_ratio: 2.0,
        ..healthy_switch_step(name)
    }
}

/// Здоровая протяжка по колесу: зум ушёл к 5×, вибраций 15, наименьший промежуток 32 мс.
pub fn healthy_drag_step(name: &str) -> ZoomStepRecord {
    ZoomStepRecord {
        expected: "no_switch".to_owned(),
        switch: None,
        first_live_frame_ms: None,
        new_camera: None,
        held_frames: 0,
        mirror: MirrorCheck::NotApplicable,
        screen_ratio: 5.4,
        haptics: Some(HapticWindow {
            count: 15,
            min_gap_ms: Some(32.0),
            unmeasured: false,
        }),
        ..healthy_switch_step(name)
    }
}

/// Здоровый отчёт: пять шагов, экран оставлен на 1× основной камеры.
pub fn healthy_report() -> ZoomObservationReport {
    ZoomObservationReport {
        planned_steps: 3,
        steps: vec![
            healthy_switch_step("01_tap_0.6"),
            healthy_crop_step("02_tap_2"),
            healthy_drag_step("03_wheel_drag"),
        ],
        unfinished: Vec::new(),
        run_failure: None,
        totals: ZoomTotals {
            held_frames: 20,
            switches_completed: 1,
            lens_intents_published: 12,
            ..ZoomTotals::default()
        },
        final_state: Some(FinalState {
            ratio_milli: 1_000,
            facing: LensFacing::Back,
            active_role: Some(LensRole::Wide),
        }),
    }
}
