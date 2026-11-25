//! Тесты перевода белого в факты экрана: каждый исход заявки, ожидание, пределы, показание телефона только
//! при идущем потоке и «факты не меняются без причины».

use std::sync::Arc;

use camera_contract::catalog::CameraCatalog;
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::fixtures::{camera_id, main_wide_camera};
use camera_contract::geometry::PixelSize;
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ConfiguredStream, FrameRateMechanism, ReadoutAspect, ResolvedStream, TargetFrameRate};
use camera_contract::white::{
    HeldWhite, WhiteConfidence, WhiteOrigin, WhiteRequestId, WhiteSourceFailure, WhiteSourceId, WhiteUnavailableReason,
    WhiteUnknownReason,
};
use camera_contract::white_reading::{WhiteReading, WhiteReadingBoard, WhiteSourceReading};
use camera_core::command::{CommandId, InvalidStateReason};
use camera_core::lens_status::LensStatus;
use camera_core::state::ClosedReason;
use camera_core::status::{AppPresence, CameraCounters, StreamStats};
use camera_core::three_a_status::ThreeAStatus;
use camera_core::white::{
    WhiteCancelCause, WhiteFollowState, WhiteRequestOutcome, WhiteRequestResult, WhiteRequestState, WhiteTakeReport,
    WhiteTakeState as EngineTake,
};
use camera_core::{CameraState, CameraStatus};
use camera_screen::{WhiteFollowFacts, WhiteTakeFacts, WhiteTakeState};

use super::white_facts_of;
use crate::camera_screen_facts::{CameraFactsWatcher, FactsCache, FactsChange, camera_facts_with_cache};

fn status_in(state: CameraState) -> CameraStatus {
    CameraStatus {
        state,
        presence: AppPresence::Foreground,
        catalog: Arc::new(CameraCatalog::new(vec![main_wide_camera()], Vec::new()).expect("каталог образцов")),
        configured: None,
        stream: StreamStats::empty(),
        last_unavailable: None,
        counters: CameraCounters::default(),
        three_a: ThreeAStatus::initial(),
        lens: LensStatus::initial(),
    }
}

fn streaming_state() -> CameraState {
    CameraState::Streaming {
        stream: ConfiguredStream {
            stream: ResolvedStream {
                camera: camera_id("2"),
                raw_format: RawFormat::Raw10,
                buffer_size: PixelSize::new(4096, 3072),
                readout: ReadoutAspect::FourByThree,
                frame_rate: TargetFrameRate::new(30).expect("30 к/с"),
            },
            frame_rate_mechanism: FrameRateMechanism::HalDefault,
        },
    }
}

fn kelvin(value: f64) -> ColorTemperature {
    ColorTemperature::from_kelvin(value).expect("температура")
}

fn measured(value: f64) -> WhiteReading {
    WhiteReading::Measured {
        white: WhiteBalanceSetting {
            temperature: kelvin(value),
            tint: Tint::NEUTRAL,
        },
    }
}

fn with_take(command: u64, state: EngineTake) -> CameraStatus {
    let mut status = status_in(streaming_state());
    status.three_a.white.last_take = Some(WhiteTakeReport {
        command: CommandId(command),
        state,
    });
    status
}

#[test]
fn every_request_outcome_maps_to_one_screen_take_state() {
    let table = [
        (
            WhiteRequestOutcome::Applied {
                confidence: WhiteConfidence::FULL,
            },
            WhiteTakeState::Taken,
        ),
        (
            WhiteRequestOutcome::Cancelled {
                by: WhiteCancelCause::ManualWhite,
            },
            WhiteTakeState::Cancelled,
        ),
        (
            WhiteRequestOutcome::Cancelled {
                by: WhiteCancelCause::Background,
            },
            WhiteTakeState::Cancelled,
        ),
        (WhiteRequestOutcome::Busy, WhiteTakeState::Refused),
        (
            WhiteRequestOutcome::Unknown {
                reason: WhiteUnknownReason::NoFreshMeasurement,
            },
            WhiteTakeState::Refused,
        ),
        (
            WhiteRequestOutcome::Unavailable {
                reason: WhiteUnavailableReason::NotRegistered,
            },
            WhiteTakeState::Refused,
        ),
        (
            WhiteRequestOutcome::Failed {
                failure: WhiteSourceFailure::Executor,
            },
            WhiteTakeState::Refused,
        ),
        (WhiteRequestOutcome::NotUnderstood, WhiteTakeState::Refused),
        (WhiteRequestOutcome::TimedOut, WhiteTakeState::Refused),
    ];
    for (outcome, expected) in table {
        let facts = white_facts_of(&with_take(9, EngineTake::Finished(outcome)), &WhiteReadingBoard::new());
        assert_eq!(
            facts.take,
            Some(WhiteTakeFacts {
                serial: 9,
                state: expected
            }),
            "{outcome:?}"
        );
    }
}

#[test]
fn a_command_the_engine_never_asked_the_source_about_is_a_refusal_with_its_number() {
    let status = with_take(
        4,
        EngineTake::NotAsked {
            reason: InvalidStateReason::NotStreaming,
        },
    );
    assert_eq!(
        white_facts_of(&status, &WhiteReadingBoard::new()).take,
        Some(WhiteTakeFacts {
            serial: 4,
            state: WhiteTakeState::Refused
        })
    );
}

#[test]
fn a_waiting_take_and_a_waiting_request_are_both_visible_and_no_take_means_none() {
    let waiting = with_take(5, EngineTake::Waiting);
    assert_eq!(
        white_facts_of(&waiting, &WhiteReadingBoard::new()).take,
        Some(WhiteTakeFacts {
            serial: 5,
            state: WhiteTakeState::Waiting
        })
    );
    let idle = status_in(streaming_state());
    let facts = white_facts_of(&idle, &WhiteReadingBoard::new());
    assert_eq!(facts.take, None, "команд «взять» не было");

    let mut asking = status_in(streaming_state());
    asking.three_a.white.request = WhiteRequestState::Waiting {
        request: WhiteRequestId(1),
        source: WhiteSourceId::PHONE,
    };
    // Заявка открытия ждёт, а своей команды «взять» нет: кнопка не мерцает (в фактах нет Waiting).
    assert_eq!(white_facts_of(&asking, &WhiteReadingBoard::new()).take, None);
    // Заявка прошла и закрыта: ожидания нет, а отчёта о своей команде заявка открытия приложения не создаёт.
    asking.three_a.white.request = WhiteRequestState::Idle {
        last: Some(WhiteRequestResult {
            request: WhiteRequestId(1),
            source: WhiteSourceId::PHONE,
            result: WhiteRequestOutcome::TimedOut,
        }),
    };
    let after = white_facts_of(&asking, &WhiteReadingBoard::new());
    assert_eq!(after.take, None);
}

#[test]
fn held_white_origin_and_engine_limits_come_through_unchanged() {
    let mut status = status_in(streaming_state());
    let held = HeldWhite {
        setting: WhiteBalanceSetting {
            temperature: kelvin(3_140.0),
            tint: Tint::from_duv(0.012).expect("оттенок"),
        },
        origin: WhiteOrigin::Source(WhiteSourceId::PHONE),
    };
    status.three_a.white.held = held;
    let facts = white_facts_of(&status, &WhiteReadingBoard::new());
    assert_eq!(facts.held, held);
    let engine_range = status.three_a.white.range;
    assert_eq!(facts.range.min_kelvin, engine_range.min_temperature.kelvin());
    assert_eq!(facts.range.max_kelvin, engine_range.max_temperature.kelvin());
    assert_eq!(facts.range.max_abs_duv, engine_range.max_abs_tint.duv());
}

#[test]
fn the_phone_reading_shows_only_while_the_stream_runs() {
    let board = WhiteReadingBoard::new();
    board.publish(WhiteSourceReading {
        source: WhiteSourceId::PHONE,
        reading: measured(3_100.0),
    });
    // Показание чужого источника на кнопку «Телефон» не попадает.
    board.publish(WhiteSourceReading {
        source: WhiteSourceId::new("network"),
        reading: measured(9_000.0),
    });
    assert_eq!(
        white_facts_of(&status_in(streaming_state()), &board).phone_reading,
        Some(measured(3_100.0))
    );
    for stopped in [
        CameraState::Closed {
            reason: ClosedReason::AppInBackground,
        },
        CameraState::Opening { camera: camera_id("2") },
    ] {
        assert_eq!(
            white_facts_of(&status_in(stopped.clone()), &board).phone_reading,
            None,
            "{stopped:?}: старое число не показывается"
        );
    }
    // Без показаний на доске — тоже «нет», не выдумка.
    assert_eq!(
        white_facts_of(&status_in(streaming_state()), &WhiteReadingBoard::new()).phone_reading,
        None
    );
}

#[test]
fn facts_follow_the_shared_board_and_stay_unchanged_without_a_reason() {
    let board = WhiteReadingBoard::new();
    let mut cache = FactsCache::with_white_readings(board.clone());
    let mut watcher = CameraFactsWatcher::new();
    let status = status_in(streaming_state());

    let first = camera_facts_with_cache(&status, &mut cache);
    assert!(matches!(watcher.observe(first), FactsChange::Changed(_)));
    // Два одинаковых статуса подряд: перерисовывать нечего.
    assert_eq!(
        watcher.observe(camera_facts_with_cache(&status, &mut cache)),
        FactsChange::Unchanged
    );

    // Рендер опубликовал показание: факты изменились ровно один раз и несут его.
    board.publish(WhiteSourceReading {
        source: WhiteSourceId::PHONE,
        reading: measured(3_100.0),
    });
    match watcher.observe(camera_facts_with_cache(&status, &mut cache)) {
        FactsChange::Changed(facts) => assert_eq!(facts.white.phone_reading, Some(measured(3_100.0))),
        FactsChange::Unchanged => panic!("новое показание обязано дойти до экрана"),
    }
    assert_eq!(
        watcher.observe(camera_facts_with_cache(&status, &mut cache)),
        FactsChange::Unchanged
    );

    // Поток остановился: показание уходит с экрана, хотя на доске оно ещё лежит.
    let closed = status_in(CameraState::Closed {
        reason: ClosedReason::AppInBackground,
    });
    match watcher.observe(camera_facts_with_cache(&closed, &mut cache)) {
        FactsChange::Changed(facts) => assert_eq!(facts.white.phone_reading, None),
        FactsChange::Unchanged => panic!("остановка потока меняет факты"),
    }
}

#[test]
fn the_follow_state_of_the_engine_maps_to_on_for_following_and_for_held_by_lock_and_to_off_otherwise() {
    let phone = WhiteSourceId::PHONE;
    let table = [
        (WhiteFollowState::Off, WhiteFollowFacts::Off),
        (WhiteFollowState::Following { source: phone }, WhiteFollowFacts::On),
        // Заморожено блокировкой AE/AF (WB46): для человека режим тот же, «Auto» остаётся горящей.
        (WhiteFollowState::HeldByLock { source: phone }, WhiteFollowFacts::On),
    ];
    for (engine_state, expected) in table {
        let mut status = status_in(streaming_state());
        status.three_a.white.follow.state = engine_state;
        let facts = white_facts_of(&status, &WhiteReadingBoard::new());
        assert_eq!(facts.follow, expected, "{engine_state:?}");
    }
}

#[test]
fn a_fresh_status_starts_with_follow_off() {
    let facts = white_facts_of(&status_in(streaming_state()), &WhiteReadingBoard::new());
    assert_eq!(facts.follow, WhiteFollowFacts::Off);
}
