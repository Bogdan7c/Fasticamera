//! Тесты расписания статистики: период, новый поток, скрытые кадры, требование «каждый кадр», нет потребителей.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use super::*;
use crate::vulkan_backend::camera::white_balance::test_support::{frame, new_stream};

const COMPUTE: StatisticsPlan = StatisticsPlan::Compute;
const SKIP: StatisticsPlan = StatisticsPlan::Skip;

fn schedule(every_n_frames: u32) -> StatisticsSchedule {
    StatisticsSchedule::new(NonZeroU32::new(every_n_frames).expect("период не нулевой"))
}

fn periodic() -> StatisticsNeeds {
    StatisticsNeeds {
        auto_exposure: Some(StatisticsDemand::Periodic),
        cover: ColdStartCover::Showing,
    }
}

/// Решения на `count` кадрах одного потока, начиная с кадра `from`.
fn plans(
    schedule: &mut StatisticsSchedule,
    stream: &Arc<StreamPassport>,
    from: i64,
    count: i64,
    needs: StatisticsNeeds,
) -> Vec<StatisticsPlan> {
    (from..from + count)
        .map(|index| schedule.decide(&frame(stream, index), needs).gpu)
        .collect()
}

#[test]
fn every_fourth_frame_computes_statistics_and_counters_add_up() {
    let mut schedule = schedule(4);
    let stream = new_stream();
    let plans = plans(&mut schedule, &stream, 0, 12, periodic());
    let expected = [COMPUTE, SKIP, SKIP, SKIP];
    assert_eq!(plans[..4], expected);
    assert_eq!(plans[4..8], expected);
    assert_eq!(plans[8..], expected);
    let diagnostics = schedule.diagnostics();
    assert_eq!(
        (
            diagnostics.every_n_frames,
            diagnostics.requested,
            diagnostics.skipped_by_period
        ),
        (4, 3, 9)
    );
}

#[test]
fn period_one_computes_every_frame() {
    let mut schedule = schedule(1);
    let stream = new_stream();
    assert!(
        plans(&mut schedule, &stream, 0, 5, periodic())
            .iter()
            .all(|plan| *plan == COMPUTE)
    );
}

#[test]
fn without_a_consumer_statistics_are_never_computed_and_nothing_is_counted() {
    let mut schedule = schedule(4);
    let stream = new_stream();
    let none = StatisticsNeeds {
        auto_exposure: None,
        cover: ColdStartCover::Showing,
    };
    let decisions: Vec<_> = (0..8)
        .map(|index| schedule.decide(&frame(&stream, index), none))
        .collect();
    assert!(
        decisions
            .iter()
            .all(|decision| decision.gpu == SKIP && !decision.ae_receives())
    );
    let diagnostics = schedule.diagnostics();
    assert_eq!((diagnostics.requested, diagnostics.skipped_by_period), (0, 0));
    // Даже скрытые холодным стартом кадры статистику без потребителя не получают: она стоит тока.
    let hidden_without_consumer = StatisticsNeeds {
        auto_exposure: None,
        cover: ColdStartCover::HidingFrames,
    };
    assert_eq!(schedule.decide(&frame(&stream, 8), hidden_without_consumer).gpu, SKIP);
}

#[test]
fn a_new_stream_restarts_the_schedule_and_its_first_frame_computes() {
    let mut schedule = schedule(4);
    let first = new_stream();
    assert_eq!(plans(&mut schedule, &first, 0, 2, periodic()), [COMPUTE, SKIP]);
    // Кадр 2 прежнего потока был бы «пропустить»; новый поток обязан дать статистику сразу.
    let second = new_stream();
    assert_eq!(
        plans(&mut schedule, &second, 0, 5, periodic()),
        [COMPUTE, SKIP, SKIP, SKIP, COMPUTE]
    );
}

#[test]
fn hidden_frames_compute_every_frame_then_the_period_counts_from_the_last_one() {
    let mut schedule = schedule(4);
    let stream = new_stream();
    let hidden = StatisticsNeeds {
        auto_exposure: Some(StatisticsDemand::Periodic),
        cover: ColdStartCover::HidingFrames,
    };
    assert_eq!(plans(&mut schedule, &stream, 0, 3, hidden), [COMPUTE; 3]);
    assert_eq!(
        plans(&mut schedule, &stream, 3, 4, periodic()),
        [SKIP, SKIP, SKIP, COMPUTE],
        "после скрытия период отсчитывается от последнего скрытого кадра"
    );
}

#[test]
fn ae_every_frame_demand_computes_without_shifting_the_steady_period() {
    let mut schedule = schedule(4);
    let stream = new_stream();
    let every_frame = StatisticsNeeds {
        auto_exposure: Some(StatisticsDemand::EveryFrame),
        cover: ColdStartCover::Showing,
    };
    // Кадр 0 — по периоду (первый кадр потока), кадры 1–2 — по требованию AE.
    let mut all = plans(&mut schedule, &stream, 0, 1, periodic());
    all.extend(plans(&mut schedule, &stream, 1, 2, every_frame));
    all.extend(plans(&mut schedule, &stream, 3, 2, periodic()));
    assert_eq!(all, [COMPUTE, COMPUTE, COMPUTE, SKIP, COMPUTE]);
    // Периодный кадр пришёл на 4-й от нулевого (как если бы AE ничего не форсировал), а не от последнего форсированного.
    let diagnostics = schedule.diagnostics();
    assert_eq!((diagnostics.requested, diagnostics.skipped_by_period), (4, 1));
}

#[test]
fn only_computed_frames_are_handed_to_the_consumer() {
    let mut schedule = schedule(2);
    let stream = new_stream();
    let decisions: Vec<_> = (0..4)
        .map(|index| schedule.decide(&frame(&stream, index), periodic()))
        .collect();
    let received: Vec<_> = decisions.iter().map(|decision| decision.ae_receives()).collect();
    assert_eq!(received, [true, false, true, false]);
}

#[test]
fn the_schedule_does_not_depend_on_the_color_calibration_of_the_stream() {
    let mut schedule = schedule(4);
    let calibrated = new_stream();
    let uncalibrated = Arc::new(StreamPassport {
        color_calibration: Vec::new(),
        ..(*calibrated).clone()
    });
    assert_eq!(
        plans(&mut schedule, &uncalibrated, 0, 5, periodic()),
        [COMPUTE, SKIP, SKIP, SKIP, COMPUTE],
        "поток без цветовой калибровки получает статистику для AE так же"
    );
}

#[test]
fn hidden_presentation_maps_to_hiding_cover_and_shown_to_showing() {
    assert_eq!(
        ColdStartCover::of_presentation(IspPresentation::Show),
        ColdStartCover::Showing
    );
    assert_eq!(
        ColdStartCover::of_presentation(IspPresentation::HideUntilColorKnown),
        ColdStartCover::HidingFrames
    );
}
