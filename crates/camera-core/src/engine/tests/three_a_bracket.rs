//! Серия с брекетингом: отправка, сбор по индексу, таймаут, отказы и потеря сессии.

use std::time::Duration;

use camera_contract::capabilities::RequestCapability;
use camera_contract::control::{ExposureTime, Iso};
use camera_contract::three_a::{BracketExposure, BurstId, ExposureBracket, RequestOrigin};

use super::three_a_support::*;
use super::*;
use crate::backend::{BackendFailure, RequestFailure};
use crate::command::{InvalidStateReason, UnsupportedCommand, UnsupportedThreeA};
use crate::request_controls::ExposurePlan;
use crate::three_a_status::{BracketFailure, BracketFrameResult, BracketReport};

/// Серия из кадров с выдержками в миллисекундах и ISO 100.
fn bracket_of(exposures_ms: &[u64]) -> ExposureBracket {
    let frames = exposures_ms
        .iter()
        .map(|ms| BracketExposure {
            exposure_time: ExposureTime::new(Duration::from_millis(*ms)).expect("выдержка"),
            iso: Iso::new(100).expect("ISO"),
        })
        .collect();
    ExposureBracket::new(frames, 32).expect("серия собрана")
}

fn capture(rig: &mut Rig, exposures_ms: &[u64]) -> BurstId {
    assert_eq!(
        rig.send(CameraCommand::CaptureBracket {
            bracket: bracket_of(exposures_ms)
        }),
        CommandOutcome::Accepted
    );
    BurstId(rig.last_command_id().0)
}

fn frame_exposures_ms(frames: &[BracketFrameResult]) -> Vec<u128> {
    frames
        .iter()
        .map(|frame| frame.exposure_time.expect("экспозиция в результате").as_millis())
        .collect()
}

#[test]
fn bracket_is_submitted_over_the_running_stream_and_reported_in_index_order() {
    let mut rig = streaming_rig();
    let burst = capture(&mut rig, &[1, 4, 16]);

    let calls = rig.fake.take_calls();
    assert_eq!(call_names(&calls), vec!["submit_bracket"], "поток не перестраивается");
    let BackendCall::SubmitBracket {
        burst: submitted,
        bracket,
        base,
        ..
    } = &calls[0]
    else {
        panic!("ожидалась отправка серии");
    };
    assert_eq!(*submitted, burst);
    assert_eq!(bracket.frames().len(), 3);
    assert!(
        matches!(base.exposure, ExposurePlan::VendorAuto { .. }),
        "база — текущий план потока"
    );
    assert_eq!(rig.engine.status().three_a.counters.brackets_submitted, 1);

    // Результаты приходят вразнобой — отчёт собирается по индексу кадра.
    rig.fresh_events();
    rig.fake.complete_bracket_frames(burst, &[2, 0]);
    rig.pump();
    assert!(bracket_reports(&rig.fresh_events()).is_empty(), "серия ещё не полная");
    rig.fake.complete_bracket_frames(burst, &[1]);
    rig.pump();

    let reports = bracket_reports(&rig.fresh_events());
    let [
        BracketReport::Captured {
            burst: reported,
            frames,
        },
    ] = reports.as_slice()
    else {
        panic!("ожидался один успешный отчёт, а пришло {reports:?}");
    };
    assert_eq!(*reported, burst);
    assert_eq!(
        frames.iter().map(|frame| frame.index).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(
        frame_exposures_ms(frames),
        vec![1, 4, 16],
        "каждый кадр вернул свою экспозицию"
    );
    let counters = rig.engine.status().three_a.counters;
    assert_eq!((counters.brackets_completed, counters.bracket_failures), (1, 0));
    assert_eq!(rig.engine.next_deadline(), None, "таймер серии снят");
}

#[test]
fn a_second_bracket_is_refused_while_one_is_running_and_allowed_after_it_finishes() {
    let mut rig = streaming_rig();
    let burst = capture(&mut rig, &[1, 2]);
    rig.fake.take_calls();
    assert_eq!(
        rig.send(CameraCommand::CaptureBracket {
            bracket: bracket_of(&[3])
        }),
        CommandOutcome::InvalidState(InvalidStateReason::BracketInProgress)
    );
    assert!(rig.fake.take_calls().is_empty(), "вторая серия до бэкенда не дошла");
    rig.fake.complete_bracket_frames(burst, &[0, 1]);
    rig.pump();
    assert_eq!(
        rig.send(CameraCommand::CaptureBracket {
            bracket: bracket_of(&[3])
        }),
        CommandOutcome::Accepted
    );
}

#[test]
fn bracket_longer_than_the_config_limit_is_unsupported() {
    let mut rig = streaming_rig();
    let limit = CameraEngineConfig::default().three_a.max_bracket_frames;
    let too_long = vec![1; limit + 1];
    assert_eq!(
        rig.send(CameraCommand::CaptureBracket {
            bracket: bracket_of(&too_long)
        }),
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::BracketTooLong))
    );
    assert!(rig.fake.take_calls().is_empty());
    // Ровно предел — можно.
    assert_eq!(
        rig.send(CameraCommand::CaptureBracket {
            bracket: bracket_of(&vec![1; limit])
        }),
        CommandOutcome::Accepted
    );
}

#[test]
fn bracket_needs_a_stream_and_a_manual_sensor() {
    let mut rig = Rig::new();
    assert_eq!(
        rig.send(CameraCommand::CaptureBracket {
            bracket: bracket_of(&[1])
        }),
        CommandOutcome::InvalidState(InvalidStateReason::NotStreaming)
    );

    let mut rig = streaming_rig_with_edited_wide(|camera| {
        camera
            .capabilities
            .request_capabilities
            .retain(|capability| *capability != RequestCapability::ManualSensor);
    });
    assert_eq!(
        rig.send(CameraCommand::CaptureBracket {
            bracket: bracket_of(&[1])
        }),
        CommandOutcome::Unsupported(UnsupportedCommand::ThreeA(UnsupportedThreeA::ManualSensor))
    );
    assert!(rig.fake.take_calls().is_empty());
}

#[test]
fn bracket_that_never_completes_fails_with_the_frames_that_did_arrive() {
    let mut rig = streaming_rig();
    let burst = capture(&mut rig, &[1, 2, 3]);
    rig.fake.complete_bracket_frames(burst, &[1]);
    rig.pump();
    rig.fresh_events();
    let timeout = Duration::from_millis(CameraEngineConfig::default().three_a.bracket_result_timeout_ms);

    rig.advance(timeout - Duration::from_millis(1));
    assert!(bracket_reports(&rig.fresh_events()).is_empty());
    rig.advance(Duration::from_millis(1));

    let reports = bracket_reports(&rig.fresh_events());
    let [
        BracketReport::Failed {
            burst: reported,
            failure,
            frames,
        },
    ] = reports.as_slice()
    else {
        panic!("ожидался один проваленный отчёт, а пришло {reports:?}");
    };
    assert_eq!((*reported, *failure), (burst, BracketFailure::ResultsTimedOut));
    assert_eq!(frames.iter().map(|frame| frame.index).collect::<Vec<_>>(), vec![1]);
    assert_eq!(rig.engine.status().three_a.counters.bracket_failures, 1);
    // Запоздавший кадр уже не воскрешает серию, и новая серия возможна.
    rig.fake.complete_bracket_frames(burst, &[0, 2]);
    rig.pump();
    assert!(bracket_reports(&rig.fresh_events()).is_empty());
    assert_eq!(
        rig.send(CameraCommand::CaptureBracket {
            bracket: bracket_of(&[1])
        }),
        CommandOutcome::Accepted
    );
}

#[test]
fn rejected_bracket_submission_is_reported_and_leaves_the_stream_running() {
    let mut rig = streaming_rig();
    rig.fake
        .push_bracket_result(Err(RequestFailure::Backend(BackendFailure {
            operation: "ACameraCaptureSession_capture",
            code: -10001,
            detail: "тест".to_owned(),
        })));
    let burst = capture(&mut rig, &[1, 2]);
    let reports = bracket_reports(&rig.fresh_events());
    assert_eq!(
        reports,
        vec![BracketReport::Failed {
            burst,
            failure: BracketFailure::SubmitFailed,
            frames: Vec::new()
        }]
    );
    assert_eq!(rig.state_name(), "streaming", "отказ серии не ломает видоискатель");
    let counters = rig.engine.status().three_a.counters;
    assert_eq!((counters.brackets_submitted, counters.bracket_failures), (0, 1));
    assert_eq!(rig.engine.next_deadline(), None);
}

#[test]
fn bracket_submission_on_a_lost_device_also_triggers_recovery() {
    let mut rig = streaming_rig();
    rig.fake.push_bracket_result(Err(RequestFailure::DeviceDisconnected));
    let burst = capture(&mut rig, &[1]);
    assert!(matches!(
        bracket_reports(&rig.fresh_events()).as_slice(),
        [BracketReport::Failed { burst: reported, failure: BracketFailure::SubmitFailed, .. }] if *reported == burst
    ));
    assert_eq!(rig.state_name(), "error");
}

#[test]
fn losing_the_session_mid_bracket_fails_it_with_session_lost() {
    let mut rig = streaming_rig();
    let burst = capture(&mut rig, &[1, 2, 3]);
    rig.fake.complete_bracket_frames(burst, &[0]);
    rig.pump();
    rig.fresh_events();
    rig.fake.disconnect_current_device();
    rig.pump();
    let reports = bracket_reports(&rig.fresh_events());
    let [BracketReport::Failed { failure, frames, .. }] = reports.as_slice() else {
        panic!("ожидался один проваленный отчёт, а пришло {reports:?}");
    };
    assert_eq!(*failure, BracketFailure::SessionLost);
    assert_eq!(frames.len(), 1);
    rig.advance(Duration::from_secs(60));
    assert!(
        bracket_reports(&rig.fresh_events()).is_empty(),
        "таймер потерянной серии не стреляет"
    );
}

#[test]
fn foreign_and_duplicate_bracket_frames_do_not_complete_or_corrupt_a_bracket() {
    let mut rig = streaming_rig();
    let burst = capture(&mut rig, &[1, 2]);
    rig.fresh_events();
    // Кадр чужой серии, индекс вне серии и повтор кадра 0 с другой экспозицией.
    rig.feed_result(result_meta(
        50,
        RequestOrigin::Burst {
            burst: BurstId(burst.0 + 100),
            index: 1,
        },
        None,
    ));
    rig.feed_result(result_meta(51, RequestOrigin::Burst { burst, index: 9 }, None));
    rig.fake.complete_bracket_frames(burst, &[0, 0]);
    rig.pump();
    assert!(bracket_reports(&rig.fresh_events()).is_empty(), "кадра 1 всё ещё нет");
    rig.fake.complete_bracket_frames(burst, &[1]);
    rig.pump();
    let reports = bracket_reports(&rig.fresh_events());
    let [BracketReport::Captured { frames, .. }] = reports.as_slice() else {
        panic!("ожидался успешный отчёт, а пришло {reports:?}");
    };
    assert_eq!(
        frame_exposures_ms(frames),
        vec![1, 2],
        "повтор кадра 0 не затёр первый результат"
    );
}

#[test]
fn bracket_results_do_not_disturb_the_viewfinder_stats() {
    let mut rig = streaming_rig();
    let burst = capture(&mut rig, &[1, 2]);
    rig.fake.complete_bracket_frames(burst, &[0, 1]);
    rig.pump();
    let status = rig.engine.status();
    // Разовые результаты идут невытесняемой полосой и обгоняют очередь кадров: в темп потока
    // их не берём (ревью 08), иначе интервалы портятся «не по порядку» и ложными пропусками.
    assert_eq!(
        status.stream.results_completed, 0,
        "кадры серии не входят в темп повторяющегося потока"
    );
    assert_eq!(status.stream.timestamps_out_of_order, 0);
    assert_eq!(status.stream.gaps, 0);
    assert_eq!(
        status.three_a.last_result.map(|last| last.origin),
        Some(RequestOrigin::Burst { burst, index: 1 })
    );
}
