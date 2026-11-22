//! Исполнитель шагов `capture` в ручном времени: порядок шагов, RAW включается и возвращается, времена фаз, отказы и
//! учёт серии. Мир (`capture_test_support`) отвечает как экран и служба записи.

use std::time::Duration;

use camera_screen::settings::RawCapture;

use super::super::capture_frame_timing::FrameChoiceView;
use super::super::capture_plan::planned_steps;
use super::super::capture_report::ShotOutcome;
use super::super::capture_test_support::{Behaviour, ChoicePolicy, World, drive, fast_config};
use super::*;

const TICK: Duration = Duration::from_millis(10);
const LIMIT: Duration = Duration::from_secs(60);

fn run(behaviour: Behaviour) -> (World, RunnerState, RunnerRecords) {
    let mut world = World::new(behaviour);
    let mut runner = CaptureRunner::new(fast_config(), planned_steps(), world.now());
    let state = drive(&mut runner, &mut world, TICK, LIMIT);
    (world, state, runner.into_records())
}

fn saved_addresses(records: &RunnerRecords) -> Vec<String> {
    records
        .shots
        .iter()
        .flat_map(|shot| match &shot.outcome {
            ShotOutcome::Saved { primary, raw } => vec![Some(primary.clone()), raw.clone()],
            _ => Vec::new(),
        })
        .flatten()
        .collect()
}

#[test]
fn a_healthy_run_takes_one_heif_then_heif_with_dng_then_a_burst_and_leaves_raw_off() {
    let (world, state, records) = run(Behaviour::default());

    assert_eq!(state, RunnerState::AllDone);
    assert_eq!(
        records.steps.iter().map(|step| step.name).collect::<Vec<_>>(),
        vec!["single_heif", "single_heif_dng", "burst"]
    );
    assert!(
        records.steps.iter().all(|step| step.failure.is_none()),
        "{:?}",
        records.steps
    );
    assert!(records.unfinished.is_empty());
    // 1 + 1 + 10 снимков; файлов 1 + 2 + 10.
    assert_eq!(records.shots.len(), 12);
    assert_eq!(saved_addresses(&records).len(), 13);
    // RAW: включён ровно на втором шаге и возвращён к серии (два нажатия значка).
    assert_eq!(world.raw_taps, 2);
    assert_eq!(world.raw, RawCapture::Off);
    let requested: Vec<bool> = records.shots.iter().map(|shot| shot.raw_requested).collect();
    assert_eq!(requested[..2], [false, true]);
    assert!(
        requested[2..].iter().all(|raw| !raw),
        "серия снимала без RAW: {requested:?}"
    );
    // Времена фаз от нажатия точные (мир ведёт ручное время).
    assert_eq!(records.shots[0].secured_after_ms, Some(30));
    assert_eq!(records.shots[0].saved_after_ms, Some(400));
    assert_eq!(records.shots[0].aspect, "4:3");
    assert_eq!(records.shots[0].zoom_milli, 1_000);
    assert_eq!(records.shots[0].format, "heif");
    // Рендер мира выбирает ровно показанный кадр: нажатие − начало экспозиции = 80 мс, показан за 10 мс до нажатия.
    for shot in &records.shots {
        assert_eq!(
            shot.frame.frame_choice,
            FrameChoiceView::Exact,
            "снимок {}",
            shot.number
        );
        assert_eq!(shot.frame.press_to_frame_ms, Some(80.0), "снимок {}", shot.number);
        assert_eq!(shot.frame.shown_before_press_ms, Some(10.0), "снимок {}", shot.number);
    }
}

#[test]
fn the_dng_step_reports_two_addresses_for_one_shot() {
    let (_world, _state, records) = run(Behaviour::default());

    let ShotOutcome::Saved { primary, raw } = &records.shots[1].outcome else {
        panic!("{:?}", records.shots[1].outcome);
    };
    assert!(primary.contains("/images/media/2"), "{primary}");
    assert!(raw.as_deref().is_some_and(|raw| raw.contains("/file/2")), "{raw:?}");
    assert!(records.shots[1].raw_expected && records.shots[1].raw_requested);
}

#[test]
fn a_burst_at_a_hundred_milliseconds_is_pressed_on_schedule_and_every_press_is_accounted() {
    let (_world, state, records) = run(Behaviour::default());

    assert_eq!(state, RunnerState::AllDone);
    let burst = records.burst.expect("учёт серии");
    assert_eq!(burst.presses, 10);
    assert_eq!(burst.submitted, 10);
    assert_eq!(burst.accepted_by_service, 10);
    assert_eq!(burst.refused_by_screen, 0);
    assert_eq!((burst.saved, burst.failed, burst.unfinished), (10, 0, 0));
    assert!(burst.min_press_gap_ms.is_some_and(|gap| gap >= 100), "{burst:?}");
}

#[test]
fn a_slow_capture_makes_the_screen_refuse_extra_presses_and_the_numbers_still_add_up() {
    // «Кадр захвачен» приходит через 250 мс, нажатия идут каждые 100 мс: затвор занят, часть нажатий экран отвергает.
    let (_world, state, records) = run(Behaviour {
        secured_after: Duration::from_millis(250),
        ..Behaviour::default()
    });

    assert_eq!(state, RunnerState::AllDone, "{:?}", records.failure);
    let burst = records.burst.expect("учёт серии");
    assert!(
        burst.refused_by_screen > 0,
        "отказы экрана должны быть видны: {burst:?}"
    );
    assert_eq!(
        u64::from(burst.presses),
        burst.submitted + burst.refused_by_screen,
        "нажатия = попытки постановки + отказы экрана"
    );
    assert_eq!(
        u64::from(burst.saved),
        burst.accepted_by_service,
        "принятые все сохранены"
    );
    assert_eq!(burst.screen_actions_rejected, burst.refused_by_screen);
}

#[test]
fn a_service_that_rejects_presses_is_counted_apart_from_screen_refusals() {
    // Мир отвергает как «занят» нажатия 3 и 4 от начала прогона (это 1-е и 2-е нажатие серии).
    let (_world, state, records) = run(Behaviour {
        service_rejects_press: [3, 4].into_iter().collect(),
        ..Behaviour::default()
    });

    assert_eq!(state, RunnerState::AllDone, "{:?}", records.failure);
    let burst = records.burst.expect("учёт серии");
    assert_eq!(burst.rejected_by_service, 2);
    assert_eq!(burst.accepted_by_service, 8);
    assert_eq!(burst.refused_by_screen, 0);
    assert_eq!(burst.saved, 8);
}

#[test]
fn a_failed_phase_of_a_single_shot_stops_the_run_with_the_service_reason() {
    let (_world, state, records) = run(Behaviour {
        failing_requests: [1].into_iter().collect(),
        ..Behaviour::default()
    });

    let RunnerState::Failed(reason) = state else {
        panic!("{state:?}");
    };
    assert!(
        reason.contains("снимок 1 сорвался") && reason.contains("MediaStore отказал"),
        "{reason}"
    );
    assert_eq!(records.steps.len(), 1);
    assert_eq!(records.steps[0].failure.as_deref(), Some(reason.as_str()));
    assert_eq!(records.unfinished, vec!["single_heif_dng", "burst"]);
    assert!(matches!(records.shots[0].outcome, ShotOutcome::Failed { .. }));
}

#[test]
fn a_failed_burst_shot_is_counted_as_failed_and_not_hidden() {
    // Первый снимок серии — запрос 3 (единичные были 1 и 2).
    let (_world, state, records) = run(Behaviour {
        failing_requests: [3].into_iter().collect(),
        ..Behaviour::default()
    });

    assert_eq!(state, RunnerState::AllDone, "серия сама не падает: её судит вердикт");
    let burst = records.burst.expect("учёт серии");
    assert_eq!((burst.saved, burst.failed), (9, 1));
}

#[test]
fn a_raw_icon_that_changes_nothing_fails_the_step_naming_the_setting() {
    let (world, state, records) = run(Behaviour {
        raw_toggle_works: false,
        ..Behaviour::default()
    });

    let RunnerState::Failed(reason) = state else {
        panic!("{state:?}");
    };
    assert!(reason.contains("значок RAW не переключил настройку"), "{reason}");
    assert_eq!(world.raw_taps, 1, "значок нажат один раз, а не молотится");
    assert_eq!(
        records.steps.iter().map(|step| step.name).collect::<Vec<_>>(),
        vec!["single_heif", "single_heif_dng"]
    );
    assert_eq!(records.unfinished, vec!["burst"]);
}

#[test]
fn a_press_the_service_never_sees_fails_with_the_counters_that_explain_it() {
    let (_world, state, records) = run(Behaviour {
        service_rejects_press: [1].into_iter().collect(),
        ..Behaviour::default()
    });

    let RunnerState::Failed(reason) = state else {
        panic!("{state:?}");
    };
    assert!(
        reason.contains("не дошло до сервиса") && reason.contains("отвергнуто сервисом 1"),
        "{reason}"
    );
    assert!(records.shots.is_empty());
}

#[test]
fn a_service_that_never_reports_a_captured_frame_fails_on_the_capture_deadline() {
    let (_world, state, records) = run(Behaviour {
        silent_service: true,
        ..Behaviour::default()
    });

    let RunnerState::Failed(reason) = state else {
        panic!("{state:?}");
    };
    assert!(reason.contains("«кадр захвачен» не пришёл"), "{reason}");
    assert_eq!(records.shots[0].outcome, ShotOutcome::Unfinished { secured: false });
}

#[test]
fn a_captured_but_never_saved_shot_fails_on_the_saving_deadline() {
    let (_world, state, records) = run(Behaviour {
        never_saves: true,
        ..Behaviour::default()
    });

    let RunnerState::Failed(reason) = state else {
        panic!("{state:?}");
    };
    assert!(reason.contains("«сохранено» не пришло"), "{reason}");
    assert_eq!(records.shots[0].outcome, ShotOutcome::Unfinished { secured: true });
}

#[test]
fn a_burst_whose_results_never_arrive_fails_on_the_drain_deadline_and_counts_them_unfinished() {
    // Единичные снимки (запросы 1 и 2) сохраняются, а у серии итогов нет.
    let mut world = World::new(Behaviour {
        no_result_after_request: Some(2),
        ..Behaviour::default()
    });
    let mut config = fast_config();
    config.burst.drain_timeout_ms = 1_000;
    let mut runner = CaptureRunner::new(config, planned_steps(), world.now());

    let state = drive(&mut runner, &mut world, TICK, LIMIT);

    let RunnerState::Failed(reason) = state else {
        panic!("{state:?}");
    };
    assert!(reason.contains("итоги снимков серии не пришли"), "{reason}");
    let records = runner.into_records();
    let burst = records.burst.expect("учёт серии есть и при провале");
    assert_eq!(burst.saved, 0);
    assert_eq!(burst.unfinished, 10, "{burst:?}");
}

#[test]
fn losing_the_real_service_mid_run_fails_the_run() {
    let mut world = World::new(Behaviour::default());
    let mut runner = CaptureRunner::new(fast_config(), planned_steps(), world.now());
    world.mode_is_real = false;

    let state = drive(&mut runner, &mut world, TICK, LIMIT);

    assert!(
        matches!(&state, RunnerState::Failed(reason) if reason.contains("перестал быть настоящим")),
        "{state:?}"
    );
    assert_eq!(world.presses, 0, "до нажатий дело не дошло");
}

#[test]
fn a_substituting_render_shows_up_in_every_shot_with_its_reason_and_no_shown_time() {
    let (_world, state, records) = run(Behaviour {
        choice: ChoicePolicy::Substituted {
            latency_ms: 130,
            reason: still_contract::frame_choice::SubstitutionReason::ShownFrameEvicted,
        },
        ..Behaviour::default()
    });

    assert_eq!(state, RunnerState::AllDone);
    for shot in &records.shots {
        assert!(shot.frame.is_evicted_substitution(), "снимок {}", shot.number);
        assert_eq!(shot.frame.press_to_frame_ms, Some(130.0));
        assert_eq!(shot.frame.shown_before_press_ms, None);
    }
}

#[test]
fn a_render_that_never_reports_a_choice_leaves_every_shot_unknown_without_inventing_numbers() {
    let (_world, _state, records) = run(Behaviour {
        choice: ChoicePolicy::Missing,
        ..Behaviour::default()
    });

    for shot in &records.shots {
        assert!(!shot.frame.choice_known(), "снимок {}", shot.number);
        assert_eq!(shot.frame.press_to_frame_ms, None);
    }
}
