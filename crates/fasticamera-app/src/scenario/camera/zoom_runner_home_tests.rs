//! Исполнитель `zoom`, привязанный к дому (Back 1×): подготовка в начале, возврат после провала и суд над событиями вне
//! шагов. Стенд и подставные срезы — из `zoom_runner_tests`.

use super::*;

fn anchored_rig(steps: Vec<ZoomStep>, engine: EngineView, screen: ScreenView) -> Rig {
    let mut rig = Rig::new(steps.clone(), engine, screen);
    rig.runner = ZoomRunner::new(rig.config.clone(), steps).anchored_at_back_one_x();
    rig
}

fn failed_switch(from: LensRole, to: LensRole) -> CameraEvent {
    CameraEvent::LensSwitchFailed {
        from,
        to,
        reason: LensSwitchFailure::Unavailable {
            reason: camera_contract::availability::CameraUnavailableReason::DeviceFailure,
        },
    }
}

/// Все вложения-действия за тики, пока исполнитель не скажет что-то кроме `Running` или не кончатся тики.
fn drain_actions(rig: &mut Rig, max_ticks: usize) -> (Vec<ScreenAction>, RunnerState) {
    let mut actions = Vec::new();
    let mut state = RunnerState::Running;
    for _ in 0..max_ticks {
        let tick = rig.tick(100, &[]);
        for injection in tick.injections {
            if let Injection::Action(action) = injection {
                actions.push(*action);
            }
        }
        state = tick.state;
        if state != RunnerState::Running {
            break;
        }
    }
    (actions, state)
}

#[test]
fn a_run_that_starts_on_the_front_camera_first_flips_back_and_taps_one_x_without_recording_it() {
    let mut rig = anchored_rig(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Front, "1", LensFacing::Front),
        screen(1_000, LensFacing::Front, 0),
    );
    // Тик 0: первым действием идёт переворот на заднюю, а не тап плана по `.6`.
    let first = rig.tick(0, &[]);
    assert!(
        matches!(only_action(&first), ScreenAction::FlipCameraRequested { .. }),
        "{first:?}"
    );
    // Телефон перевернулся на основную; экран вернул последнее заднее значение.
    rig.engine = engine(LensRole::Wide, "2", LensFacing::Back);
    rig.screen = screen(1_000, LensFacing::Back, 1);
    rig.frames.push(live_frame(1_300, "2", PreviewMirror::None));
    rig.tick(320, &[switched(LensRole::Front, LensRole::Wide)]);

    // Дальше: тап по `1` (подготовка, объектив тот же), затем настоящий шаг плана — тап по `.6`.
    rig.screen = screen(1_000, LensFacing::Back, 2);
    let mut taps = Vec::new();
    for _ in 0..100 {
        let tick = rig.tick(100, &[]);
        for injection in &tick.injections {
            if let Injection::Action(action) = injection
                && let ScreenAction::ZoomStopTapped { stop, .. } = **action
            {
                taps.push(stop);
                if taps.len() == 2 {
                    // Второй тап — шаг плана: движок переходит на ширик.
                    rig.engine = engine(LensRole::UltraWide, "3", LensFacing::Back);
                    rig.screen = screen(590, LensFacing::Back, 3);
                    rig.frames
                        .push(live_frame(rig.elapsed_ms + 100, "3", PreviewMirror::None));
                }
            }
        }
        if taps.len() == 2 && rig.runner.records().is_empty() {
            rig.tick(100, &[switched(LensRole::Wide, LensRole::UltraWide)]);
        }
        if !rig.runner.records().is_empty() {
            break;
        }
    }

    let group = rig.engine.catalog.back.as_ref().expect("задние камеры");
    let (one_x, _) = nearest_stop(group, 1.0).expect("кнопка 1");
    let (ultra_wide, _) = nearest_stop(group, 0.6).expect("кнопка .6");
    assert_eq!(taps, vec![one_x, ultra_wide], "сначала дом (1×), потом план (.6)");
    let names: Vec<&str> = rig.runner.records().iter().map(|record| record.name.as_str()).collect();
    assert_eq!(names, vec!["01_tap_0.6"], "подготовка в отчёт не попадает");
    assert_eq!(rig.runner.records()[0].failure, None);
}

#[test]
fn a_screen_already_at_home_gets_no_extra_preparation() {
    let mut rig = anchored_rig(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    let first = rig.tick(0, &[]);
    assert!(
        matches!(only_action(&first), ScreenAction::ZoomStopTapped { .. }),
        "первым действием сразу идёт шаг плана"
    );
    let group = rig.engine.catalog.back.as_ref().expect("задние камеры");
    let (ultra_wide, _) = nearest_stop(group, 0.6).expect("кнопка .6");
    assert!(matches!(only_action(&first), ScreenAction::ZoomStopTapped { stop, .. } if *stop == ultra_wide));
}

#[test]
fn preparation_waits_for_an_open_wheel_instead_of_tapping_over_it() {
    let mut rig = anchored_rig(
        vec![step("01_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        ScreenView {
            zoom: ZoomView {
                ratio_milli: 1_000,
                facing: LensFacing::Back,
                wheel_open: true,
                gliding: false,
            },
            ..screen(1_000, LensFacing::Back, 0)
        },
    );
    for _ in 0..5 {
        let tick = rig.tick(100, &[]);
        assert!(tick.injections.is_empty(), "колесо открыто — ничего не вкладываем");
        assert_eq!(tick.state, RunnerState::Running);
    }
    rig.screen = screen(1_000, LensFacing::Back, 0);
    let tick = rig.tick(100, &[]);
    assert!(
        matches!(only_action(&tick), ScreenAction::ZoomStopTapped { .. }),
        "колесо закрылось"
    );
}

#[test]
fn a_failure_in_the_middle_still_returns_the_phone_to_back_one_x_before_reporting_it() {
    let mut rig = anchored_rig(
        vec![
            step("01_flip_to_front", ZoomAction::Flip),
            step("02_tap_2", ZoomAction::TapStop { ratio: 2.0 }),
        ],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    // Переворот прошёл: телефон на фронталке.
    rig.engine = engine(LensRole::Front, "1", LensFacing::Front);
    rig.screen = screen(1_000, LensFacing::Front, 1);
    rig.frames.push(live_frame(1_300, "1", PreviewMirror::Horizontal));
    rig.tick(320, &[switched(LensRole::Wide, LensRole::Front)]);
    run_to_record(&mut rig);
    // Следующий шаг (2×) стартует на фронталке и тут же ловит отказ движка.
    rig.tick(100, &[]);
    let failing = rig.tick(100, &[failed_switch(LensRole::Front, LensRole::Wide)]);

    // Провал не объявлен сразу: исполнитель сначала возвращает телефон домой.
    assert_eq!(failing.state, RunnerState::Running, "возврат к дому идёт");
    let (actions, state) = drain_actions(&mut rig, 3);
    assert!(
        actions
            .iter()
            .any(|action| matches!(action, ScreenAction::FlipCameraRequested { .. })),
        "возврат начинается с переворота на заднюю: {actions:?}"
    );
    // Телефон вернулся на основную 1×: возврат доделывает тапом по `1` и объявляет исходный провал.
    rig.engine = engine(LensRole::Wide, "2", LensFacing::Back);
    rig.screen = screen(1_000, LensFacing::Back, 2);
    rig.frames
        .push(live_frame(rig.elapsed_ms + 50, "2", PreviewMirror::None));
    let mut all_actions = actions;
    let mut state = state;
    let mut pending_events = vec![switched(LensRole::Front, LensRole::Wide)];
    for _ in 0..200 {
        if state != RunnerState::Running {
            break;
        }
        let tick = rig.tick(100, &std::mem::take(&mut pending_events));
        all_actions.extend(tick.injections.into_iter().filter_map(|injection| match injection {
            Injection::Action(action) => Some(*action),
            Injection::Touch(_) => None,
        }));
        state = tick.state;
    }
    let RunnerState::Failed(reason) = state else {
        panic!("ждали исходный провал после возврата, получили {state:?}");
    };
    assert!(
        reason.contains("02_tap_2") && reason.contains("front→wide") && reason.contains("не удалась"),
        "названа причина исходного провала: {reason}"
    );
    assert!(
        all_actions
            .iter()
            .any(|action| matches!(action, ScreenAction::ZoomStopTapped { .. })),
        "возврат закончен тапом по 1×: {all_actions:?}"
    );
    assert_eq!(
        rig.runner.records().len(),
        2,
        "шаг 1 и провалившийся шаг 2; возврат в отчёт не попадает"
    );
}

#[test]
fn without_anchoring_a_failure_is_reported_at_once_and_nothing_is_restored() {
    let mut rig = Rig::new(
        vec![step("01_tap_0.6", ZoomAction::TapStop { ratio: 0.6 })],
        engine(LensRole::Front, "1", LensFacing::Front),
        screen(1_000, LensFacing::Front, 0),
    );
    rig.tick(0, &[]);
    let tick = rig.tick(100, &[failed_switch(LensRole::Front, LensRole::UltraWide)]);
    assert!(matches!(tick.state, RunnerState::Failed(_)), "{tick:?}");
    assert!(tick.injections.is_empty());
}

#[test]
fn a_failed_switch_between_steps_fails_the_run_instead_of_being_ignored() {
    let mut rig = Rig::new(
        vec![
            step("01_tap_2", ZoomAction::TapStop { ratio: 2.0 }),
            step("02_tap_1", ZoomAction::TapStop { ratio: 1.0 }),
        ],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(2_000, LensFacing::Back, 1);
    run_to_record(&mut rig);

    // Шаг 1 закончен, шаг 2 ещё не начат: отказ, который пришёл сейчас, не принадлежит ни одному шагу.
    let tick = rig.tick(50, &[failed_switch(LensRole::Wide, LensRole::UltraWide)]);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал, получили {:?}", tick.state);
    };
    assert!(
        reason.contains("после шага 01_tap_2") && reason.contains("wide→ultra_wide") && reason.contains("не удалась"),
        "{reason}"
    );
    assert!(tick.injections.is_empty(), "шаг 2 не начат");
    assert_eq!(
        rig.runner.records().last().map(|record| record.name.as_str()),
        Some("unjudged_events")
    );
    assert_eq!(rig.runner.unfinished_names(), vec!["02_tap_1".to_owned()]);
}

#[test]
fn an_unexpected_lens_switch_after_the_last_step_fails_the_run() {
    let mut rig = Rig::new(
        vec![step("01_tap_2", ZoomAction::TapStop { ratio: 2.0 })],
        engine(LensRole::Wide, "2", LensFacing::Back),
        screen(1_000, LensFacing::Back, 0),
    );
    rig.tick(0, &[]);
    rig.screen = screen(2_000, LensFacing::Back, 1);
    run_to_record(&mut rig);

    let tick = rig.tick(50, &[switched(LensRole::Wide, LensRole::UltraWide)]);

    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал вместо AllDone, получили {:?}", tick.state);
    };
    assert!(
        reason.contains("после шага 01_tap_2") && reason.contains("ни один шаг этого не просил"),
        "{reason}"
    );
}
