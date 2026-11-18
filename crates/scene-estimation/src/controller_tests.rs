//! Контроллер белой точки: сходимость, профили, блокировка, ручной режим, слабые оценки, шаг времени.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use isp_model::color::white_point::{Duv, LocusBand};

use super::*;
use crate::estimate::RawEstimate;
use crate::estimator::finalize_estimate;

/// Кадр 60 к/с в наносекундах.
const FRAME_NANOS: i64 = 16_666_667;

fn controller() -> WhiteBalanceController {
    WhiteBalanceController::new(SceneEstimationConfig::default()).expect("конфиг по умолчанию верен")
}

fn band() -> LocusBand {
    LocusBand::new(Kelvin(2000.0), Kelvin(12000.0), Duv(0.02)).expect("полоса")
}

/// Оценка света `kelvin` с заданной уверенностью и числом плиток (через настоящий скалярный шаг CPU).
fn estimate(kelvin: f64, confidence: f32, tiles: u32) -> EstimateOutcome {
    let raw = RawEstimate {
        mired: Kelvin(kelvin).mired() as f32,
        duv: 0.0,
        confidence,
        accepted_tiles: tiles,
        eligible_tiles: 600,
    };
    finalize_estimate(raw, band()).expect("оценка переводится в цветность")
}

// Майред оценки проходит через f32 (как выход шейдера), поэтому сравнения белой — с допуском 1e-3 майред.
fn confident(kelvin: f64) -> EstimateOutcome {
    estimate(kelvin, 0.5, 200)
}

fn directive(mode: WhiteBalanceMode, profile: ThreeAProfile) -> WhiteBalanceDirective {
    WhiteBalanceDirective { mode, profile }
}

fn manual(kelvin: f64, duv: f64) -> WhiteBalanceMode {
    WhiteBalanceMode::Manual {
        setting: WhiteBalanceSetting {
            temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
            tint: Tint::from_duv(duv).expect("Duv"),
        },
    }
}

/// Прогоняет `frames` кадров 60 к/с; возвращает майред сообщённой белой после каждого и число сообщений об изменении.
fn run_frames(controller: &mut WhiteBalanceController, clock: &mut i64, frames: usize) -> (Vec<f64>, usize) {
    let mut trace = Vec::new();
    let mut changes = 0;
    for _ in 0..frames {
        *clock += FRAME_NANOS;
        let step = controller.advance(*clock).expect("шаг");
        changes += usize::from(step.changed);
        trace.push(step.white.expect("белая определена").cct_duv.temperature.mired());
    }
    (trace, changes)
}

/// Контроллер, уже стоящий на белой `kelvin` (первая оценка принимается прыжком).
fn settled_at(kelvin: f64, profile: ThreeAProfile) -> (WhiteBalanceController, i64) {
    let mut controller = controller();
    controller.set_directive(directive(WhiteBalanceMode::Auto, profile));
    assert_eq!(controller.ingest(&confident(kelvin)), IngestOutcome::Accepted);
    let clock = 0;
    let step = controller.advance(clock).expect("шаг");
    assert!(step.changed, "первая белая — это изменение");
    (controller, clock)
}

#[test]
fn step_from_6500_to_2800_converges_monotonically_without_overshoot_in_the_configured_time() {
    let (mut controller, mut clock) = settled_at(6500.0, ThreeAProfile::Photo);
    let start = Kelvin(6500.0).mired();
    let target = Kelvin(2800.0).mired();
    controller.ingest(&confident(2800.0));
    // 0,5 с (30 кадров): ещё в пути — не мгновенный прыжок.
    let (early, _) = run_frames(&mut controller, &mut clock, 30);
    let at_half_second = *early.last().expect("кадры");
    assert!(
        at_half_second < target - 5.0,
        "через 0,5 с ещё не дошла: {at_half_second}"
    );
    // До 1,0 с (ещё 30 кадров): τ = 0,25 с ⇒ остаток 203·e^(−4) ≈ 3,7 майред — в допуске 5.
    let (later, _) = run_frames(&mut controller, &mut clock, 30);
    let trace: Vec<f64> = early.iter().chain(&later).copied().collect();
    let final_mired = *trace.last().expect("кадры");
    assert!(
        (final_mired - target).abs() <= 5.0,
        "за 1 с дошла до {final_mired}, цель {target}"
    );
    assert!(trace.windows(2).all(|pair| pair[1] >= pair[0]), "движение монотонно");
    assert!(
        trace.iter().all(|&mired| mired <= target + 1e-9 && mired >= start),
        "без перелёта"
    );
}

#[test]
fn photo_profile_is_faster_than_video_profile() {
    let frames_to_get_within_20_mired = |profile| {
        let (mut controller, mut clock) = settled_at(6500.0, profile);
        controller.ingest(&confident(2800.0));
        let target = Kelvin(2800.0).mired();
        for frame in 1..=2000 {
            clock += FRAME_NANOS;
            let mired = controller
                .advance(clock)
                .expect("шаг")
                .white
                .expect("белая")
                .cct_duv
                .temperature
                .mired();
            if (mired - target).abs() <= 20.0 {
                return frame;
            }
        }
        panic!("не дошла за 2000 кадров");
    };
    let photo = frames_to_get_within_20_mired(ThreeAProfile::Photo);
    let video = frames_to_get_within_20_mired(ThreeAProfile::Video);
    assert!(
        video > photo * 4,
        "видео заметно медленнее фото: {video} против {photo} кадров"
    );
}

#[test]
fn steady_scene_stops_reporting_changes_so_the_isp_is_not_re_resolved() {
    let (mut controller, mut clock) = settled_at(6500.0, ThreeAProfile::Photo);
    controller.ingest(&confident(2800.0));
    let (_, moving_changes) = run_frames(&mut controller, &mut clock, 60);
    assert!(moving_changes >= 2, "при движении сообщения идут: {moving_changes}");
    let (_, settling_changes) = run_frames(&mut controller, &mut clock, 300);
    let _ = settling_changes;
    let (_, steady_changes) = run_frames(&mut controller, &mut clock, 120);
    assert_eq!(steady_changes, 0, "в покое белая не сообщается заново");
    // Новая оценка внутри порога сообщения не рождает изменения.
    controller.ingest(&confident(2800.5));
    let (_, after_tiny_change) = run_frames(&mut controller, &mut clock, 60);
    assert_eq!(after_tiny_change, 0, "сдвиг 0,06 майред меньше порога 0,5");
}

#[test]
fn slow_video_profile_still_reports_every_threshold_crossing() {
    // Шаг за кадр у «Видео» меньше порога 0,5 майред в конце движения: сообщения не должны пропасть.
    let (mut controller, mut clock) = settled_at(6500.0, ThreeAProfile::Video);
    controller.ingest(&confident(5500.0));
    let (trace, changes) = run_frames(&mut controller, &mut clock, 60 * 12);
    assert!(
        changes > 5,
        "медленное движение сообщается ступенями, а не молчит: {changes}"
    );
    let target = Kelvin(5500.0).mired();
    assert!((trace.last().expect("кадры") - target).abs() <= 1.0);
}

#[test]
fn locked_freezes_the_white_and_ignores_new_estimates() {
    let (mut controller, mut clock) = settled_at(6500.0, ThreeAProfile::Photo);
    controller.set_directive(directive(WhiteBalanceMode::Locked, ThreeAProfile::Photo));
    assert_eq!(controller.ingest(&confident(2800.0)), IngestOutcome::IgnoredLocked);
    let (trace, changes) = run_frames(&mut controller, &mut clock, 120);
    let frozen = Kelvin(6500.0).mired();
    assert!(
        trace.iter().all(|&mired| (mired - frozen).abs() < 1e-3),
        "белая не сдвинулась"
    );
    assert_eq!(changes, 0);
    assert_eq!(controller.counters().ignored_locked, 1);
    // Снятие блокировки возвращает слежение за светом.
    controller.set_directive(directive(WhiteBalanceMode::Auto, ThreeAProfile::Photo));
    controller.ingest(&confident(2800.0));
    let (after, _) = run_frames(&mut controller, &mut clock, 120);
    assert!((after.last().expect("кадры") - Kelvin(2800.0).mired()).abs() < 5.0);
}

#[test]
fn lock_on_a_fresh_stream_catches_the_first_confident_estimate_and_holds_it() {
    let mut controller = controller();
    controller.set_directive(directive(WhiteBalanceMode::Locked, ThreeAProfile::Photo));
    assert!(
        controller.advance(0).expect("шаг").white.is_none(),
        "блокировать пока нечего"
    );
    assert_eq!(controller.ingest(&confident(4000.0)), IngestOutcome::Accepted);
    assert_eq!(controller.ingest(&confident(2800.0)), IngestOutcome::IgnoredLocked);
    let held = controller.advance(FRAME_NANOS).expect("шаг").white.expect("белая");
    assert!(
        (held.cct_duv.temperature.0 - 4000.0).abs() < 1.0,
        "держится первая: {held:?}"
    );
}

#[test]
fn manual_reaches_the_setting_and_snaps_on_a_fresh_stream() {
    let mut fresh = controller();
    fresh.set_directive(directive(manual(3200.0, 0.004), ThreeAProfile::Photo));
    let first = fresh.advance(0).expect("шаг");
    let white = first.white.expect("ручная точка сразу задаёт белую");
    assert!(first.changed);
    assert!((white.cct_duv.temperature.0 - 3200.0).abs() < 1e-6 && (white.cct_duv.duv.0 - 0.004).abs() < 1e-12);

    let (mut controller, mut clock) = settled_at(6500.0, ThreeAProfile::Video);
    controller.set_directive(directive(manual(3200.0, 0.004), ThreeAProfile::Video));
    let (trace, _) = run_frames(&mut controller, &mut clock, 60);
    let target = Kelvin(3200.0).mired();
    assert!(
        (trace.last().expect("кадры") - target).abs() < 1.0,
        "ручная точка достигнута за секунду даже в профиле «Видео»"
    );
    assert!(controller.manual_limit().is_none(), "точка внутри таблицы");
}

#[test]
fn manual_outside_the_table_is_limited_and_reports_what_was_applied() {
    let mut controller = controller();
    controller.set_directive(directive(manual(1500.0, 0.09), ThreeAProfile::Photo));
    let limit = controller.manual_limit().expect("ограничение есть");
    assert_eq!((limit.requested_kelvin, limit.applied_kelvin), (1500.0, 2000.0));
    assert_eq!((limit.requested_duv, limit.applied_duv), (0.09, 0.05));
    let white = controller.advance(0).expect("шаг").white.expect("белая");
    assert!(
        (white.cct_duv.temperature.0 - 2000.0).abs() < 1e-6,
        "применена прижатая точка"
    );
    controller.set_directive(directive(WhiteBalanceMode::Auto, ThreeAProfile::Photo));
    assert!(
        controller.manual_limit().is_none(),
        "вне ручного режима ограничения нет"
    );
}

#[test]
fn weak_estimates_do_not_move_the_white() {
    let (mut controller, mut clock) = settled_at(6500.0, ThreeAProfile::Photo);
    let low_confidence = estimate(2800.0, 0.05, 200);
    assert!(matches!(
        controller.ingest(&low_confidence),
        IngestOutcome::HeldLowConfidence { .. }
    ));
    let few_tiles = estimate(2800.0, 0.9, 3);
    assert!(matches!(
        controller.ingest(&few_tiles),
        IngestOutcome::HeldLowConfidence { .. }
    ));
    let nothing = EstimateOutcome::NoCandidates { eligible_tiles: 300 };
    assert_eq!(controller.ingest(&nothing), IngestOutcome::HeldNoCandidates);
    let (trace, changes) = run_frames(&mut controller, &mut clock, 120);
    assert!(trace.iter().all(|&mired| (mired - Kelvin(6500.0).mired()).abs() < 1e-3));
    assert_eq!(changes, 0);
    let counters = controller.counters();
    assert_eq!(
        (
            counters.held_low_confidence,
            counters.held_no_candidates,
            counters.accepted
        ),
        (2, 1, 1)
    );
}

#[test]
fn first_estimate_without_a_scene_white_is_taken_as_is_not_smoothed_from_nowhere() {
    let mut controller = controller();
    assert!(!controller.has_scene_white());
    controller.ingest(&confident(2800.0));
    assert!(controller.has_scene_white());
    let white = controller.advance(0).expect("шаг").white.expect("белая");
    assert!((white.cct_duv.temperature.0 - 2800.0).abs() < 1.0);
}

#[test]
fn time_steps_come_from_sensor_timestamps_and_are_guarded() {
    let (mut controller, _) = settled_at(6500.0, ThreeAProfile::Photo);
    controller.ingest(&confident(2800.0));
    let before = controller.current_state().expect("белая").mired;
    // Метка раньше предыдущей (смена потока) и повтор той же метки: шаг ноль.
    controller.advance(-5_000_000_000).expect("шаг");
    controller.advance(-5_000_000_000).expect("шаг");
    assert_eq!(
        controller.current_state().expect("белая").mired,
        before,
        "отрицательный и нулевой dt не двигают"
    );
    // Огромный скачок вперёд обрезается до max_step (0,25 с = τ): доля 1 − e^(−1).
    controller.advance(1_000_000_000_000).expect("шаг");
    let moved = controller.current_state().expect("белая").mired - before;
    let target = Kelvin(2800.0).mired();
    let expected = (target - before) * (1.0 - (-1.0_f64).exp());
    assert!((moved - expected).abs() < 1e-3, "сдвиг {moved}, ожидалось {expected}");
}

#[test]
fn stream_change_keeps_the_scene_white_and_restarts_the_clock() {
    let (mut controller, _) = settled_at(2800.0, ThreeAProfile::Photo);
    controller.begin_stream();
    assert!(
        controller.has_scene_white(),
        "белая сцены переживает смену камеры (решение 65)"
    );
    // Первый кадр нового потока имеет произвольную метку: dt = 0, цели нет — белая стоит.
    let step = controller.advance(9_000_000_000_000).expect("шаг");
    assert!(!step.changed);
    let white = step.white.expect("белая");
    assert!((white.cct_duv.temperature.0 - 2800.0).abs() < 1.0);
}

#[test]
fn restored_white_is_the_starting_point_and_best_effort_works_only_while_there_is_no_white() {
    let mut restored = controller();
    restored.restore_scene_white(WhiteState {
        mired: Kelvin(5000.0).mired(),
        duv: 0.002,
    });
    assert!(restored.has_scene_white());
    let white = restored.advance(0).expect("шаг").white.expect("белая");
    assert!((white.cct_duv.temperature.0 - 5000.0).abs() < 1.0 && (white.cct_duv.duv.0 - 0.002).abs() < 1e-9);
    // Повторное восстановление не затирает действующую белую.
    restored.restore_scene_white(WhiteState { mired: 100.0, duv: 0.0 });
    assert!((restored.current_state().expect("белая").mired - Kelvin(5000.0).mired()).abs() < 1e-9);

    let weak = match estimate(3000.0, 0.05, 4) {
        EstimateOutcome::Estimated(weak) => weak,
        EstimateOutcome::NoCandidates { .. } => panic!("кандидаты есть"),
    };
    let mut fresh = controller();
    assert!(fresh.adopt_best_effort(&weak), "белой нет — слабая оценка берётся");
    assert!(!fresh.adopt_best_effort(&weak), "белая уже есть — второй раз нет");
    assert_eq!(
        fresh.counters().accepted,
        0,
        "принудительная оценка — не принятая по порогу"
    );
    // Уверенная оценка потом ведёт белую плавно, а не прыжком.
    fresh.advance(0).expect("шаг");
    fresh.ingest(&confident(6500.0));
    let step = fresh.advance(FRAME_NANOS).expect("шаг").white.expect("белая");
    let (from, to) = (Kelvin(3000.0).mired(), Kelvin(6500.0).mired());
    let mired = step.cct_duv.temperature.mired();
    assert!(mired < from && mired > to, "первый шаг к цели, не прыжок: {mired}");
}

#[test]
fn out_of_order_frame_does_not_count_the_same_time_twice() {
    // Ревью 08: кадр старше учтённого раньше откатывал метку, и следующий кадр засчитывал
    // тот же отрезок дважды — сглаживание шло быстрее, чем по времени сенсора.
    let mut reference = settled_at(6500.0, ThreeAProfile::Photo).0;
    let mut shuffled = settled_at(6500.0, ThreeAProfile::Photo).0;
    for controller in [&mut reference, &mut shuffled] {
        assert_eq!(controller.ingest(&confident(2800.0)), IngestOutcome::Accepted);
    }
    // Эталон: кадры 1, 2 по порядку. Перемешанный: 2, затем опоздавший 1, затем снова 2+.
    let reference_white = {
        reference.advance(FRAME_NANOS).expect("шаг");
        reference.advance(2 * FRAME_NANOS).expect("шаг")
    };
    let shuffled_white = {
        shuffled.advance(2 * FRAME_NANOS).expect("шаг");
        let late = shuffled.advance(FRAME_NANOS).expect("шаг");
        assert_eq!(
            late.white.map(|white| white.cct_duv.temperature.mired()),
            shuffled
                .advance(2 * FRAME_NANOS)
                .expect("шаг")
                .white
                .map(|white| white.cct_duv.temperature.mired()),
            "опоздавший и повторный кадр шага не дают"
        );
        late
    };
    let to_mired = |step: WhiteStep| step.white.expect("белая").cct_duv.temperature.mired();
    assert!(
        (to_mired(reference_white) - to_mired(shuffled_white)).abs() < 1e-6,
        "за одно и то же время сенсора белая сдвинулась одинаково"
    );
}
