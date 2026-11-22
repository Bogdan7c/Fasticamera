//! Исполнитель шагов `capture` в ручном времени: порядок шагов, RAW включается и возвращается, времена фаз, отказы и
//! учёт серии. Мир (`capture_test_support`) отвечает как экран и служба записи.

use std::time::Duration;

use camera_screen::settings::{FlashMode, RawCapture};

use super::super::capture_flash_record::{CameraFlashRecord, FlashShotRecord};
use super::super::capture_frame_timing::FrameChoiceView;
use super::super::capture_plan::planned_steps;
use super::super::capture_report::ShotOutcome;
use super::super::capture_test_support::{Behaviour, ChoicePolicy, World, drive, fast_config};
use super::*;
use crate::still_capture_service::FlashModeName;

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
fn a_healthy_run_takes_one_heif_then_heif_with_dng_then_both_flash_steps_then_a_burst_and_leaves_raw_and_flash_off() {
    let (world, state, records) = run(Behaviour::default());

    assert_eq!(state, RunnerState::AllDone);
    assert_eq!(
        records.steps.iter().map(|step| step.name).collect::<Vec<_>>(),
        vec![
            "single_heif",
            "single_heif_dng",
            "single_flash_on",
            "single_flash_auto",
            "burst"
        ]
    );
    assert!(
        records.steps.iter().all(|step| step.failure.is_none()),
        "{:?}",
        records.steps
    );
    assert!(records.unfinished.is_empty());
    // 1 + 1 + 1 + 1 + 10 снимков; файлов 1 + 2 + 1 + 1 + 10.
    assert_eq!(records.shots.len(), 14);
    assert_eq!(saved_addresses(&records).len(), 15);
    // RAW: включён ровно на втором шаге и возвращён к следующему (два нажатия значка).
    assert_eq!(world.raw_taps, 2);
    assert_eq!(world.raw, RawCapture::Off);
    // Вспышка идёт по кругу Off → Auto → On → Off: до On два нажатия, до Auto (с On) два, до Off (с Auto) два.
    assert_eq!(world.flash_taps, 6);
    assert_eq!(world.flash, FlashMode::Off, "серия снимала с выключенной вспышкой");
    let requested_flash: Vec<_> = records.shots.iter().map(|shot| shot.flash_requested).collect();
    assert_eq!(
        requested_flash[..4],
        [
            FlashModeName::Off,
            FlashModeName::Off,
            FlashModeName::On,
            FlashModeName::Auto
        ]
    );
    assert!(
        requested_flash[4..].iter().all(|mode| *mode == FlashModeName::Off),
        "{requested_flash:?}"
    );
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
    for shot in records.shots.iter().filter(|shot| shot.step != "single_flash_on") {
        assert_eq!(
            shot.frame.frame_choice,
            FrameChoiceView::Exact,
            "снимок {}",
            shot.number
        );
        assert_eq!(shot.frame.press_to_frame_ms, Some(80.0), "снимок {}", shot.number);
        assert_eq!(shot.frame.shown_before_press_ms, Some(10.0), "снимок {}", shot.number);
    }
    // Шаг On: кадр разового запроса со вспышкой, задержка затвора к нему не применима, камера сообщила итог.
    let flash_shot = &records.shots[2];
    assert_eq!(flash_shot.frame.frame_choice, FrameChoiceView::FlashFrame);
    assert_eq!(flash_shot.frame.press_to_frame_ms, None);
    assert_eq!(flash_shot.frame.shown_before_press_ms, None);
    let flash = flash_shot.flash.as_ref().expect("запись вспышки");
    assert_eq!(flash.route_uses_flash, Some(true));
    assert_eq!(flash.fired(), Some(true));
    // Шаг Auto без совета вспыхнуть: показанный кадр, вспышки в записи нет (сервис решил «нет», камеры не просили).
    let auto_shot = &records.shots[3];
    assert_eq!(auto_shot.frame.frame_choice, FrameChoiceView::Exact);
    assert_eq!(
        auto_shot.flash.as_ref().and_then(|flash| flash.route_uses_flash),
        Some(false)
    );
    assert_eq!(auto_shot.flash.as_ref().and_then(FlashShotRecord::fired), None);
}

#[test]
fn auto_flash_takes_the_flash_frame_when_the_camera_advises_a_flash() {
    let (_world, state, records) = run(Behaviour {
        auto_advises_flash: true,
        ..Behaviour::default()
    });

    assert_eq!(state, RunnerState::AllDone, "{:?}", records.failure);
    let auto_shot = &records.shots[3];
    assert_eq!(auto_shot.step, "single_flash_auto");
    assert_eq!(auto_shot.frame.frame_choice, FrameChoiceView::FlashFrame);
    assert_eq!(auto_shot.flash.as_ref().and_then(FlashShotRecord::fired), Some(true));
}

#[test]
fn a_flash_icon_that_changes_nothing_fails_the_step_naming_both_settings_and_taps_once() {
    let (world, state, records) = run(Behaviour {
        flash_toggle_works: false,
        ..Behaviour::default()
    });

    let RunnerState::Failed(reason) = state else {
        panic!("{state:?}");
    };
    assert!(reason.contains("вспышка стоит Off, нужна On"), "{reason}");
    assert_eq!(world.flash_taps, 1, "значок нажат один раз, а не молотится");
    assert_eq!(records.steps.last().map(|step| step.name), Some("single_flash_on"));
    assert_eq!(records.unfinished, vec!["single_flash_auto", "burst"]);
}

#[test]
fn a_camera_that_fails_the_flash_still_stops_the_run_at_the_flash_step() {
    let (_world, state, records) = run(Behaviour {
        camera_fires_flash: false,
        ..Behaviour::default()
    });

    let RunnerState::Failed(reason) = state else {
        panic!("{state:?}");
    };
    assert!(reason.contains("снимок 3 сорвался"), "{reason}");
    let flash = records.shots[2].flash.as_ref().expect("запись вспышки");
    assert!(
        matches!(flash.camera, Some(CameraFlashRecord::Failed { .. })),
        "срыв камеры назван в записи: {flash:?}"
    );
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
    // Мир отвергает как «занят» нажатия 5 и 6 от начала прогона (это 1-е и 2-е нажатие серии).
    let (_world, state, records) = run(Behaviour {
        service_rejects_press: [5, 6].into_iter().collect(),
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
    assert_eq!(
        records.unfinished,
        vec!["single_heif_dng", "single_flash_on", "single_flash_auto", "burst"]
    );
    assert!(matches!(records.shots[0].outcome, ShotOutcome::Failed { .. }));
}

#[test]
fn a_failed_burst_shot_is_counted_as_failed_and_not_hidden() {
    // Первый снимок серии — запрос 5 (единичные были 1–4).
    let (_world, state, records) = run(Behaviour {
        failing_requests: [5].into_iter().collect(),
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
    assert!(reason.contains("RAW стоит Off, нужно On"), "{reason}");
    assert_eq!(world.raw_taps, 1, "значок нажат один раз, а не молотится");
    assert_eq!(
        records.steps.iter().map(|step| step.name).collect::<Vec<_>>(),
        vec!["single_heif", "single_heif_dng"]
    );
    assert_eq!(
        records.unfinished,
        vec!["single_flash_on", "single_flash_auto", "burst"]
    );
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
    // Единичные снимки (запросы 1–4) сохраняются, а у серии итогов нет.
    let mut world = World::new(Behaviour {
        no_result_after_request: Some(4),
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
    // Кадр шага On — кадр вспышки (его выбор не «замена показанного»), остальные — замены.
    for shot in records.shots.iter().filter(|shot| shot.step != "single_flash_on") {
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

    // Кадр шага On найден по метке запроса: журнал выбора для него не нужен.
    for shot in records.shots.iter().filter(|shot| shot.step != "single_flash_on") {
        assert!(!shot.frame.choice_known(), "снимок {}", shot.number);
        assert_eq!(shot.frame.press_to_frame_ms, None);
    }
}

#[test]
fn the_press_to_thumbnail_time_lands_only_in_the_shots_whose_picture_was_actually_drawn() {
    use camera_screen::shutter::ShotRequestId;
    use camera_screen::thumbnail::PictureTiming;
    use shell_contract::clock::MonotonicNanos;

    let mut world = World::new(Behaviour::default());
    let mut runner = CaptureRunner::new(fast_config(), planned_steps(), world.now());
    let state = drive(&mut runner, &mut world, TICK, LIMIT);
    assert_eq!(state, RunnerState::AllDone);
    let timing = |request: u64, shown_ms: i64| PictureTiming {
        shot: ShotRequestId(request),
        pressed_at: MonotonicNanos::from_nanos(1_000_000_000),
        shown_at: Some(MonotonicNanos::from_nanos((1_000 + shown_ms) * 1_000_000)),
    };
    // Экран показал картинки снимков 1 и 3; снимок 2 вытеснен следующим и не рисовался; 4-й ещё не показан.
    runner.absorb_picture_timing(Some(timing(1, 310)));
    runner.absorb_picture_timing(Some(timing(3, 455)));
    runner.absorb_picture_timing(Some(PictureTiming {
        shown_at: None,
        ..timing(4, 0)
    }));

    let records = runner.into_records();

    let by_request: Vec<(u64, Option<u64>)> = records
        .shots
        .iter()
        .take(4)
        .map(|shot| (shot.request, shot.press_to_thumbnail_ms))
        .collect();
    assert_eq!(by_request, [(1, Some(310)), (2, None), (3, Some(455)), (4, None)]);
}
