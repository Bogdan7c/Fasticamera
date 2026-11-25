//! Исполнитель источников: заявки, отказы, один ответ на заявку, годность кадров.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::flash::{FlashModeEcho, FlashState};
use camera_contract::frame_passport::FrameCaptureMeta;
use camera_contract::three_a::{BurstId, RequestOrigin};
use camera_contract::white::{
    WhileAwaitingWhite, WhiteAnswerChannel, WhiteRequestId, WhiteSourceOutcome, WhiteUnavailableReason,
};
use scene_estimation::white_source::{WhiteInputKind, WhiteRequestRefusal};

use std::num::NonZeroU32;

use camera_contract::white::WhiteSourceId;
use camera_contract::white_reading::{WhiteReading, WhiteReadingBoard};

use super::sources::SourceExecutor;
use super::test_support::{
    Call, FAKE_ID, FakeSourceBoard, fake_registry, frame, new_stream, ready_outcome, request_of, setting_of,
};
use super::*;

const HIDE: WhileAwaitingWhite = WhileAwaitingWhite::HideFrames;
const READING_EVERY: NonZeroU32 = NonZeroU32::MIN;

fn executor_with(input: WhiteInputKind) -> (SourceExecutor, Arc<FakeSourceBoard>, WhiteAnswerChannel) {
    let (registry, board) = fake_registry(input);
    let answers = WhiteAnswerChannel::new();
    let mut executor = SourceExecutor::new(&registry, answers.clone(), WhiteReadingBoard::new(), READING_EVERY)
        .expect("источники собираются");
    executor.begin_stream(CameraTag::of(&new_stream().camera), None);
    (executor, board, answers)
}

fn answer_count(answers: &WhiteAnswerChannel) -> u64 {
    answers.newer_than(None).map_or(0, |(generation, _)| generation.0)
}

#[test]
fn an_unregistered_source_gets_not_registered_at_once_and_once() {
    let (registry, _) = fake_registry(WhiteInputKind::ColorHints);
    let answers = WhiteAnswerChannel::new();
    let mut executor =
        SourceExecutor::new(&registry, answers.clone(), WhiteReadingBoard::new(), READING_EVERY).unwrap();
    executor.begin_stream(CameraTag::of(&new_stream().camera), None);
    let unknown = camera_contract::white::WhiteSourceId::new("nobody");
    let request = request_of(7, unknown, HIDE);
    executor.sync_request(Some(request));
    executor.sync_request(Some(request));
    let (_, answer) = answers.newer_than(None).expect("ответ опубликован сразу");
    assert_eq!(answer.request, WhiteRequestId(7));
    assert_eq!(
        answer.outcome,
        WhiteSourceOutcome::Unavailable {
            reason: WhiteUnavailableReason::NotRegistered
        }
    );
    assert_eq!(answer_count(&answers), 1, "на заявку один ответ");
    assert_eq!(executor.counters().answers_published, 1);
}

#[test]
fn a_source_that_needs_more_than_colour_hints_gets_input_not_provided_without_being_asked() {
    for input in [WhiteInputKind::SceneTiles, WhiteInputKind::LinearThumbnail] {
        let (mut executor, board, answers) = executor_with(input);
        executor.sync_request(Some(request_of(1, FAKE_ID, HIDE)));
        let (_, answer) = answers.newer_than(None).expect("ответ");
        assert_eq!(
            answer.outcome,
            WhiteSourceOutcome::Unavailable {
                reason: WhiteUnavailableReason::InputNotProvided
            }
        );
        assert!(board.requests().is_empty(), "источнику заявку не подавали");
    }
}

#[test]
fn refusals_of_the_source_are_published_as_distinct_outcomes() {
    let cases = [
        (WhiteRequestRefusal::Busy, WhiteSourceOutcome::Busy),
        (
            WhiteRequestRefusal::Unavailable(WhiteUnavailableReason::AutomationSilent),
            WhiteSourceOutcome::Unavailable {
                reason: WhiteUnavailableReason::AutomationSilent,
            },
        ),
    ];
    for (refusal, expected) in cases {
        let (mut executor, board, answers) = executor_with(WhiteInputKind::ColorHints);
        *board.refusal.lock().unwrap() = Some(refusal);
        executor.sync_request(Some(request_of(1, FAKE_ID, HIDE)));
        assert_eq!(answers.newer_than(None).expect("ответ").1.outcome, expected);
    }
}

#[test]
fn a_ready_reply_is_published_with_the_camera_and_only_once_per_request() {
    let (mut executor, board, answers) = executor_with(WhiteInputKind::ColorHints);
    executor.sync_request(Some(request_of(3, FAKE_ID, HIDE)));
    assert_eq!(board.requests(), vec![WhiteRequestId(3)]);
    assert_eq!(answer_count(&answers), 0, "источник ещё считает");
    board.reply(3, ready_outcome(2800.0));
    board.reply(3, ready_outcome(9000.0));
    executor.collect_answers();
    let (_, answer) = answers.newer_than(None).expect("ответ");
    assert_eq!(answer.outcome, ready_outcome(2800.0), "победил первый ответ");
    assert_eq!(answer.camera, CameraTag::of(&new_stream().camera));
    assert_eq!(answer_count(&answers), 1);
    assert_eq!(
        executor.counters().replies_dropped,
        1,
        "повторный ответ отброшен и сосчитан"
    );
    assert_eq!(executor.last_answer(), Some(answer));
}

#[test]
fn a_reply_to_a_cancelled_or_foreign_request_is_not_published() {
    let (mut executor, board, answers) = executor_with(WhiteInputKind::ColorHints);
    executor.sync_request(Some(request_of(1, FAKE_ID, HIDE)));
    // Владелец заменил заявку (ручная команда, новая кнопка): прежняя отменена у источника.
    executor.sync_request(Some(request_of(2, FAKE_ID, WhileAwaitingWhite::KeepShowing)));
    assert_eq!(board.count(Call::Cancel(WhiteRequestId(1))), 1);
    assert_eq!(board.requests(), vec![WhiteRequestId(1), WhiteRequestId(2)]);
    board.reply(1, ready_outcome(2800.0));
    board.reply(99, ready_outcome(2800.0));
    executor.collect_answers();
    assert_eq!(answer_count(&answers), 0);
    assert_eq!(executor.counters().replies_dropped, 2);
    // Заявка исчезла совсем: у источника отменена и она.
    executor.sync_request(None);
    assert_eq!(board.count(Call::Cancel(WhiteRequestId(2))), 1);
    board.reply(2, ready_outcome(2800.0));
    executor.collect_answers();
    assert_eq!(answer_count(&answers), 0, "ответ на снятую заявку не публикуется");
}

#[test]
fn a_new_stream_resubmits_the_pending_request_but_never_a_request_already_answered() {
    let (mut executor, board, answers) = executor_with(WhiteInputKind::ColorHints);
    let request = request_of(5, FAKE_ID, HIDE);
    executor.sync_request(Some(request));
    // Поток сменился до ответа: источник заявку забыл, подаём заново ТОТ ЖЕ номер.
    executor.begin_stream(CameraTag::of(&new_stream().camera), None);
    executor.sync_request(Some(request));
    assert_eq!(board.requests(), vec![WhiteRequestId(5), WhiteRequestId(5)]);
    board.reply(5, ready_outcome(3000.0));
    executor.collect_answers();
    assert_eq!(answer_count(&answers), 1);
    // Владелец ещё не закрыл заявку указанием, а поток снова сменился: второго ответа не будет.
    executor.begin_stream(CameraTag::of(&new_stream().camera), None);
    executor.sync_request(Some(request));
    assert_eq!(board.requests().len(), 2, "отвеченную заявку заново не подают");
    assert_eq!(answer_count(&answers), 1);
}

#[test]
fn only_viewfinder_frames_in_ambient_light_with_their_own_result_reach_the_source() {
    let (mut executor, board, _) = executor_with(WhiteInputKind::ColorHints);
    let stream = new_stream();
    executor.observe(&frame(&stream, 0));
    assert_eq!(board.observed(), 1, "обычный кадр дошёл");

    let mut burst = frame(&stream, 1);
    let mut torch = frame(&stream, 2);
    let mut flash_still = frame(&stream, 3);
    let mut without_result = frame(&stream, 4);
    if let FrameCaptureMeta::Paired(details) = &mut burst.capture {
        details.result.three_a.origin = RequestOrigin::Burst {
            burst: BurstId(1),
            index: 0,
        };
    }
    if let FrameCaptureMeta::Paired(details) = &mut torch.capture {
        details.result.three_a.flash.mode = Some(FlashModeEcho::Torch);
        details.result.three_a.flash.state = Some(FlashState::Fired);
    }
    if let FrameCaptureMeta::Paired(details) = &mut flash_still.capture {
        details.result.three_a.flash.state = Some(FlashState::Fired);
    }
    without_result.capture = FrameCaptureMeta::Missing {
        reason: camera_contract::frame_passport::MissingCaptureMeta::PairingTimedOut,
    };
    for passport in [&burst, &torch, &flash_still, &without_result] {
        executor.observe(passport);
    }
    assert_eq!(
        board.observed(),
        1,
        "серия, фонарик, вспышка и кадр без результата источнику не нужны"
    );
    assert_eq!(executor.counters().frames_observed, 1);
}

// ---- показания (WB33) ----

fn reading_stand(
    input: WhiteInputKind,
    every: u32,
) -> (
    SourceExecutor,
    Arc<FakeSourceBoard>,
    WhiteAnswerChannel,
    WhiteReadingBoard,
) {
    let (registry, board) = fake_registry(input);
    let answers = WhiteAnswerChannel::new();
    let readings = WhiteReadingBoard::new();
    let every = NonZeroU32::new(every).expect("интервал не ноль");
    let mut executor = SourceExecutor::new(&registry, answers.clone(), readings.clone(), every).unwrap();
    executor.begin_stream(CameraTag::of(&new_stream().camera), None);
    (executor, board, answers, readings)
}

fn shown_by_fake(board: &FakeSourceBoard, reading: WhiteReading) {
    *board.shown_reading.lock().unwrap() = Some(reading);
}

fn measured(kelvin: f64) -> WhiteReading {
    WhiteReading::Measured {
        white: setting_of(kelvin),
    }
}

fn on_board(readings: &WhiteReadingBoard) -> Option<WhiteReading> {
    readings.latest_of(FAKE_ID)
}

#[test]
fn a_new_stream_puts_not_measured_yet_on_the_board_replacing_the_old_number() {
    let (mut executor, board, _, readings) = reading_stand(WhiteInputKind::ColorHints, 1);
    assert_eq!(
        on_board(&readings),
        Some(WhiteReading::NotMeasuredYet),
        "сразу после begin_stream"
    );
    shown_by_fake(&board, measured(3100.0));
    executor.observe(&frame(&new_stream(), 0));
    assert_eq!(on_board(&readings), Some(measured(3100.0)));
    executor.begin_stream(CameraTag::of(&new_stream().camera), None);
    assert_eq!(
        on_board(&readings),
        Some(WhiteReading::NotMeasuredYet),
        "число прежнего потока на доске не остаётся"
    );
}

#[test]
fn readings_are_refreshed_only_every_n_eligible_frames() {
    let (mut executor, board, _, readings) = reading_stand(WhiteInputKind::ColorHints, 3);
    let stream = new_stream();
    shown_by_fake(&board, measured(3100.0));
    executor.observe(&frame(&stream, 0));
    executor.observe(&frame(&stream, 1));
    assert_eq!(
        on_board(&readings),
        Some(WhiteReading::NotMeasuredYet),
        "двух кадров мало"
    );
    executor.observe(&frame(&stream, 2));
    assert_eq!(on_board(&readings), Some(measured(3100.0)), "на третьем годном кадре");
    shown_by_fake(&board, measured(4500.0));
    executor.observe(&frame(&stream, 3));
    executor.observe(&frame(&stream, 4));
    assert_eq!(
        on_board(&readings),
        Some(measured(3100.0)),
        "до следующего интервала число то же"
    );
    executor.observe(&frame(&stream, 5));
    assert_eq!(on_board(&readings), Some(measured(4500.0)));
}

#[test]
fn frames_that_never_reach_the_source_do_not_count_towards_the_interval() {
    let (mut executor, board, _, readings) = reading_stand(WhiteInputKind::ColorHints, 2);
    let stream = new_stream();
    shown_by_fake(&board, measured(3100.0));
    let mut burst = frame(&stream, 1);
    if let FrameCaptureMeta::Paired(details) = &mut burst.capture {
        details.result.three_a.origin = RequestOrigin::Burst {
            burst: BurstId(1),
            index: 0,
        };
    }
    executor.observe(&frame(&stream, 0));
    executor.observe(&burst);
    executor.observe(&burst);
    assert_eq!(
        on_board(&readings),
        Some(WhiteReading::NotMeasuredYet),
        "серия показание не двигает"
    );
    executor.observe(&frame(&stream, 2));
    assert_eq!(on_board(&readings), Some(measured(3100.0)));
}

#[test]
fn an_unchanged_reading_is_not_published_again() {
    let (mut executor, board, _, readings) = reading_stand(WhiteInputKind::ColorHints, 1);
    let stream = new_stream();
    shown_by_fake(&board, measured(3100.0));
    for index in 0..10 {
        executor.observe(&frame(&stream, index));
    }
    assert_eq!(
        executor.counters().readings_published,
        3,
        "NotMeasuredYet при создании и при начале потока, одно измеренное; повторов нет"
    );
    shown_by_fake(&board, measured(3200.0));
    executor.observe(&frame(&stream, 10));
    assert_eq!(executor.counters().readings_published, 4);
    assert_eq!(on_board(&readings), Some(measured(3200.0)));
}

#[test]
fn a_source_whose_input_is_not_prepared_reads_input_not_provided() {
    for input in [WhiteInputKind::SceneTiles, WhiteInputKind::LinearThumbnail] {
        let (mut executor, board, _, readings) = reading_stand(input, 1);
        shown_by_fake(&board, measured(3100.0));
        executor.observe(&frame(&new_stream(), 0));
        assert_eq!(
            on_board(&readings),
            Some(WhiteReading::Unavailable {
                reason: WhiteUnavailableReason::InputNotProvided
            }),
            "сам источник мог бы показать число, но вход ему не готовится"
        );
    }
}

#[test]
fn publishing_readings_neither_answers_nor_touches_the_request() {
    let (mut executor, board, answers, readings) = reading_stand(WhiteInputKind::ColorHints, 1);
    let stream = new_stream();
    executor.sync_request(Some(request_of(8, FAKE_ID, HIDE)));
    shown_by_fake(&board, measured(3100.0));
    for index in 0..6 {
        executor.observe(&frame(&stream, index));
        executor.collect_answers();
    }
    assert_eq!(on_board(&readings), Some(measured(3100.0)));
    assert_eq!(answer_count(&answers), 0, "показание ответом не становится");
    assert_eq!(executor.last_answer(), None);
    assert_eq!(
        board.requests(),
        vec![WhiteRequestId(8)],
        "заявка подана один раз и не тронута"
    );
    assert_eq!(board.count(Call::Cancel(WhiteRequestId(8))), 0);
    // Ответ источника по-прежнему идёт своим путём.
    board.reply(8, ready_outcome(2800.0));
    executor.collect_answers();
    assert_eq!(answer_count(&answers), 1);
}

#[test]
fn sources_do_not_mix_on_the_board() {
    let (mut executor, board, _, readings) = reading_stand(WhiteInputKind::ColorHints, 1);
    shown_by_fake(&board, measured(3100.0));
    executor.observe(&frame(&new_stream(), 0));
    assert_eq!(
        readings.latest_of(WhiteSourceId::PHONE),
        None,
        "чужой источник не появился"
    );
}

#[test]
fn a_new_executor_wipes_the_previous_sessions_number_before_any_frame() {
    let (registry, _) = fake_registry(WhiteInputKind::ColorHints);
    let readings = WhiteReadingBoard::new();
    readings.publish(camera_contract::white_reading::WhiteSourceReading {
        source: FAKE_ID,
        reading: measured(3100.0),
    });
    let _fresh = SourceExecutor::new(&registry, WhiteAnswerChannel::new(), readings.clone(), READING_EVERY).unwrap();
    assert_eq!(
        on_board(&readings),
        Some(WhiteReading::NotMeasuredYet),
        "кадров ещё не было"
    );
}

#[test]
fn an_ended_stream_wipes_the_reading_and_the_next_measurement_is_published_again() {
    let (mut executor, board, _, readings) = reading_stand(WhiteInputKind::ColorHints, 1);
    shown_by_fake(&board, measured(3100.0));
    executor.observe(&frame(&new_stream(), 0));
    assert_eq!(on_board(&readings), Some(measured(3100.0)));
    // Источник ещё помнит число (begin_stream не было), но поток кончился.
    executor.forget_stream_readings();
    assert_eq!(on_board(&readings), Some(WhiteReading::NotMeasuredYet));
    // То же число на новом кадре обязано вернуться на доску: «только при изменении» сверяется с доской, не с источником.
    executor.observe(&frame(&new_stream(), 1));
    assert_eq!(on_board(&readings), Some(measured(3100.0)));
}
