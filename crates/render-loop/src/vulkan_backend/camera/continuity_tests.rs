//! Тесты машины непрерывности: каждый переход состояний и счёт по факту записи (без GPU).

use camera_contract::ids::LensFacing;
use camera_contract::zoom::{PreviewMirror, ZoomRatio};
use shell_contract::clock::MonotonicNanos;
use ui_draw::frame_source::BackdropContinuity;

use super::*;
use crate::config::ContinuityConfig;
use crate::viewfinder::framing::WantedZoom;
use crate::vulkan_backend::camera::white_balance::AwbStream;

const A: AwbStream = AwbStream(1);
const B: AwbStream = AwbStream(2);

fn ms(value: i64) -> MonotonicNanos {
    MonotonicNanos::from_nanos(value * NANOS_PER_MILLI)
}

fn ratio(value: f32) -> ZoomRatio {
    ZoomRatio::new(value).expect("коэффициент")
}

fn picture(stream: AwbStream, shown_ratio: f32, facing: Option<LensFacing>) -> PictureMeta {
    PictureMeta {
        stream,
        shown_ratio: ratio(shown_ratio),
        mirror: PreviewMirror::None,
        facing,
    }
}

fn config() -> ContinuityConfig {
    ContinuityConfig {
        max_hold_ms: 100,
        max_hold_without_stream_ms: 300,
        ..ContinuityConfig::default()
    }
}

fn frame(stream: AwbStream, color_known: bool, exposure: ExposureReadiness) -> StreamEvent {
    StreamEvent::Processed(ProcessedFrame {
        stream,
        color_known,
        exposure,
    })
}

/// Один кадр: решение, запись «как задумано» (или заданным итогом), фиксация.
fn step(
    machine: &mut ContinuityMachine,
    now: i64,
    event: StreamEvent,
    composed: Option<Composed>,
    new_picture: Option<PictureMeta>,
) -> (FramePlan, CommitReport) {
    let plan = machine.plan(ms(now), event, None);
    let composed = composed.unwrap_or(match plan.screen {
        Screen::Live => Composed::Live,
        Screen::Held => Composed::Held,
        Screen::Black => Composed::Black,
    });
    let report = machine.commit(ms(now), event, &plan, &FrameOutcome { composed, new_picture });
    (plan, report)
}

/// Машина с показанной картинкой потока A (1×, задняя камера).
fn machine_showing_a() -> ContinuityMachine {
    let mut machine = ContinuityMachine::new(config());
    let (plan, report) = step(
        &mut machine,
        0,
        frame(A, true, ExposureReadiness::Ready),
        None,
        Some(picture(A, 1.0, Some(LensFacing::Back))),
    );
    assert_eq!(plan.screen, Screen::Live);
    assert_eq!(report.label, FrameLabel::Live);
    machine
}

#[test]
fn cold_start_is_black_until_the_first_ready_frame_and_that_black_is_allowed() {
    let mut machine = ContinuityMachine::new(config());
    for now in [0, 16] {
        let (plan, report) = step(&mut machine, now, StreamEvent::Quiet, None, None);
        assert_eq!(plan.screen, Screen::Black);
        assert_eq!(report.label, FrameLabel::Black(BlackKind::NoPicture));
        assert_eq!(plan.backdrop(), BackdropContinuity::NoCameraFrame);
    }
    // Кадр есть, но цвет неизвестен (холодный старт AWB): всё ещё чёрный до первой картинки.
    let (plan, report) = step(
        &mut machine,
        32,
        frame(A, false, ExposureReadiness::Waiting),
        None,
        None,
    );
    assert_eq!(plan.screen, Screen::Black);
    assert_eq!(report.label, FrameLabel::Black(BlackKind::NoPicture));
    assert!(!machine.is_holding(), "удерживать нечего");
    let (plan, report) = step(
        &mut machine,
        48,
        frame(A, true, ExposureReadiness::Waiting),
        None,
        Some(picture(A, 1.0, None)),
    );
    assert_eq!((plan.screen, report.label), (Screen::Live, FrameLabel::Live));
    assert_eq!(machine.shown_picture(), Some(picture(A, 1.0, None)));
}

#[test]
fn quiet_frames_with_a_picture_stay_live_and_planning_does_not_change_the_state() {
    let mut machine = machine_showing_a();
    let before = machine.clone();
    let plan = machine.plan(ms(16), StreamEvent::Inactive, None);
    assert_eq!(plan.screen, Screen::Held);
    assert_eq!(
        (machine.is_holding(), machine.shown_picture()),
        (before.is_holding(), before.shown_picture()),
        "plan ничего не меняет"
    );
    let (plan, report) = step(&mut machine, 16, StreamEvent::Quiet, None, None);
    assert_eq!(
        (plan.screen, report.label, report.event),
        (Screen::Live, FrameLabel::Live, None)
    );
    assert_eq!(plan.backdrop(), BackdropContinuity::Live);
}

#[test]
fn stream_inactive_starts_the_hold_and_the_held_backdrop_keeps_the_first_held_time() {
    let mut machine = machine_showing_a();
    let (plan, report) = step(&mut machine, 16, StreamEvent::Inactive, None, None);
    assert_eq!(plan.screen, Screen::Held);
    assert_eq!(plan.begins_hold, Some(picture(A, 1.0, Some(LensFacing::Back))));
    assert_eq!(report.event, Some(HoldEvent::Started(HoldReason::StreamInactive)));
    assert_eq!(report.label, FrameLabel::Transition(TransitionKind::HeldLastFrame));
    assert_eq!(plan.backdrop(), BackdropContinuity::HeldLastFrame { since: ms(16) });
    assert!(machine.is_holding());
    assert_eq!(machine.shown_picture(), None, "живая картинка отцеплена");
    // Дальше кадры держатся, начало удержания не двигается, begins_hold больше нет.
    for now in [32, 48, 64] {
        let (plan, report) = step(&mut machine, now, StreamEvent::Inactive, None, None);
        assert_eq!(plan.screen, Screen::Held);
        assert_eq!(plan.begins_hold, None);
        assert_eq!(plan.backdrop(), BackdropContinuity::HeldLastFrame { since: ms(16) });
        assert_eq!(report.event, None);
    }
}

#[test]
fn hidden_and_dark_frames_of_the_new_stream_do_not_show_and_the_first_ready_one_reveals() {
    let mut machine = machine_showing_a();
    step(&mut machine, 16, StreamEvent::Inactive, None, None);
    // Поток B: цвет неизвестен (скрытие AWB), потом цвет есть, но яркость ждёт AE.
    for (now, color_known, exposure) in [
        (32, false, ExposureReadiness::Waiting),
        (48, false, ExposureReadiness::Waiting),
        (64, true, ExposureReadiness::Waiting),
    ] {
        let (plan, report) = step(
            &mut machine,
            now,
            frame(B, color_known, exposure),
            Some(Composed::Held),
            None,
        );
        assert_eq!(plan.screen, Screen::Held, "кадр {now} ещё нельзя показывать");
        assert_eq!(report.label, FrameLabel::Transition(TransitionKind::HeldLastFrame));
        assert_eq!(plan.ends_hold, None);
    }
    let new_picture = picture(B, 1.0, Some(LensFacing::Back));
    let (plan, report) = step(
        &mut machine,
        80,
        frame(B, true, ExposureReadiness::Ready),
        None,
        Some(new_picture),
    );
    assert_eq!((plan.screen, plan.ends_hold), (Screen::Live, Some(HoldEnd::Condition)));
    assert_eq!(
        report.event,
        Some(HoldEvent::Ended {
            end: HoldEnd::Condition,
            duration_ms: 64
        }),
        "от первого удержанного кадра (16) до первого живого (80)"
    );
    assert_eq!(report.label, FrameLabel::Live);
    assert!(!machine.is_holding());
    assert_eq!(machine.shown_picture(), Some(new_picture));
    assert_eq!(plan.backdrop(), BackdropContinuity::Live);
}

#[test]
fn a_new_stream_that_never_gets_plausible_is_revealed_by_timeout_not_frozen_forever() {
    let mut machine = machine_showing_a();
    step(&mut machine, 0, StreamEvent::Inactive, None, None);
    let waiting = frame(B, true, ExposureReadiness::Waiting);
    let (plan, _) = step(&mut machine, 99, waiting, None, None);
    assert_eq!(plan.screen, Screen::Held, "до max_hold_ms 100 держим");
    let (plan, report) = step(&mut machine, 100, waiting, None, Some(picture(B, 1.0, None)));
    assert_eq!((plan.screen, plan.ends_hold), (Screen::Live, Some(HoldEnd::Timeout)));
    assert_eq!(
        report.event,
        Some(HoldEvent::Ended {
            end: HoldEnd::Timeout,
            duration_ms: 100
        })
    );
}

#[test]
fn timeout_without_a_processed_frame_keeps_holding_until_the_no_stream_limit_then_honest_black() {
    let mut machine = machine_showing_a();
    step(&mut machine, 0, StreamEvent::Inactive, None, None);
    // max_hold_ms прошёл, но обработанного кадра нет: показывать нечего, держим.
    for now in [150, 250, 299] {
        let (plan, _) = step(&mut machine, now, StreamEvent::Quiet, None, None);
        assert_eq!(plan.screen, Screen::Held, "{now} мс: потока нет, но срок не вышел");
    }
    let (plan, report) = step(&mut machine, 300, StreamEvent::Inactive, None, None);
    assert_eq!((plan.screen, plan.ends_hold), (Screen::Black, Some(HoldEnd::NoStream)));
    assert_eq!(plan.backdrop(), BackdropContinuity::NoCameraFrame);
    assert_eq!(report.label, FrameLabel::Black(BlackKind::CameraUnavailable));
    assert_eq!(
        report.event,
        Some(HoldEvent::Ended {
            end: HoldEnd::NoStream,
            duration_ms: 300
        })
    );
    assert!(!machine.is_holding());
    // Дальше — чёрный «камера недоступна», пока не появится новая картинка.
    let (_, report) = step(&mut machine, 316, StreamEvent::Quiet, None, None);
    assert_eq!(report.label, FrameLabel::Black(BlackKind::CameraUnavailable));
    let (_, report) = step(
        &mut machine,
        332,
        frame(B, true, ExposureReadiness::Ready),
        None,
        Some(picture(B, 1.0, None)),
    );
    assert_eq!(report.label, FrameLabel::Live);
    let (_, report) = step(&mut machine, 348, StreamEvent::Quiet, Some(Composed::Black), None);
    assert_eq!(
        report.label,
        FrameLabel::Black(BlackKind::PictureExpected),
        "после возвращения картинки чёрный снова недопустим"
    );
}

#[test]
fn activity_of_the_new_stream_postpones_the_no_stream_limit() {
    let mut machine = machine_showing_a();
    step(&mut machine, 0, StreamEvent::Inactive, None, None);
    // Кадр нового потока в 90 мс (цвет неизвестен, срок max_hold_ms 100 ещё не вышел): срок «потока нет»
    // отсчитывается от него, а не от начала удержания.
    let (plan, _) = step(
        &mut machine,
        90,
        frame(B, false, ExposureReadiness::Waiting),
        None,
        None,
    );
    assert_eq!(plan.screen, Screen::Held);
    let (plan, _) = step(&mut machine, 380, StreamEvent::Quiet, None, None);
    assert_eq!(
        plan.screen,
        Screen::Held,
        "с 90 мс прошло 290 < 300 (от начала было бы 380 ≥ 300)"
    );
    let (plan, _) = step(&mut machine, 400, StreamEvent::Quiet, None, None);
    assert_eq!(plan.screen, Screen::Black);
}

#[test]
fn direct_switch_to_a_ready_stream_needs_no_hold_and_a_not_ready_one_holds_in_the_same_tick() {
    // Готовый сразу новый поток: показывать его можно в этот же кадр, удерживать нечего.
    let mut machine = machine_showing_a();
    let b_picture = picture(B, 0.59, Some(LensFacing::Back));
    let (plan, report) = step(
        &mut machine,
        16,
        frame(B, true, ExposureReadiness::Ready),
        None,
        Some(b_picture),
    );
    assert_eq!(
        (plan.screen, plan.begins_hold, report.event),
        (Screen::Live, None, None)
    );
    assert_eq!(machine.shown_picture(), Some(b_picture));

    // Не готовый: первый же кадр B начинает удержание ДО его записи (картинка A отцепляется до ISP).
    let mut machine = machine_showing_a();
    let (plan, report) = step(&mut machine, 16, frame(B, true, ExposureReadiness::Waiting), None, None);
    assert_eq!(plan.screen, Screen::Held);
    assert_eq!(plan.begins_hold, Some(picture(A, 1.0, Some(LensFacing::Back))));
    assert_eq!(report.event, Some(HoldEvent::Started(HoldReason::NewStreamNotReady)));
    assert_eq!(plan.backdrop(), BackdropContinuity::HeldLastFrame { since: ms(16) });
}

#[test]
fn a_planned_reveal_that_did_not_reach_the_screen_keeps_the_hold_and_black_while_holding_is_flagged() {
    let mut machine = machine_showing_a();
    step(&mut machine, 0, StreamEvent::Inactive, None, None);
    // Reveal задуман, но ISP отказал, и на экран легло удержанное: удержание продолжается, события нет.
    let ready = frame(B, true, ExposureReadiness::Ready);
    let (plan, report) = step(&mut machine, 16, ready, Some(Composed::Held), None);
    assert_eq!(plan.ends_hold, Some(HoldEnd::Condition));
    assert_eq!(report.event, None);
    assert!(machine.is_holding());
    // Чёрный во время удержания — недопустим и помечен как таковой.
    let (_, report) = step(&mut machine, 32, StreamEvent::Quiet, Some(Composed::Black), None);
    assert_eq!(report.label, FrameLabel::Black(BlackKind::PictureExpected));
    // Честный чёрный, который не удалось записать чёрным, удержание тоже не заканчивает.
    let mut lone = machine_showing_a();
    step(&mut lone, 0, StreamEvent::Inactive, None, None);
    let (plan, report) = step(&mut lone, 300, StreamEvent::Quiet, Some(Composed::Held), None);
    assert_eq!(plan.ends_hold, Some(HoldEnd::NoStream));
    assert_eq!(report.event, None);
    assert!(lone.is_holding());
}

fn wanted(facing: LensFacing, value: f32) -> Option<WantedZoom> {
    Some(WantedZoom {
        facing,
        ratio: ratio(value),
    })
}

#[test]
fn held_picture_is_magnified_only_upwards_and_only_within_the_same_known_facing() {
    let held = picture(A, 1.0, Some(LensFacing::Back));
    assert_eq!(held_magnification(&held, wanted(LensFacing::Back, 1.4)), 1.4);
    assert_eq!(
        held_magnification(&held, wanted(LensFacing::Back, 0.6)),
        1.0,
        "меньше 1 не бывает: чёрные поля"
    );
    assert_eq!(held_magnification(&held, None), 1.0, "зума экрана нет");
    assert_eq!(
        held_magnification(&held, wanted(LensFacing::Front, 3.0)),
        1.0,
        "удержан задний кадр, экран на фронтальной шкале: шкалы несравнимы"
    );
    let front_held = picture(A, 1.0, Some(LensFacing::Front));
    assert_eq!(
        held_magnification(&front_held, wanted(LensFacing::Back, 2.0)),
        1.0,
        "удержан фронтальный кадр, рамка задняя 2×: увеличивать нельзя"
    );
    let unknown_facing = picture(A, 1.0, None);
    assert_eq!(
        held_magnification(&unknown_facing, wanted(LensFacing::Back, 2.0)),
        1.0,
        "направление картинки неизвестно: не увеличиваем"
    );
    let ultra_wide = picture(A, 0.59, Some(LensFacing::Back));
    assert!((held_magnification(&ultra_wide, wanted(LensFacing::Back, 1.0)) - 1.0 / 0.59).abs() < 1e-5);

    // В плане: удержанный кадр масштабируется за пальцем, живой — нет.
    let mut machine = machine_showing_a();
    let back = |value| wanted(LensFacing::Back, value);
    let plan = machine.plan(ms(16), StreamEvent::Inactive, back(1.8));
    assert_eq!(plan.magnification, 1.8);
    machine.commit(
        ms(16),
        StreamEvent::Inactive,
        &plan,
        &FrameOutcome {
            composed: Composed::Held,
            new_picture: None,
        },
    );
    let plan = machine.plan(ms(32), StreamEvent::Quiet, back(2.5));
    assert_eq!(plan.magnification, 2.5);
    let flipped = machine.plan(ms(32), StreamEvent::Quiet, wanted(LensFacing::Front, 2.5));
    assert_eq!(
        flipped.magnification, 1.0,
        "переворот во время удержания: без увеличения"
    );
    let live = ContinuityMachine::new(config()).plan(ms(0), StreamEvent::Quiet, back(2.5));
    assert_eq!(live.magnification, 1.0);
}

#[test]
fn arriving_but_unpresentable_frames_count_as_stream_activity_and_never_end_in_camera_unavailable() {
    let mut machine = machine_showing_a();
    step(&mut machine, 0, StreamEvent::Inactive, None, None);
    // Поток B стримит, но ни один кадр не показать: приходы раз в 50 мс, до границы max_hold_ms (100).
    for now in [50, 99] {
        let (plan, _) = step(&mut machine, now, StreamEvent::Unpresentable, None, None);
        assert_eq!(plan.screen, Screen::Held, "{now} мс: срок max_hold_ms ещё не вышел");
    }
    // Тихий тик сразу после прихода: поток жив (приход в 99), «потока нет» (300 мс) не наступает.
    let (plan, _) = step(&mut machine, 120, StreamEvent::Quiet, None, None);
    assert_ne!(plan.ends_hold, Some(HoldEnd::NoStream));
    assert_eq!(
        (plan.screen, plan.ends_hold),
        (Screen::Black, Some(HoldEnd::NoPresentableFrame)),
        "камера стримит, а показать нечего дольше max_hold_ms: честный чёрный, не вечная заморозка"
    );
}

#[test]
fn persistent_unpresentable_stream_ends_the_hold_honestly_with_its_own_label_and_recovers() {
    let mut machine = machine_showing_a();
    step(&mut machine, 0, StreamEvent::Inactive, None, None);
    let (plan, report) = step(&mut machine, 100, StreamEvent::Unpresentable, None, None);
    assert_eq!(
        (plan.screen, plan.ends_hold),
        (Screen::Black, Some(HoldEnd::NoPresentableFrame))
    );
    assert_eq!(report.label, FrameLabel::Black(BlackKind::PictureUnavailable));
    assert_eq!(
        report.event,
        Some(HoldEvent::Ended {
            end: HoldEnd::NoPresentableFrame,
            duration_ms: 100
        })
    );
    assert_eq!(plan.backdrop(), BackdropContinuity::NoCameraFrame);
    assert!(!machine.is_holding());
    // Дальше чёрный сохраняет причину, пока не появится картинка.
    let (_, report) = step(&mut machine, 116, StreamEvent::Unpresentable, None, None);
    assert_eq!(report.label, FrameLabel::Black(BlackKind::PictureUnavailable));
    let (_, report) = step(
        &mut machine,
        132,
        frame(B, true, ExposureReadiness::Ready),
        None,
        Some(picture(B, 1.0, None)),
    );
    assert_eq!(report.label, FrameLabel::Live);
}

#[test]
fn a_rare_failure_between_processed_frames_does_not_cut_the_hold() {
    let mut machine = machine_showing_a();
    step(&mut machine, 0, StreamEvent::Inactive, None, None);
    step(&mut machine, 90, frame(B, true, ExposureReadiness::Waiting), None, None);
    // Приход без картинки через 20 мс после обработанного кадра: от обработанного прошло меньше max_hold_ms.
    let (plan, _) = step(&mut machine, 110, StreamEvent::Unpresentable, None, None);
    assert_eq!(plan.screen, Screen::Held);
    // Следующий обработанный не готовый кадр после срока раскрывается по таймауту, а не чёрным.
    let (plan, _) = step(
        &mut machine,
        126,
        frame(B, true, ExposureReadiness::Waiting),
        None,
        Some(picture(B, 1.0, None)),
    );
    assert_eq!((plan.screen, plan.ends_hold), (Screen::Live, Some(HoldEnd::Timeout)));
}
