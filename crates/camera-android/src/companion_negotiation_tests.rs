use std::cell::{Cell, RefCell};
use std::rc::Rc;

use camera_contract::geometry::PixelSize;
use camera_core::backend::BackendFailure;

use super::*;

const SPEC: CompanionStreamSpec = CompanionStreamSpec {
    size: PixelSize::new(640, 480),
    max_images: 4,
};

/// Фейковый поток: считает, сколько раз его освободили (Drop = освобождение ресурса платформы).
#[derive(Debug)]
struct FakeCompanion {
    drops: Rc<Cell<u32>>,
}

impl Drop for FakeCompanion {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

fn hal_rejection() -> ConfigureFailure {
    ConfigureFailure::RejectedByHal {
        detail: "STREAM_CONFIGURE_FAIL".to_owned(),
    }
}

/// Итог прогона: результат переговоров, «был ли поток» в каждой попытке и число освобождений потока.
type Run = (
    Result<Negotiated<FakeCompanion, &'static str>, ConfigureFailure>,
    Vec<bool>,
    Rc<Cell<u32>>,
);

fn run(
    spec: Option<CompanionStreamSpec>,
    companion_result: Result<(), i32>,
    attempts: Vec<Result<&'static str, AttemptError>>,
) -> Run {
    let drops = Rc::new(Cell::new(0));
    let seen = RefCell::new(Vec::new());
    let mut attempts = attempts.into_iter();
    let result = negotiate_session(
        spec,
        |_| {
            companion_result.map(|()| FakeCompanion {
                drops: Rc::clone(&drops),
            })
        },
        |companion| {
            seen.borrow_mut().push(companion.is_some());
            attempts
                .next()
                .unwrap_or(Err(AttemptError::Failed(ConfigureFailure::DeviceDisconnected)))
        },
    );
    (result, seen.into_inner(), drops)
}

#[test]
fn disabled_stream_builds_one_session_without_it_and_reports_disabled() {
    let (result, seen, drops) = run(None, Ok(()), vec![Ok("session")]);
    let negotiated = result.unwrap();
    assert_eq!(negotiated.session, "session");
    assert!(negotiated.companion.is_none());
    assert_eq!(negotiated.status, CompanionStreamStatus::DisabledByConfig);
    assert_eq!(seen, vec![false]);
    assert_eq!(drops.get(), 0);
}

#[test]
fn healthy_stream_goes_into_the_session_and_stays_alive() {
    let (result, seen, drops) = run(Some(SPEC), Ok(()), vec![Ok("session")]);
    let negotiated = result.unwrap();
    assert!(negotiated.companion.is_some());
    assert_eq!(
        negotiated.status,
        CompanionStreamStatus::Active {
            width: 640,
            height: 480
        }
    );
    assert_eq!(seen, vec![true], "одна попытка, и с потоком");
    assert_eq!(drops.get(), 0, "поток жив и отдан вызывающему");
}

#[test]
fn reader_creation_failure_builds_the_session_without_the_stream_and_says_why() {
    let (result, seen, _) = run(Some(SPEC), Err(-10000), vec![Ok("session")]);
    let negotiated = result.unwrap();
    assert!(negotiated.companion.is_none());
    assert_eq!(
        negotiated.status,
        CompanionStreamStatus::Unavailable {
            reason: CompanionUnavailableReason::ReaderCreationFailed { status: -10000 }
        }
    );
    assert_eq!(seen, vec![false]);
    assert!(negotiated.status.hal_three_a_is_dead());
}

#[test]
fn rejected_output_is_retried_without_the_stream_and_the_stream_is_released_first() {
    let (result, seen, drops) = run(
        Some(SPEC),
        Ok(()),
        vec![
            Err(AttemptError::CompanionOutputRejected { status: -10001 }),
            Ok("session"),
        ],
    );
    let negotiated = result.unwrap();
    assert_eq!(seen, vec![true, false]);
    assert_eq!(drops.get(), 1, "отвергнутый поток освобождён");
    assert!(negotiated.companion.is_none());
    assert_eq!(
        negotiated.status,
        CompanionStreamStatus::Unavailable {
            reason: CompanionUnavailableReason::OutputRegistrationFailed { status: -10001 }
        }
    );
}

#[test]
fn hal_rejection_with_the_stream_is_retried_without_it() {
    let (result, seen, drops) = run(
        Some(SPEC),
        Ok(()),
        vec![Err(AttemptError::Failed(hal_rejection())), Ok("session")],
    );
    let negotiated = result.unwrap();
    assert_eq!(seen, vec![true, false]);
    assert_eq!(drops.get(), 1);
    assert_eq!(
        negotiated.status,
        CompanionStreamStatus::Unavailable {
            reason: CompanionUnavailableReason::RejectedByHal
        }
    );
    assert_eq!(negotiated.session, "session");
}

#[test]
fn hal_rejection_without_a_stream_is_returned_as_is_without_a_retry_loop() {
    let (result, seen, _) = run(None, Ok(()), vec![Err(AttemptError::Failed(hal_rejection()))]);
    assert_eq!(result.unwrap_err(), hal_rejection());
    assert_eq!(seen, vec![false], "повторов без потока нет");
}

#[test]
fn unrelated_failure_with_the_stream_is_returned_immediately_without_a_second_attempt() {
    let (result, seen, drops) = run(
        Some(SPEC),
        Ok(()),
        vec![
            Err(AttemptError::Failed(ConfigureFailure::DeviceDisconnected)),
            Ok("never"),
        ],
    );
    assert_eq!(result.unwrap_err(), ConfigureFailure::DeviceDisconnected);
    assert_eq!(seen, vec![true]);
    assert_eq!(drops.get(), 1, "поток не утёк: освобождён при выходе");
}

#[test]
fn failure_of_the_retry_without_the_stream_is_the_final_error() {
    let backend_failure = ConfigureFailure::Backend(BackendFailure {
        operation: "ACameraDevice_createCaptureSession",
        code: -10005,
        detail: "x".to_owned(),
    });
    let (result, seen, _) = run(
        Some(SPEC),
        Ok(()),
        vec![
            Err(AttemptError::Failed(hal_rejection())),
            Err(AttemptError::Failed(backend_failure.clone())),
        ],
    );
    assert_eq!(result.unwrap_err(), backend_failure);
    assert_eq!(seen, vec![true, false]);
}

#[test]
fn output_rejection_in_an_attempt_without_a_stream_becomes_a_plain_backend_failure() {
    let (result, _, _) = run(
        None,
        Ok(()),
        vec![Err(AttemptError::CompanionOutputRejected { status: -10001 })],
    );
    assert!(matches!(
        result.unwrap_err(),
        ConfigureFailure::Backend(BackendFailure { code: -10001, .. })
    ));
}
