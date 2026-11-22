//! Тесты снимка и звука затвора в исполнителе эффектов: запрос передаётся целиком (с моментом нажатия), звук идёт
//! через свой сервис, отказ платформы — счётчик, а не ошибка экрана.

use super::*;

#[test]
fn the_whole_shot_request_with_its_press_moment_reaches_the_capture_service() {
    let (mut executor, rig) = build_executor(Behavior::default());

    executor.execute(vec![Effect::Capture(shot(3)), Effect::PlayShutterSound]);

    let received = rig.capture.requests.lock().expect("замок").clone();
    assert_eq!(received.len(), 1);
    assert_eq!(received[0].pressed_at, MonotonicNanos::from_nanos(3_000_000));
    assert_eq!(received[0].orientation, DeviceOrientation::LandscapeLeft);
    assert_eq!(received[0].primary_format, PrimaryFormat::Jpeg);
    assert_eq!(received[0].framing, shot(3).framing);
}

#[test]
fn shutter_sound_effect_plays_through_the_service_and_counts_each_outcome() {
    let (mut executor, rig) = build_executor(Behavior::default());
    let feedback = executor.execute(vec![Effect::PlayShutterSound]);
    assert!(feedback.is_empty(), "звук экрану ничего не возвращает");
    assert_eq!(*rig.shutter_sound.calls.lock().expect("замок"), 1);
    assert_eq!(executor.counters().shutter_sound.queued, 1);
    assert!(
        rig.capture.requests.lock().expect("замок").is_empty(),
        "звук снимок не просит"
    );

    for (outcome, check) in [
        (
            CommandOutcome::NotAttached,
            (|c: ShutterSoundCounters| c.not_attached) as fn(ShutterSoundCounters) -> u64,
        ),
        (CommandOutcome::Backpressure, |c| c.backpressure),
    ] {
        let (mut executor, rig) = build_executor(Behavior {
            shutter_sound: outcome,
            ..Behavior::default()
        });
        let feedback = executor.execute(vec![Effect::PlayShutterSound, Effect::PlayShutterSound]);
        let counters = executor.counters().shutter_sound;
        assert_eq!(check(counters), 2, "{outcome:?}: {counters:?}");
        assert_eq!(counters.queued, 0, "{outcome:?}");
        assert!(feedback.is_empty(), "отказ звука — не ошибка экрана: {outcome:?}");
        assert_eq!(*rig.shutter_sound.calls.lock().expect("замок"), 2);
    }
}

/// Сквозной случай решения 140: 11-е нажатие при десяти ждущих снимках отвечает экрану `CaptureFinished(Failed)`
/// СРАЗУ (в том же вызове исполнителя), а десять уже принятых не тронуты.
#[test]
fn eleventh_press_with_ten_saving_gets_an_immediate_failed_finish_from_the_real_service() {
    use crate::still_test_support::StillRig;

    let rig = StillRig::start();
    let (mut executor, _stub_rig) = build_executor_with_capture(Behavior::default(), Some(rig.capture.clone()));

    let feedback_for_ten: Vec<ScreenAction> = (1..=10)
        .flat_map(|request| executor.execute(vec![Effect::Capture(shot(request))]))
        .collect();
    assert!(
        feedback_for_ten.is_empty(),
        "принятые снимки ответа сразу не дают: {feedback_for_ten:?}"
    );

    let feedback = executor.execute(vec![Effect::Capture(shot(11))]);

    assert_eq!(
        feedback,
        vec![ScreenAction::CaptureFinished {
            request: ShotRequestId(11),
            outcome: CaptureOutcome::Failed(CaptureFailureReason::Processing),
        }]
    );
    assert_eq!(executor.counters().capture.accepted, 10);
    assert_eq!(executor.counters().capture.rejected_busy, 1);
    assert_eq!(rig.service.stats().pending_now, 10, "десять ждущих на месте");
}
