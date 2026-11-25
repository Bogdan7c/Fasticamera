//! Тесты сервиса снимка на НАСТОЯЩЕЙ службе записи: план рендеру, итог экрану тем же номером запроса, предел
//! допуска, смена экрана, «рендер недоступен». Платформа и рендер — подделки (`still_test_support`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use camera_screen::ScreenAction;
use camera_screen::settings::RawCapture;
use camera_screen::shutter::{CaptureFailureReason, CaptureOutcome, ShotRequestId};
use still_contract::develop::DevelopFailure;
use still_contract::frame_choice::NoFrameReason;
use still_contract::plan::{FrameSelection, PrimaryFormat as PlanFormat, RawOutput};

use super::*;
use crate::camera_screen_input::ScreenInput;
use crate::photo_config::PhotoConfig;
use crate::still_test_support::{
    DraftState, FakeJpeg, RenderMode, StillRig, facts_for, readback_image, shot, wait_until,
};

fn finished(request: u64, outcome: CaptureOutcome) -> ScreenInput {
    ScreenInput::action(ScreenAction::CaptureFinished {
        request: ShotRequestId(request),
        outcome,
    })
}

fn secured(request: u64) -> ScreenInput {
    ScreenInput::action(ScreenAction::CaptureSecured {
        request: ShotRequestId(request),
    })
}

/// Рендер-игрок доводит снимок до сохранения по пути JPEG.
fn develop_jpeg(rig: &StillRig, id: StillId) {
    rig.render.emit(
        id,
        DevelopEvent::Secured {
            facts: facts_for(id),
            raw: None,
        },
    );
    rig.render.wait_readback_target(id);
    rig.render.emit(
        id,
        DevelopEvent::PrimaryReadback {
            id,
            image: crate::still_test_support::readback_image(),
        },
    );
}

#[test]
fn shutter_press_hands_the_render_a_plan_built_from_the_shot_request() {
    let rig = StillRig::start();
    let mut request = shot(4);
    request.raw = RawCapture::On;

    let outcome = rig.capture.submit(
        request.clone(),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );

    assert_eq!(outcome, SubmitOutcome::Accepted);
    let plans = rig.render.plans();
    assert_eq!(plans.len(), 1, "рендер получил ровно один Begin");
    let plan = &plans[0];
    assert_eq!(plan.id, StillId(1), "первый номер процесса");
    assert_eq!(
        plan.press.at, request.pressed_at,
        "момент нажатия — из запроса, не время кадра"
    );
    assert_eq!(plan.selection, FrameSelection::ShownAtPress);
    assert_eq!(plan.framing, request.framing);
    assert_eq!(plan.orientation, request.orientation);
    assert_eq!(plan.outputs.primary, PlanFormat::Jpeg);
    assert_eq!(plan.outputs.raw, RawOutput::Dng, "RAW-кнопка экрана включила DNG");
}

#[test]
fn the_plan_carries_exactly_the_look_recipe_the_request_was_submitted_with() {
    // Инвариант 3 и решение 8: ручки и основа идут в план как есть, на момент нажатия; DNG-флаг экрана на них не влияет.
    let rig = StillRig::start();
    let recipe = look_contract::LookRecipe {
        base: look_contract::ToneBaseModel::Filmic,
        settings: crate::look_test_support::contrast_look(0.5),
    };

    rig.capture
        .submit(shot(1), recipe.clone(), MonotonicNanos::from_nanos(0));
    rig.capture.submit(
        shot(2),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );

    let plans = rig.render.plans();
    assert_eq!(
        (plans[0].look.clone(), plans[0].tone_base),
        (recipe.settings, recipe.base)
    );
    assert_eq!(
        (plans[1].look.clone(), plans[1].tone_base),
        (look_contract::LookSettings::NEUTRAL, look_contract::ToneBaseModel::Agx),
        "второй снимок — свои значения и своя основа"
    );
}

#[test]
fn still_numbers_are_process_wide_and_independent_of_screen_request_numbers() {
    let rig = StillRig::start();
    // У экрана номера запросов идут своим порядком (здесь 9 и 2), у службы — сквозные 1, 2.
    rig.capture.submit(
        shot(9),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );
    rig.capture.submit(
        shot(2),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );
    let ids: Vec<StillId> = rig.render.plans().iter().map(|plan| plan.id).collect();
    assert_eq!(ids, vec![StillId(1), StillId(2)]);
}

#[test]
fn render_events_become_secured_then_saved_with_the_same_request_number_and_a_file() {
    let mut rig = StillRig::start();
    assert_eq!(
        rig.capture.submit(
            shot(5),
            crate::look_test_support::neutral_recipe(),
            MonotonicNanos::from_nanos(0)
        ),
        SubmitOutcome::Accepted
    );

    develop_jpeg(&rig, StillId(1));

    assert_eq!(
        rig.wait_screen_inputs(2),
        vec![secured(5), finished(5, CaptureOutcome::Saved)],
        "экран получил обе фазы в порядке и под СВОИМ номером запроса"
    );
    let drafts = rig.store.drafts();
    assert_eq!(drafts.len(), 1, "основная картинка без DNG — один файл");
    assert_eq!(drafts[0].state, DraftState::Published);
    assert!(drafts[0].name.ends_with("JPG"), "{}", drafts[0].name);
    assert!(
        drafts[0].bytes.ends_with(&FakeJpeg::BODY),
        "в файл ушло тело JPEG платформы"
    );
    let stats = rig.service.stats();
    assert_eq!((stats.accepted, stats.saved, stats.pending_now), (1, 1, 0));
}

#[test]
fn eleventh_press_with_ten_waiting_is_refused_at_once_and_the_ten_are_untouched() {
    let mut rig = StillRig::start();
    for request in 1..=10 {
        assert_eq!(
            rig.capture.submit(
                shot(request),
                crate::look_test_support::neutral_recipe(),
                MonotonicNanos::from_nanos(0)
            ),
            SubmitOutcome::Accepted
        );
    }

    let eleventh = rig.capture.submit(
        shot(11),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );

    assert_eq!(eleventh, SubmitOutcome::Rejected(SubmitRejection::Busy));
    assert_eq!(rig.render.plans().len(), 10, "рендер об 11-м не знает");
    let stats = rig.service.stats();
    assert_eq!((stats.pending_now, stats.admission_refusals), (10, 1));
    assert!(rig.screen_inputs().is_empty(), "ждущим снимкам фазы не рассылались");
    // Десять ждущих по-прежнему живые: завершим первый — он дойдёт до экрана под своим номером.
    develop_jpeg(&rig, StillId(1));
    assert_eq!(
        rig.wait_screen_inputs(2),
        vec![secured(1), finished(1, CaptureOutcome::Saved)]
    );
    // Место освободилось: 12-е нажатие принято.
    wait_until("слот допуска освободился", || {
        rig.service.stats().pending_now == 9
    });
    assert_eq!(
        rig.capture.submit(
            shot(12),
            crate::look_test_support::neutral_recipe(),
            MonotonicNanos::from_nanos(0)
        ),
        SubmitOutcome::Accepted
    );
}

#[test]
fn replacing_the_screen_drops_phases_of_the_old_one_and_counts_them() {
    let mut rig = StillRig::start();
    rig.capture.submit(
        shot(1),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );
    // Экран переустановлен до того, как снимок проявился: у нового экрана свой номер 1 — другой снимок.
    rig.reinstall_screen();

    develop_jpeg(&rig, StillId(1));
    wait_until("файл записан", || rig.service.stats().saved == 1);

    assert!(
        rig.screen_inputs().is_empty(),
        "новый экран не получил итог снимка старого"
    );
    let real = rig.backend.diagnostics().real.expect("настоящий режим");
    assert_eq!(
        real.screen.dropped_stale_screen, 2,
        "Secured и Saved старого экрана учтены"
    );
    assert_eq!(real.service.saved, 1, "файл при этом записан");
}

#[test]
fn render_gone_fails_the_still_and_frees_the_admission_slot() {
    let mut rig = StillRig::start_with(PhotoConfig::default(), RenderMode::Gone);

    let outcome = rig.capture.submit(
        shot(3),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );

    assert_eq!(
        outcome,
        SubmitOutcome::Accepted,
        "итог придёт фазой службы, не отказом исполнителю"
    );
    assert_eq!(
        rig.wait_screen_inputs(1),
        vec![finished(3, CaptureOutcome::Failed(CaptureFailureReason::Processing))]
    );
    wait_until("слот допуска освобождён", || {
        rig.service.stats().pending_now == 0
    });
    let real = rig.backend.diagnostics().real.expect("настоящий режим");
    assert_eq!(real.capture.render_refused, 1);
    assert_eq!(real.service.failed_develop, 1);
    assert!(rig.store.drafts().is_empty(), "частичных файлов нет");
}

#[test]
fn unavailable_render_also_fails_the_still_without_hanging() {
    let mut rig = StillRig::start_with(PhotoConfig::default(), RenderMode::Unavailable);
    assert_eq!(
        rig.capture.submit(
            shot(1),
            crate::look_test_support::neutral_recipe(),
            MonotonicNanos::from_nanos(0)
        ),
        SubmitOutcome::Accepted
    );
    assert_eq!(
        rig.wait_screen_inputs(1),
        vec![finished(1, CaptureOutcome::Failed(CaptureFailureReason::Processing))]
    );
    wait_until("слот допуска освобождён", || {
        rig.service.stats().pending_now == 0
    });
}

#[test]
fn not_secured_frame_reaches_the_screen_as_a_camera_failure() {
    let mut rig = StillRig::start();
    rig.capture.submit(
        shot(6),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );
    rig.render.emit(
        StillId(1),
        DevelopEvent::NotSecured {
            id: StillId(1),
            reason: NoFrameReason::RingEmpty,
        },
    );
    assert_eq!(
        rig.wait_screen_inputs(1),
        vec![finished(6, CaptureOutcome::Failed(CaptureFailureReason::Camera))],
        "без кадра `Secured` не было, экран получает сразу итог ошибки"
    );
}

#[test]
fn render_failure_after_secured_still_ends_with_exactly_one_finished() {
    let mut rig = StillRig::start();
    rig.capture.submit(
        shot(2),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );
    rig.render.emit(
        StillId(1),
        DevelopEvent::Secured {
            facts: facts_for(StillId(1)),
            raw: None,
        },
    );
    rig.render.wait_readback_target(StillId(1));
    rig.render.emit(
        StillId(1),
        DevelopEvent::Failed {
            id: StillId(1),
            failure: DevelopFailure::Gpu("тест".to_owned()),
        },
    );
    assert_eq!(
        rig.wait_screen_inputs(2),
        vec![
            secured(2),
            finished(2, CaptureOutcome::Failed(CaptureFailureReason::Processing))
        ]
    );
    // Опоздавшее событие рендера не создаёт второго итога.
    rig.render.emit(
        StillId(1),
        DevelopEvent::PrimaryReadback {
            id: StillId(1),
            image: readback_image(),
        },
    );
    wait_until(
        "опоздавшее событие учтено службой",
        || rig.service.stats().late_events == 1,
    );
    assert!(rig.screen_inputs().is_empty());
}

#[test]
fn stopped_service_refuses_the_press_and_leaves_nothing_behind() {
    let StillRig {
        service,
        capture,
        render,
        ..
    } = StillRig::start();
    assert_eq!(service.shutdown(), photo_output::service::ShutdownReport::Stopped);

    let outcome = capture.submit(
        shot(1),
        crate::look_test_support::neutral_recipe(),
        MonotonicNanos::from_nanos(0),
    );

    assert_eq!(outcome, SubmitOutcome::Rejected(SubmitRejection::ServiceUnavailable));
    assert!(render.plans().is_empty(), "рендеру о снимке не сообщали");
}

#[test]
fn journal_is_bounded_and_the_accepted_count_is_not() {
    let config = PhotoConfig {
        journal_capacity: 2,
        ..PhotoConfig::default()
    };
    let rig = StillRig::start_with(config, RenderMode::Accepting);
    for request in 1..=3 {
        rig.capture.submit(
            shot(request),
            crate::look_test_support::neutral_recipe(),
            MonotonicNanos::from_nanos(0),
        );
    }
    let journal: Vec<ShotRequestId> = rig.backend.journal().iter().map(|entry| entry.request).collect();
    assert_eq!(
        journal,
        vec![ShotRequestId(2), ShotRequestId(3)],
        "самый старый вытеснен"
    );
    assert_eq!(rig.backend.accepted_total(), 3, "счёт принятых не ограничен журналом");
}
