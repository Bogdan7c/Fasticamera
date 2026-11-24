use isp_vulkan::viewfinder::{IspStage, IspStageSet};
use render_loop::viewfinder::{GpuWorkSuppression, ViewfinderPainter};

use super::*;
use crate::scenario::camera::stage_budget_states::RowStates;
use crate::viewfinder_scenario_config::StageBudgetRowSetting;

fn quiet_thermal() -> ThermalSnapshot {
    ThermalSnapshot {
        status: None,
        headroom: Some(0.5),
        error: None,
    }
}

fn gpu_time(p50: f64) -> GpuTimeMs {
    GpuTimeMs {
        samples: 600,
        p50_ms: p50,
        p95_ms: p50 + 0.2,
        max_ms: p50 + 1.0,
    }
}

/// Показание рендера, где всё как у продукта; тесты меняют только нужное.
fn product_reading() -> Reading {
    Reading {
        painter: ViewfinderPainter::Isp,
        stages: IspStageSet::ALL,
        suppression: GpuWorkSuppression::NONE,
        isp_dispatch: Some(gpu_time(2.0)),
        frame_total: Some(gpu_time(3.0)),
        ui_overlay_gpu: Some(gpu_time(1.5)),
        scopes_gpu: Some(gpu_time(0.8)),
    }
}

fn tick(reading: Reading, passes: u64, drawn: u64, suppressed: u64, refresh_hz: Option<f64>) -> Tick {
    Tick {
        viewfinder: Some((
            reading,
            CounterSlice {
                scopes_passes: passes,
                ui_frames_drawn: drawn,
                ui_frames_suppressed: suppressed,
            },
        )),
        refresh_hz,
    }
}

fn observer(row: StageBudgetRowSetting, stage: StageSetting, surface: StageBudgetSurface) -> WindowObserver {
    let state = RowStates::of(row).state_for(stage);
    WindowObserver::begin(0, stage, state, surface, 6.0, WindowIssued::default(), quiet_thermal())
}

/// Два показания: начало и конец измеряемой части; счётчики между ними двигаются так, как задано.
fn run_window(
    observer: &mut WindowObserver,
    reading: Reading,
    start: (u64, u64, u64),
    end: (u64, u64, u64),
    hz: Option<f64>,
) {
    observer.record(tick(reading.clone(), start.0, start.1, start.2, hz));
    observer.record(tick(reading, end.0, end.1, end.2, hz));
}

fn mismatches_of(closed: &ClosedWindow) -> Vec<StateMismatch> {
    match &closed.condition.refusal {
        Some(WindowRefusal::StateNotApplied { mismatches }) => mismatches.clone(),
        other => panic!("ждали state_not_applied, получили {other:?}"),
    }
}

#[test]
fn a_product_window_on_calibrate_is_accepted_only_when_scopes_and_ui_actually_ran() {
    let mut window = observer(
        StageBudgetRowSetting::ColorScopes,
        StageSetting::On,
        StageBudgetSurface::CalibrateScreen,
    );
    run_window(&mut window, product_reading(), (100, 500, 0), (400, 800, 0), None);
    let closed = window.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(closed.condition.refusal, None);
    assert!(closed.sample.setting_in_effect);
    assert_eq!(
        closed.condition.counters,
        Some(CounterDelta {
            scopes_passes: 300,
            ui_frames_drawn: 300,
            ui_frames_suppressed: 0
        })
    );
    // Время подавляемых работ в окне A читается: они выполнялись.
    assert_eq!(closed.sample.scopes, Some(gpu_time(0.8)));
    assert_eq!(closed.sample.ui_overlay, Some(gpu_time(1.5)));
}

#[test]
fn scopes_that_did_not_advance_in_a_product_window_are_state_not_applied() {
    let mut window = observer(
        StageBudgetRowSetting::ColorScopes,
        StageSetting::On,
        StageBudgetSurface::CalibrateScreen,
    );
    run_window(&mut window, product_reading(), (100, 500, 0), (100, 800, 0), None);
    let closed = window.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(mismatches_of(&closed), [StateMismatch::ScopesIdle]);
}

#[test]
fn suppressed_scopes_must_stay_still_and_their_stale_gpu_time_is_not_read() {
    let suppressed = RowStates::of(StageBudgetRowSetting::ColorScopes).off.suppression;
    let reading = Reading {
        suppression: suppressed,
        ..product_reading()
    };
    // Приборы подавлены и стоят: окно принято, а залипшее время приборов (0,8 мс) в окно не попадает.
    let mut window = observer(
        StageBudgetRowSetting::ColorScopes,
        StageSetting::Off,
        StageBudgetSurface::CalibrateScreen,
    );
    run_window(&mut window, reading.clone(), (100, 500, 0), (100, 800, 0), None);
    let closed = window.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(closed.condition.refusal, None);
    assert_eq!(
        closed.sample.scopes, None,
        "подавленная работа не выполнялась: времени нет"
    );
    assert_eq!(
        closed.sample.ui_overlay,
        Some(gpu_time(1.5)),
        "UI в этом окне не подавлен"
    );

    // Подавлены, но проходы всё равно растут: подавление не сработало.
    let mut leaking = observer(
        StageBudgetRowSetting::ColorScopes,
        StageSetting::Off,
        StageBudgetSurface::CalibrateScreen,
    );
    run_window(&mut leaking, reading, (100, 500, 0), (250, 800, 0), None);
    let closed = leaking.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(mismatches_of(&closed), [StateMismatch::ScopesStillRunning]);
}

#[test]
fn suppressed_ui_must_count_suppressed_frames_and_draw_none() {
    let suppressed = RowStates::of(StageBudgetRowSetting::UiOverlay).off.suppression;
    let reading = Reading {
        suppression: suppressed,
        ..product_reading()
    };
    let mut good = observer(
        StageBudgetRowSetting::UiOverlay,
        StageSetting::Off,
        StageBudgetSurface::PhotoScreen,
    );
    run_window(&mut good, reading.clone(), (0, 500, 10), (0, 500, 310), None);
    let closed = good.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(closed.condition.refusal, None);
    assert_eq!(closed.sample.ui_overlay, None);

    // Кадры UI продолжают рисоваться и не считаются подавленными: маска не дошла до прохода UI.
    let mut broken = observer(
        StageBudgetRowSetting::UiOverlay,
        StageSetting::Off,
        StageBudgetSurface::PhotoScreen,
    );
    run_window(&mut broken, reading, (0, 500, 10), (0, 800, 10), None);
    let closed = broken.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(
        mismatches_of(&closed),
        [StateMismatch::UiStillDrawing, StateMismatch::UiSuppressionNotCounted]
    );
}

#[test]
fn a_mask_or_painter_that_the_render_does_not_show_is_state_not_applied() {
    // Строка ждёт мозаику и обе подавленные работы, а рендер показывает продукт.
    let mut window = observer(
        StageBudgetRowSetting::WholeProduct,
        StageSetting::Off,
        StageBudgetSurface::PhotoScreen,
    );
    run_window(&mut window, product_reading(), (0, 500, 0), (0, 800, 0), None);
    let closed = window.finish(quiet_thermal()).expect("окно закрыто");
    assert!(!closed.sample.setting_in_effect);
    let mismatches = mismatches_of(&closed);
    assert!(mismatches.contains(&StateMismatch::PainterOrStages), "{mismatches:?}");
    assert!(
        mismatches.contains(&StateMismatch::GpuWorkSuppression),
        "{mismatches:?}"
    );
}

#[test]
fn a_window_without_viewfinder_diagnostics_cannot_confirm_anything() {
    let mut window = observer(
        StageBudgetRowSetting::Stage(crate::viewfinder_scenario_config::IspStageSetting::Tone),
        StageSetting::On,
        StageBudgetSurface::BareViewfinder,
    );
    window.record(Tick {
        viewfinder: None,
        refresh_hz: None,
    });
    let closed = window.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(mismatches_of(&closed), [StateMismatch::NoViewfinderDiagnostics]);
    assert!(!closed.sample.setting_in_effect);
    assert_eq!(closed.sample.frame_total, None);
}

#[test]
fn a_window_with_no_measured_tick_leaves_no_record() {
    let window = observer(
        StageBudgetRowSetting::UiOverlay,
        StageSetting::On,
        StageBudgetSurface::PhotoScreen,
    );
    assert_eq!(window.finish(quiet_thermal()), None);
}

#[test]
fn old_rows_on_the_bare_viewfinder_have_no_counter_expectations() {
    // Голый видоискатель: UI и приборов нет, их счётчики стоят — и это не расхождение.
    let mut window = observer(
        StageBudgetRowSetting::Stage(crate::viewfinder_scenario_config::IspStageSetting::Tone),
        StageSetting::Off,
        StageBudgetSurface::BareViewfinder,
    );
    let reading = Reading {
        stages: IspStageSet::ALL.without(IspStage::Tone),
        ..product_reading()
    };
    run_window(&mut window, reading, (0, 0, 0), (0, 0, 0), None);
    let closed = window.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(closed.condition.refusal, None);
    assert!(closed.sample.setting_in_effect);
}

#[test]
fn refresh_120_window_is_accepted_at_120_and_refused_at_60_with_the_observed_number() {
    let at = |hz: f64| {
        let mut window = observer(
            StageBudgetRowSetting::Display120Hz,
            StageSetting::On,
            StageBudgetSurface::PhotoScreen,
        );
        run_window(&mut window, product_reading(), (0, 500, 0), (0, 800, 0), Some(hz));
        window.finish(quiet_thermal()).expect("окно закрыто")
    };
    assert_eq!(at(119.8).condition.refusal, None);
    let refused = at(60.0);
    assert_eq!(
        refused.condition.refusal,
        Some(WindowRefusal::DisplayRefreshNotReached {
            expected_hz: 120.0,
            observed_mean_hz: Some(60.0),
            tolerance_hz: 6.0
        })
    );
    assert_eq!(refused.condition.display.map(|display| display.mean_hz), Some(60.0));
    assert!(
        refused
            .condition
            .refusal
            .as_ref()
            .expect("отказ")
            .describe()
            .contains("display_refresh_not_reached")
    );
}

#[test]
fn refresh_60_window_is_accepted_at_60_and_refused_at_120() {
    let at = |hz: f64| {
        let mut window = observer(
            StageBudgetRowSetting::Display120Hz,
            StageSetting::Off,
            StageBudgetSurface::PhotoScreen,
        );
        run_window(&mut window, product_reading(), (0, 500, 0), (0, 800, 0), Some(hz));
        window.finish(quiet_thermal()).expect("окно закрыто")
    };
    assert_eq!(at(60.0).condition.refusal, None);
    assert!(matches!(
        at(120.0).condition.refusal,
        Some(WindowRefusal::DisplayRefreshNotReached { expected_hz, .. }) if expected_hz == 60.0
    ));
}

#[test]
fn an_unknown_refresh_cycle_is_a_refusal_not_a_silent_pass() {
    let mut window = observer(
        StageBudgetRowSetting::Display120Hz,
        StageSetting::On,
        StageBudgetSurface::PhotoScreen,
    );
    run_window(&mut window, product_reading(), (0, 500, 0), (0, 800, 0), None);
    let closed = window.finish(quiet_thermal()).expect("окно закрыто");
    assert_eq!(
        closed.condition.refusal,
        Some(WindowRefusal::DisplayRefreshNotReached {
            expected_hz: 120.0,
            observed_mean_hz: None,
            tolerance_hz: 6.0
        })
    );
}

#[test]
fn rows_without_a_target_refresh_never_refuse_on_frequency() {
    let mut window = observer(
        StageBudgetRowSetting::UiOverlay,
        StageSetting::On,
        StageBudgetSurface::PhotoScreen,
    );
    run_window(&mut window, product_reading(), (0, 500, 0), (0, 800, 0), Some(37.0));
    assert_eq!(window.finish(quiet_thermal()).expect("окно").condition.refusal, None);
}

#[test]
fn thermal_at_both_window_edges_is_kept() {
    let start = ThermalSnapshot {
        status: Some(shell_contract::thermal::ThermalStatus::Light),
        headroom: Some(0.4),
        error: None,
    };
    let end = ThermalSnapshot {
        status: Some(shell_contract::thermal::ThermalStatus::Moderate),
        headroom: Some(0.9),
        error: None,
    };
    let state = RowStates::of(StageBudgetRowSetting::UiOverlay).on;
    let mut window = WindowObserver::begin(
        3,
        StageSetting::On,
        state,
        StageBudgetSurface::PhotoScreen,
        6.0,
        WindowIssued::default(),
        start.clone(),
    );
    run_window(&mut window, product_reading(), (0, 500, 0), (0, 800, 0), None);
    let closed = window.finish(end.clone()).expect("окно закрыто");
    assert_eq!(closed.condition.index, 3);
    assert_eq!(closed.condition.thermal_start, start);
    assert_eq!(closed.condition.thermal_end, end);
}

#[test]
fn refresh_stats_ignore_non_frequencies_and_track_min_max_mean() {
    let mut stats = RefreshStats::default();
    for hz in [f64::NAN, 0.0, -5.0, 60.0, 120.0, 90.0] {
        stats.observe(hz);
    }
    assert_eq!(stats.mean_hz(), Some(90.0));
    let reading = stats.reading().expect("показания есть");
    assert_eq!((reading.min_hz, reading.max_hz, reading.readings), (60.0, 120.0, 3));
    assert_eq!(RefreshStats::default().mean_hz(), None);
}

#[test]
fn refusal_maps_to_the_experiment_rejection_kinds_the_table_prints() {
    let state = WindowRefusal::StateNotApplied {
        mismatches: vec![StateMismatch::ScopesIdle],
    };
    let display = WindowRefusal::DisplayRefreshNotReached {
        expected_hz: 120.0,
        observed_mean_hz: Some(60.0),
        tolerance_hz: 6.0,
    };
    assert_eq!(
        state.to_rejection(4, StageSetting::On),
        AbRejection::StateNotApplied {
            index: 4,
            stage: StageSetting::On
        }
    );
    assert_eq!(
        display.to_rejection(1, StageSetting::Off),
        AbRejection::DisplayRefreshNotReached {
            index: 1,
            stage: StageSetting::Off
        }
    );
    assert!(state.describe().contains("scopes_idle"), "{}", state.describe());
}

#[test]
fn under_the_debug_mosaic_ui_and_scopes_stay_untouched_and_that_is_not_a_mismatch() {
    // Путь кадра мозаики не доходит до развилки UI: кадры не рисуются и не считаются подавленными, приборы стоят.
    let states = RowStates::of(StageBudgetRowSetting::WholeProduct);
    let reading = Reading {
        painter: ViewfinderPainter::DebugMosaic,
        suppression: states.off.suppression,
        isp_dispatch: None,
        ..product_reading()
    };
    let mut window = observer(
        StageBudgetRowSetting::WholeProduct,
        StageSetting::Off,
        StageBudgetSurface::CalibrateScreen,
    );
    run_window(&mut window, reading.clone(), (7, 500, 288), (7, 500, 288), None);
    assert_eq!(window.finish(quiet_thermal()).expect("окно").condition.refusal, None);

    // А вот UI, продолжающий рисоваться под мозаикой, — расхождение: так не должно быть ни по какому состоянию.
    let mut drawing = observer(
        StageBudgetRowSetting::WholeProduct,
        StageSetting::Off,
        StageBudgetSurface::CalibrateScreen,
    );
    run_window(&mut drawing, reading, (7, 500, 288), (7, 800, 288), None);
    let closed = drawing.finish(quiet_thermal()).expect("окно");
    assert_eq!(mismatches_of(&closed), [StateMismatch::UiStillDrawing]);
}
