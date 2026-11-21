//! Подставные записи и отчёты для тестов сценария `zoom`: «здоровый» отчёт, который проходит вердикт, и точечные
//! порчи одного значения (тест правила портит РОВНО одно поле и ждёт ровно это нарушение).

use std::sync::Arc;

use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::ZoomCatalog;
use camera_core::lens_status::LensSwitchMethod;

use super::standby_views::StandbyEngineView;
use super::zoom_presented::ContinuityDelta;
use super::zoom_record::{HapticWindow, MirrorCheck, SwitchTiming, ZoomStepRecord};
use super::zoom_report::{FinalState, ZoomObservationReport, ZoomTotals};
use super::zoom_reveal::RevealFaults;
use super::zoom_seamless_record::{FpsPair, SeamlessRecord, SwitchGap};

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
            method: LensSwitchMethod::Reopened,
        }),
        first_live_frame_ms: Some(520.0),
        new_camera: Some("3".to_owned()),
        held_frames: 20,
        black_frames_while_picture_expected: 0,
        reveal_faults: RevealFaults::default(),
        continuity: ContinuityDelta::default(),
        presented_frames: Vec::new(),
        retargets: 0,
        lens_intents_published: 3,
        measured_fps: Some(59.5),
        expected_fps: Some(60),
        mirror: MirrorCheck::Correct,
        haptics: None,
        screen_ratio: 0.59,
        ring_overran: false,
        seamless: None,
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

/// Здоровая протяжка по колесу: зум ушёл к 5×, вибраций 15 за окно в 1,2 с (ограничитель 30 мс пропустил бы до 41).
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
            window_ms: 1_200.0,
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

/// Движок без тёплых камер (ячеек нет): так выглядит всё, кроме окон колеса и щипка.
pub fn quiet_standby(catalog: Arc<ZoomCatalog>) -> StandbyEngineView {
    StandbyEngineView {
        streaming: true,
        catalog,
        active: None,
        steady_role: None,
        slots: Vec::new(),
        standby_counters: Default::default(),
        lens_counters: Default::default(),
        stale_backend_events: 0,
        dropped_capture_events: 0,
    }
}

/// Здоровая бесшовная смена: повышение тёплой, 0 удержанных/чёрных/скрытых, разрыв в 1 период, обе камеры по 59 к/с.
pub fn healthy_seamless_switch_step(name: &str, leg: &'static str) -> ZoomStepRecord {
    ZoomStepRecord {
        held_frames: 0,
        switch: Some(SwitchTiming {
            method: LensSwitchMethod::PromotedStandby,
            ..healthy_switch_step(name).switch.expect("смена")
        }),
        seamless: Some(SeamlessRecord {
            gesture: "wheel",
            leg,
            switch_events: 1,
            ready_ms: Some(240.0),
            fps: Some(FpsPair {
                primary: 59.6,
                standby: 59.2,
            }),
            gap: Some(SwitchGap {
                gap_ms: 17.0,
                period_ms: 16.7,
            }),
            closed_ms: None,
        }),
        ..healthy_switch_step(name)
    }
}

/// Здоровое отпускание: колесо ушло, тёплая закрыта через 3,7 с.
pub fn healthy_release_step(name: &str) -> ZoomStepRecord {
    ZoomStepRecord {
        seamless: Some(SeamlessRecord {
            gesture: "wheel",
            leg: "release",
            switch_events: 0,
            ready_ms: None,
            fps: None,
            gap: None,
            closed_ms: Some(3_700.0),
        }),
        ..healthy_crop_step(name)
    }
}
