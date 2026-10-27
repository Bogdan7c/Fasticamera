//! Буфер видоискателя с запасом (решение 53): каждый кадр сенсора показывается ровно один раз.
//!
//! Модель: «приход» — публикация кадра со своим результатом, «тик» — один опрос `poll_for_vsync`
//! (как рендер на каждом показанном vsync); выданный кадр сразу возвращается нетронутым.

use camera_contract::frame_passport::FrameSequence;

use super::support::{Rig, latest_wins_config};
use crate::config::RawFrameStreamConfig;
use crate::stream::ViewfinderPoll;
use crate::viewfinder_pacing::{JitterBufferConfig, ViewfinderPacing, ViewfinderPacingSwitch};

/// Кадр приходит сразу со своим результатом: публикуется немедленно.
pub(super) fn arrive(rig: &Rig, timestamp: i64) {
    rig.offer_result(timestamp, 0);
    rig.offer_frame(timestamp, 0);
}

/// Один тик vsync: метка выданного кадра (`None` — показывать прежний). Кадр сразу возвращается.
pub(super) fn tick(rig: &Rig) -> Option<i64> {
    match rig.stream.viewfinder().poll_for_vsync(rig.at(0)) {
        ViewfinderPoll::NewFrame(lease) => {
            let timestamp = lease.passport().sensor_timestamp.as_nanos();
            lease.release_unused();
            Some(timestamp)
        }
        ViewfinderPoll::NoNewFrame => None,
        ViewfinderPoll::StreamInactive => panic!("поток должен быть активен"),
    }
}

fn jitter_rig() -> Rig {
    Rig::new(RawFrameStreamConfig::default())
}

#[test]
fn the_buffer_waits_for_its_reserve_and_then_hands_out_the_oldest_frame() {
    let rig = jitter_rig();
    assert_eq!(tick(&rig), None, "пусто: заполнение");
    arrive(&rig, 1_000);
    assert_eq!(tick(&rig), None, "один кадр — запаса ещё нет");
    arrive(&rig, 2_000);
    assert_eq!(tick(&rig), Some(1_000), "заполнен: старейший, а не самый свежий");
    assert_eq!(
        rig.stream.counters().viewfinder_queued,
        1,
        "второй кадр остался запасом"
    );

    let counters = rig.stream.counters();
    assert_eq!(counters.viewfinder_polls_while_priming, 2);
    assert_eq!(
        (
            counters.dropped_superseded,
            counters.dropped_jitter_overflow,
            counters.viewfinder_underruns
        ),
        (0, 0, 0)
    );
}

/// Разброс прихода меньше периода, поэтому тик с двумя приходами всегда соседствует с тиком без
/// прихода: запас, отданный в пустой тик, восполняет пара — оба её кадра показываются по очереди.
#[test]
fn two_frames_arriving_between_ticks_are_both_shown_on_consecutive_ticks() {
    let rig = jitter_rig();
    arrive(&rig, 1_000);
    arrive(&rig, 2_000);
    assert_eq!(tick(&rig), Some(1_000));
    // Тик без прихода (поздний кадр 3000): показан запас.
    assert_eq!(tick(&rig), Some(2_000));
    // Поздний 3000 и ранний 4000 пришли между одними тиками.
    arrive(&rig, 3_000);
    arrive(&rig, 4_000);
    assert_eq!(tick(&rig), Some(3_000));
    arrive(&rig, 5_000);
    assert_eq!(
        tick(&rig),
        Some(4_000),
        "второй из пары — на следующем тике, а не вытеснен"
    );
    arrive(&rig, 6_000);
    assert_eq!(tick(&rig), Some(5_000));

    let counters = rig.stream.counters();
    assert_eq!(counters.dropped_superseded, 0);
    assert_eq!(counters.dropped_jitter_overflow, 0);
    assert_eq!(counters.viewfinder_underruns, 0);
    assert_eq!(counters.viewfinder_delivered, 5);
}

#[test]
fn a_tick_without_arrival_shows_the_reserve_frame_instead_of_repeating() {
    let rig = jitter_rig();
    arrive(&rig, 1_000);
    arrive(&rig, 2_000);
    assert_eq!(tick(&rig), Some(1_000));
    // Тик без прихода: показывается запас, повтора нет.
    assert_eq!(tick(&rig), Some(2_000));
    assert_eq!(rig.stream.counters().viewfinder_underruns, 0);
    // Запас исчерпан: следующий пустой тик — настоящий повтор, и он посчитан.
    assert_eq!(tick(&rig), None);
    assert_eq!(rig.stream.counters().viewfinder_underruns, 1);
    // Заново ждать заполнения не нужно: пришедший кадр выдаётся сразу.
    arrive(&rig, 3_000);
    assert_eq!(tick(&rig), Some(3_000));
    assert_eq!(rig.stream.counters().viewfinder_polls_while_priming, 0);
}

#[test]
fn overflow_releases_the_oldest_unused_frame_with_its_own_counter() {
    let rig = jitter_rig();
    arrive(&rig, 1_000);
    arrive(&rig, 2_000);
    arrive(&rig, 3_000); // очередь 3 > max_queued 2: 1000 отпущен
    assert!(rig.log.is_returned(1_000), "переполнение вернуло буфер камере сразу");
    assert_eq!(rig.log.fence_count(1_000), Some(0), "GPU его не касался");
    let counters = rig.stream.counters();
    assert_eq!(counters.dropped_jitter_overflow, 1);
    assert_eq!(
        counters.dropped_superseded, 0,
        "переполнение — не вытеснение LatestWins"
    );
    assert_eq!(tick(&rig), Some(2_000));
    assert_eq!(tick(&rig), Some(3_000));
}

#[test]
fn a_late_publication_is_reordered_if_unshown_and_dropped_if_a_newer_frame_was_shown() {
    let rig = jitter_rig();
    rig.offer_frame(1_000, 0); // ждёт пару
    arrive(&rig, 2_000); // опубликован раньше 1000
    rig.offer_result(1_000, 0); // 1000 опубликован позже, но встаёт перед 2000
    assert_eq!(tick(&rig), Some(1_000));
    assert_eq!(tick(&rig), Some(2_000));

    rig.offer_frame(3_000, 0); // ждёт пару
    arrive(&rig, 4_000);
    arrive(&rig, 5_000);
    assert_eq!(tick(&rig), Some(4_000), "3000 ещё не опубликован");
    rig.offer_result(3_000, 0); // старше уже показанного 4000: назад во времени не показываем
    assert!(rig.log.is_returned(3_000));
    assert_eq!(rig.stream.counters().dropped_superseded, 1);
    assert_eq!(tick(&rig), Some(5_000));
}

#[test]
fn a_new_epoch_flushes_the_buffer_and_primes_it_again() {
    let mut rig = jitter_rig();
    arrive(&rig, 1_000);
    arrive(&rig, 2_000);
    assert_eq!(tick(&rig), Some(1_000));
    rig.restart();
    assert!(rig.log.is_returned(2_000), "запас прошлой эпохи вернулся камере");
    let counters = rig.stream.counters();
    assert_eq!((counters.viewfinder_flushed, counters.viewfinder_queued), (1, 0));

    arrive(&rig, 3_000);
    assert_eq!(tick(&rig), None, "новая эпоха заново заполняет буфер");
    arrive(&rig, 4_000);
    assert_eq!(tick(&rig), Some(3_000));

    rig.producer.end_stream(rig.epoch);
    assert!(rig.log.is_returned(4_000));
    assert!(matches!(
        rig.stream.viewfinder().poll_for_vsync(rig.at(0)),
        ViewfinderPoll::StreamInactive
    ));
    let counters = rig.stream.counters();
    assert_eq!(counters.viewfinder_flushed, 2);
    assert_eq!((counters.frames_outstanding, counters.leaked_leases), (0, 0));
}

#[test]
fn switching_policies_mid_stream_keeps_every_lease_accounted_and_never_repeats_a_frame() {
    let rig = jitter_rig();
    assert_eq!(rig.stream.viewfinder_pacing(), ViewfinderPacing::JitterBuffer);
    arrive(&rig, 1_000);
    arrive(&rig, 2_000);
    // Буфер → LatestWins: остаётся самый новый, старший вытеснен.
    assert_eq!(
        rig.stream.set_viewfinder_pacing(ViewfinderPacing::LatestWins),
        ViewfinderPacingSwitch::Switched
    );
    assert!(rig.log.is_returned(1_000));
    assert_eq!(tick(&rig), Some(2_000));
    assert_eq!(
        rig.stream.set_viewfinder_pacing(ViewfinderPacing::LatestWins),
        ViewfinderPacingSwitch::AlreadyActive
    );

    // LatestWins → буфер: кадр ячейки переходит в очередь, буфер заполняется заново.
    arrive(&rig, 3_000);
    assert_eq!(
        rig.stream.set_viewfinder_pacing(ViewfinderPacing::JitterBuffer),
        ViewfinderPacingSwitch::Switched
    );
    assert_eq!(tick(&rig), None, "заполнение после переключения");
    arrive(&rig, 4_000);
    assert_eq!(tick(&rig), Some(3_000));
    assert_eq!(tick(&rig), Some(4_000));

    rig.producer.end_stream(rig.epoch);
    let counters = rig.stream.counters();
    assert_eq!(counters.viewfinder_pacing_switches, 2);
    assert_eq!(counters.viewfinder_delivered, 3);
    assert_eq!(counters.dropped_superseded, 1);
    assert_eq!(rig.log.returned_count(), 4, "все буферы вернулись");
    assert!(
        rig.log.duplicate_returns().is_empty(),
        "ни один буфер не вернулся дважды"
    );
    assert_eq!((counters.frames_outstanding, counters.leaked_leases), (0, 0));
}

#[test]
fn reserve_and_queue_length_come_from_the_config() {
    let rig = Rig::new(RawFrameStreamConfig {
        viewfinder_jitter_buffer: JitterBufferConfig {
            reserve: 2,
            max_queued: 3,
        },
        ..RawFrameStreamConfig::default()
    });
    arrive(&rig, 1_000);
    arrive(&rig, 2_000);
    assert_eq!(tick(&rig), None, "запас 2: нужно три кадра");
    arrive(&rig, 3_000);
    assert_eq!(tick(&rig), Some(1_000));
    arrive(&rig, 4_000);
    arrive(&rig, 5_000); // очередь 4 > 3
    assert_eq!(rig.stream.counters().dropped_jitter_overflow, 1);
    assert_eq!(rig.start.pool.viewfinder, 3 + 2, "план пула растёт вместе с очередью");
}

// ---- дрейф часов камеры и экрана ----

/// Итог прогона дрейфа: показанные метки по порядку и счётчики потока.
struct DriftRun {
    shown: Vec<i64>,
    ticks: u64,
    arrived: u64,
    counters: crate::counters::RawStreamCounters,
}

/// xorshift64*: детерминированное дрожание без внешних crate-ов.
pub(super) struct TestRandom(pub(super) u64);

impl TestRandom {
    pub(super) fn next_unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        let value = self.0.wrapping_mul(0x2545_F491_4F6C_DD1D);
        (value >> 11) as f64 / (1_u64 << 53) as f64
    }
}

/// Камера `camera_hz` с возрастом при приходе равномерно в [27; 43] мс (факт OnePlus 11),
/// экран `display_hz`; опрос раз в тик, `seconds` секунд.
fn simulate_drift(config: RawFrameStreamConfig, camera_hz: f64, display_hz: f64, seconds: f64) -> DriftRun {
    let rig = Rig::new(config);
    let mut random = TestRandom(0x5EED_0053);
    let camera_period = 1e9 / camera_hz;
    let display_period = 1e9 / display_hz;
    let mut next_frame = 0_u64;
    let next_arrival = |random: &mut TestRandom, index: u64| {
        let sensor = (index as f64 * camera_period).round() as i64;
        let age = 27e6 + random.next_unit() * 16e6;
        (sensor, sensor + age.round() as i64)
    };
    let mut pending = next_arrival(&mut random, next_frame);
    let ticks = (seconds * display_hz) as u64;
    let mut shown = Vec::new();
    for tick_index in 1..=ticks {
        let tick_at = (tick_index as f64 * display_period).round() as i64;
        while pending.1 <= tick_at {
            arrive(&rig, pending.0);
            next_frame += 1;
            pending = next_arrival(&mut random, next_frame);
        }
        shown.extend(tick(&rig));
    }
    DriftRun {
        shown,
        ticks,
        arrived: next_frame,
        counters: rig.stream.counters(),
    }
}

/// Решение 53: при камере 60,03 к/с и экране 60,00 Гц буфер теряет кадры только из-за дрейфа часов
/// (~1 кадр за 33 с), а повторов почти нет. `LatestWins` на том же дрожании теряет каждый пятый-
/// шестой кадр — ради этого буфер и введён.
#[test]
fn camera_clock_faster_than_display_drops_only_the_drift_and_counts_it() {
    let seconds = 300.0;
    let run = simulate_drift(RawFrameStreamConfig::default(), 60.03, 60.00, seconds);
    let drift_frames = (0.03 * seconds) as u64; // 9 лишних кадров за 5 минут
    let counters = run.counters;
    assert!(
        run.shown.windows(2).all(|pair| pair[0] < pair[1]),
        "показ строго вперёд, без повторной выдачи"
    );
    assert_eq!(counters.dropped_superseded, 0, "вытеснений «самым свежим» нет");
    assert!(
        (drift_frames.saturating_sub(1)..=drift_frames + 2).contains(&counters.dropped_jitter_overflow),
        "переполнений {} при дрейфе {drift_frames}",
        counters.dropped_jitter_overflow
    );
    assert!(
        counters.viewfinder_underruns <= 2,
        "повторов {}",
        counters.viewfinder_underruns
    );
    // Каждый тик что-то показал, кроме заполнения на старте и редких повторов.
    let repeats = run.ticks - run.shown.len() as u64;
    assert_eq!(
        repeats,
        counters.viewfinder_polls_while_priming + counters.viewfinder_underruns
    );
    // Баланс: пришедшие = показанные + переполнение + лежащие в очереди.
    assert_eq!(
        run.arrived,
        run.shown.len() as u64 + counters.dropped_jitter_overflow + counters.viewfinder_queued
    );

    let latest_wins = simulate_drift(latest_wins_config(), 60.03, 60.00, seconds).counters;
    let superseded_ratio = latest_wins.dropped_superseded as f64 / latest_wins.frames_published as f64;
    assert!(
        superseded_ratio > 0.05,
        "при этом дрожании LatestWins должен терять заметно больше: {superseded_ratio:.3}"
    );
}

#[test]
fn camera_clock_slower_than_display_repeats_only_the_drift_and_counts_it() {
    let seconds = 300.0;
    let run = simulate_drift(RawFrameStreamConfig::default(), 59.97, 60.00, seconds);
    let drift_frames = (0.03 * seconds) as u64;
    let counters = run.counters;
    assert!(
        (drift_frames.saturating_sub(1)..=drift_frames + 2).contains(&counters.viewfinder_underruns),
        "повторов {} при дрейфе {drift_frames}",
        counters.viewfinder_underruns
    );
    assert!(counters.dropped_jitter_overflow <= 2);
    assert_eq!(counters.dropped_superseded, 0);
}

#[test]
fn sequences_handed_out_by_the_buffer_strictly_increase() {
    let rig = jitter_rig();
    let mut last = None::<FrameSequence>;
    for timestamp in (1..=20).map(|index| index * 1_000) {
        arrive(&rig, timestamp);
        if let ViewfinderPoll::NewFrame(lease) = rig.stream.viewfinder().poll_for_vsync(rig.at(0)) {
            let sequence = lease.passport().sequence;
            assert!(last.is_none_or(|previous| previous < sequence));
            last = Some(sequence);
            lease.release_unused();
        }
    }
    assert_eq!(last, Some(FrameSequence(18)), "в запасе остался последний кадр");
}
