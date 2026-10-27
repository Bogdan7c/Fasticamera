//! Свойство на 2000 кадрах: случайные приходы (парами, с пропусками, с опоздавшим результатом),
//! опросы рендера с удержанием кадров, переключения политики и смены эпохи. После каждого шага —
//! точный баланс ячейки видоискателя; в конце — каждый буфер вернулся камере ровно один раз.

use std::collections::VecDeque;

use camera_contract::frame_passport::FrameSequence;

use super::jitter_buffer::TestRandom;
use super::support::Rig;
use crate::config::RawFrameStreamConfig;
use crate::lease::RawFrameLease;
use crate::stream::ViewfinderPoll;
use crate::viewfinder_pacing::ViewfinderPacing;

const FRAMES: i64 = 2_000;
/// Сколько кадров поток рендера держит одновременно (показанный + только что взятый).
const RENDER_HOLDS: usize = 2;

/// Проверка баланса ячейки видоискателя (см. `viewfinder_cell`).
fn assert_viewfinder_balance(rig: &Rig, step: usize) {
    let counters = rig.stream.counters();
    assert_eq!(
        counters.frames_published,
        counters.viewfinder_delivered
            + counters.dropped_superseded
            + counters.dropped_jitter_overflow
            + counters.viewfinder_flushed
            + counters.viewfinder_queued,
        "шаг {step}: баланс ячейки нарушен: {counters:?}"
    );
    assert!(
        counters.frames_outstanding <= u64::from(rig.start.pool.viewfinder),
        "шаг {step}: лизингов {} больше ячеек плана пула {}",
        counters.frames_outstanding,
        rig.start.pool.viewfinder
    );
}

#[test]
fn randomized_stream_keeps_exact_accounting_and_returns_every_buffer_once() {
    let mut rig = Rig::new(RawFrameStreamConfig::default());
    let mut random = TestRandom(0xF00D_2000);
    let mut next_timestamp = 0_i64;
    let mut delayed_results: VecDeque<i64> = VecDeque::new();
    let mut held: VecDeque<RawFrameLease> = VecDeque::new();
    let mut last_handed: Option<FrameSequence> = None;
    let mut step = 0_usize;
    let mut restarts = 0_u32;

    while next_timestamp < FRAMES * 1_000 || !delayed_results.is_empty() {
        step += 1;
        // Приходы между тиками: 0, 1 или 2 кадра (разброс прихода меньше периода).
        let arrivals = if next_timestamp < FRAMES * 1_000 {
            (random.next_unit() * 3.0) as usize
        } else {
            0
        };
        for _ in 0..arrivals {
            if next_timestamp >= FRAMES * 1_000 {
                break;
            }
            next_timestamp += 1_000;
            if random.next_unit() < 0.1 {
                // Результат опоздает: кадр ждёт пару, публикация пойдёт не по порядку прихода.
                rig.offer_frame(next_timestamp, 0);
                delayed_results.push_back(next_timestamp);
            } else {
                rig.offer_result(next_timestamp, 0);
                rig.offer_frame(next_timestamp, 0);
            }
        }
        if random.next_unit() < 0.3
            && let Some(timestamp) = delayed_results.pop_front()
        {
            rig.offer_result(timestamp, 0);
        }

        // Тик: ровно один опрос; поток рендера держит не больше двух кадров.
        if let ViewfinderPoll::NewFrame(lease) = rig.stream.viewfinder().poll_for_vsync(rig.at(0)) {
            let sequence = lease.passport().sequence;
            assert!(
                last_handed.is_none_or(|previous| previous < sequence),
                "шаг {step}: кадр {sequence:?} выдан после {last_handed:?}"
            );
            last_handed = Some(sequence);
            held.push_back(lease);
            while held.len() > RENDER_HOLDS {
                if let Some(oldest) = held.pop_front() {
                    oldest.release_unused();
                }
            }
        }

        // Редкие события: переключение политики (A/B) и новая сессия камеры.
        let event = random.next_unit();
        if event < 0.01 {
            let pacing = if rig.stream.viewfinder_pacing() == ViewfinderPacing::JitterBuffer {
                ViewfinderPacing::LatestWins
            } else {
                ViewfinderPacing::JitterBuffer
            };
            rig.stream.set_viewfinder_pacing(pacing);
        } else if event < 0.015 {
            rig.restart();
            restarts += 1;
            // Опоздавшие результаты прошлой эпохи больше не нужны.
            delayed_results.clear();
        }
        assert_viewfinder_balance(&rig, step);
    }

    for lease in held.drain(..) {
        lease.release_unused();
    }
    rig.producer.end_stream(rig.epoch);
    assert_viewfinder_balance(&rig, step);

    let counters = rig.stream.counters();
    let offered = usize::try_from(counters.frames_offered).expect("счётчик");
    assert_eq!(offered, usize::try_from(FRAMES).expect("кадры"));
    assert_eq!(rig.log.returned_count(), offered, "каждый буфер вернулся камере");
    assert!(
        rig.log.duplicate_returns().is_empty(),
        "ни один буфер не вернулся дважды"
    );
    assert_eq!((counters.frames_outstanding, counters.leaked_leases), (0, 0));
    assert!(
        counters.viewfinder_pacing_switches > 0,
        "свойство обязано пройти через переключения"
    );
    assert!(restarts > 0, "и через смены эпохи");
    assert!(counters.viewfinder_delivered > 1_000, "основная масса кадров показана");
}
