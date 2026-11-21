//! Исполнитель `zoom`: защитные ветки и тонкая синхронизация (шаг без намерения линзы, потеря записей кольца, ожидание
//! инерции колеса и скольжения экрана, полнота записи шага). Стенд и подставные срезы — из `zoom_runner_tests`.

use super::*;

#[test]
fn a_step_without_a_published_lens_intent_fails() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 4),
    );
    rig.tick(0, &[]);
    rig.screen = screen(590, LensFacing::Back, 4); // намерение не прибавилось
    rig.frames.push(live_frame(1_100, "3", PreviewMirror::None));
    rig.tick(150, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    let tick = run_to_record(&mut rig);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал");
    };
    assert!(reason.contains("не положил намерение объектива"), "{reason}");
}

#[test]
fn the_screen_showing_another_ratio_than_the_chosen_stop_fails_the_step() {
    let mut rig = Rig::new(
        vec![step("03_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(1_400, LensFacing::Back, 1); // глайд застрял на 1,4×
    let tick = run_to_record(&mut rig);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал");
    };
    assert!(reason.contains("1400"), "{reason}");
}

#[test]
fn a_tap_for_a_ratio_without_a_button_fails_before_touching_the_screen() {
    let mut rig = Rig::new(
        vec![step("01_tap_5", ZoomAction::TapStop { ratio: 5.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    let tick = rig.tick(0, &[]);

    assert!(tick.injections.is_empty(), "действие не вложено");
    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал");
    };
    assert!(reason.contains("нет кнопки"), "{reason}");
}

#[test]
fn a_step_does_not_start_while_the_engine_is_not_streaming() {
    let mut view = engine(LensRole::Wide, "2", LensFacing::Back);
    view.streaming = false;
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        view,
        screen(1_000, LensFacing::Back, 0),
    );
    let tick = rig.tick(0, &[]);

    assert!(tick.injections.is_empty());
    assert!(matches!(tick.state, RunnerState::Failed(_)));
}

#[test]
fn a_ring_that_lost_records_lets_the_step_finish_with_the_gap_marked_instead_of_hanging() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(590, LensFacing::Back, 1);
    // Кольцо целиком обновилось за один тик: между старым и новым снимком — дыра, нужного кадра в нём нет.
    rig.frames = vec![
        live_frame(1_900, "2", PreviewMirror::None),
        live_frame(1_916, "2", PreviewMirror::None),
    ];
    rig.tick(100, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    run_to_record(&mut rig);

    let record = &rig.runner.records()[0];
    assert!(record.ring_overran);
    assert_eq!(record.first_live_frame_ms, None, "кадр не измерен, а не «нулевой»");
    assert_eq!(record.mirror, MirrorCheck::NoFrameObserved);
}

#[test]
fn unfinished_names_list_the_running_step_and_the_pending_ones() {
    let mut rig = Rig::new(
        vec![
            step("01_tap_2", ZoomAction::TapStop { ratio: 2.0 }),
            step("02_tap_1", ZoomAction::TapStop { ratio: 1.0 }),
        ],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    assert_eq!(rig.runner.unfinished_names(), vec!["01_tap_2", "02_tap_1"]);
    rig.tick(0, &[]);
    assert_eq!(
        rig.runner.unfinished_names(),
        vec!["01_tap_2", "02_tap_1"],
        "идущий шаг ещё не закончен"
    );
}

#[test]
fn after_the_last_step_the_runner_reports_all_done_and_stays_idle() {
    let mut rig = Rig::new(
        vec![step("01_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(2_000, LensFacing::Back, 1);
    run_to_record(&mut rig);

    let done = rig.tick(16, &[]);
    assert_eq!(done.state, RunnerState::AllDone);
    assert!(done.injections.is_empty());
    assert!(rig.runner.unfinished_names().is_empty());
}

#[test]
fn a_drag_step_waits_for_the_inertia_to_stop_and_the_wheel_to_close_before_it_is_recorded() {
    let mut rig = Rig::new(
        vec![step("09_wheel_drag", ZoomAction::WheelDrag)],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    // Колесо открыто и катится по инерции: до конца протяжки и после.
    rig.screen = ScreenView {
        zoom: ZoomView {
            wheel_open: true,
            ..rig.screen.zoom
        },
        lens_published: 5,
        ..rig.screen
    };
    for _ in 0..80 {
        rig.tick(16, &[]);
    }
    assert!(
        rig.runner.records().is_empty(),
        "колесо открыто — шаг не записан, тап следующего шага перебила бы инерция"
    );

    rig.screen = ScreenView {
        zoom: ZoomView {
            wheel_open: false,
            ratio_milli: 7_000,
            ..rig.screen.zoom
        },
        ..rig.screen
    };
    for _ in 0..40 {
        rig.tick(16, &[]);
    }
    assert_eq!(rig.runner.records().len(), 1, "колесо ушло — шаг записан");
    assert_eq!(rig.runner.records()[0].screen_ratio, 7.0);
}

#[test]
fn a_tap_step_waits_while_the_screen_is_still_gliding_to_the_stop() {
    let mut rig = Rig::new(
        vec![step("03_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = ScreenView {
        zoom: ZoomView {
            ratio_milli: 1_400,
            gliding: true,
            ..rig.screen.zoom
        },
        lens_published: 3,
        ..rig.screen
    };
    for _ in 0..30 {
        rig.tick(100, &[]);
    }
    assert!(
        rig.runner.records().is_empty(),
        "переход ещё идёт: судить значение рано"
    );

    rig.screen = ScreenView {
        zoom: ZoomView {
            ratio_milli: 2_000,
            gliding: false,
            ..rig.screen.zoom
        },
        ..rig.screen
    };
    rig.tick(100, &[]);
    assert_eq!(rig.runner.records().len(), 1);
    assert_eq!(rig.runner.records()[0].failure, None);
}

#[test]
fn the_step_record_tells_what_the_screen_showed_and_which_continuity_counters_moved() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(590, LensFacing::Back, 1);
    // После действия экран 300 мс был чёрным «нет картинки», потом пошли живые кадры новой камеры.
    rig.frames = vec![
        FrameRecord {
            present_time: at_ms(rig.elapsed_ms + 16),
            label: FrameLabel::Black(BlackKind::NoPicture),
            backdrop: BackdropContinuity::Live,
            crop: None,
            held_magnification: 1.0,
        },
        live_frame(rig.elapsed_ms + 316, "3", PreviewMirror::None),
    ];
    rig.counters.black_frames_before_first_picture = 1;
    rig.counters.live_frames_presented = 1;
    rig.isp_counters.frames_hidden_cold_start = 2;
    rig.tick(100, &[switched(LensRole::Wide, LensRole::UltraWide)]);
    run_to_record(&mut rig);

    let record = &rig.runner.records()[0];
    assert_eq!(record.continuity.black_frames_before_first_picture, 1);
    assert_eq!(record.continuity.live_frames_presented, 1);
    assert_eq!(record.continuity.frames_hidden_cold_start, 2);
    assert_eq!(record.held_frames, 0);
    let first_black = record
        .presented_frames
        .iter()
        .find(|bucket| bucket.label == "black_no_picture");
    assert_eq!(
        first_black.map(|bucket| (bucket.camera.clone(), bucket.frames)),
        Some((None, 1))
    );
    assert!(
        record
            .presented_frames
            .iter()
            .any(|bucket| bucket.label == "live" && bucket.camera.as_deref() == Some("3"))
    );
}
