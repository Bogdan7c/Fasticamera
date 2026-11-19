//! Окно удержания блокировки в сценарии `3a` (приёмка 08c): вибрация в начале и конце, размах замера сцены,
//! догон цели. Помощники стенда (`run_three_a_with`, `short_own_config`) — из родительского модуля.

use std::time::{Duration, Instant};

use camera_contract::three_a::AeAfLock;

use super::*;

#[test]
fn lock_hold_is_framed_by_two_haptic_cues_and_reports_a_static_scene_and_the_catch_up() {
    let mut first_cue_lock = None;
    let (harness, result) = run_three_a_with(
        short_own_config(|three_a| three_a.lock.hold_ms = 400),
        HalFaults::default(),
        |_| {},
        |harness, _| {
            // В момент первого сигнала блокировка уже должна стоять: сигнал означает «окно началось».
            let played = !harness.shell.haptics_played.lock().expect("замок").is_empty();
            if played && first_cue_lock.is_none() {
                first_cue_lock = Some(harness.rig().handle().status().three_a.lock);
            }
        },
    );

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        first_cue_lock,
        Some(AeAfLock::Locked),
        "сигнал начала подан до блокировки"
    );
    assert_eq!(
        *harness.shell.haptics_played.lock().expect("замок"),
        [
            shell_contract::haptics::HapticPattern::HeavyImpact,
            shell_contract::haptics::HapticPattern::Success
        ],
        "первый сигнал — начало окна, второй — конец, и других вибраций в сценарии нет"
    );
    let lock = &result["report"]["lock"];
    let queued = serde_json::json!({"outcome": "queued", "realization": "waveform"});
    assert_eq!(lock["haptic_hold_start"], queued, "{lock}");
    assert_eq!(lock["haptic_hold_end"], queued, "{lock}");
    assert_eq!(
        lock["observed_scene_range_ev"].as_f64(),
        Some(0.0),
        "сцена не менялась: размах замера нулевой, и отчёт это показывает: {lock}"
    );
    assert!(
        lock["product_frozen_after_frames"]
            .as_i64()
            .is_some_and(|frames| (0..=60).contains(&frames)),
        "произведение замерло за разумное число кадров после команды: {lock}"
    );
    let stages = harness.output.stage_names();
    assert_eq!(stages.iter().filter(|stage| *stage == "three_a_lock_cue").count(), 2);
}

#[test]
fn scene_change_during_the_hold_shows_up_as_observed_scene_range_in_ev() {
    let mut cue_at: Option<Instant> = None;
    let mut scene_changed = false;
    let (_harness, result) = run_three_a_with(
        short_own_config(|three_a| three_a.lock.hold_ms = 800),
        HalFaults::default(),
        |_| {},
        |harness, render_ae| {
            if cue_at.is_none() && !harness.shell.haptics_played.lock().expect("замок").is_empty() {
                cue_at = Some(Instant::now());
            }
            // Через 300 мс после сигнала «закрывают объектив»: сцена светлеет в 8 раз (+3 EV), как под рукой
            // не бывает (темнее), но размах от направления не зависит.
            let due = cue_at.is_some_and(|at| at.elapsed() >= Duration::from_millis(300));
            if let (true, false, Some(render_ae)) = (due, scene_changed, render_ae) {
                scene_changed = true;
                render_ae.change_scene(16.0);
            }
        },
    );

    assert!(
        scene_changed,
        "окно удержания не началось — менять сцену было не во что"
    );
    assert_eq!(result["status"], "passed", "{result}");
    let range = result["report"]["lock"]["observed_scene_range_ev"]
        .as_f64()
        .expect("размах замера сцены");
    // Сцена 2.0 -> 16.0 с·ISO: ровно +3 EV.
    assert!((range - 3.0).abs() < 0.01, "размах {range} EV вместо 3: {result}");
}

#[test]
fn haptics_that_cannot_play_are_reported_in_the_lock_report_and_do_not_fail_the_run() {
    use shell_contract::haptics::HapticOutcome;
    let mut forced = false;
    let (_harness, result) = run_three_a_with(
        short_own_config(|_| {}),
        HalFaults::default(),
        |_| {},
        |harness, _| {
            if !forced {
                forced = true;
                *harness.shell.haptics_outcome.lock().expect("замок") = Some(HapticOutcome::Unsupported);
            }
        },
    );

    assert_eq!(
        result["status"], "passed",
        "вибромотор — не предмет проверки камеры: {result}"
    );
    let lock = &result["report"]["lock"];
    assert_eq!(
        lock["haptic_hold_start"],
        serde_json::json!({"outcome": "unsupported"}),
        "{lock}"
    );
    assert_eq!(
        lock["haptic_hold_end"],
        serde_json::json!({"outcome": "unsupported"}),
        "{lock}"
    );
}
