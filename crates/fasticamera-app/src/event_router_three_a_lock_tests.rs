//! Окно удержания блокировки в сценарии `3a` (приёмка 08c): отсчёт, подсказки в начале и конце, размах замера сцены,
//! догон цели. Помощники стенда (`run_three_a_with`, `short_own_config`) — из родительского модуля.

use std::time::{Duration, Instant};

use camera_contract::three_a::AeAfLock;
use shell_contract::commands::CommandOutcome;
use shell_contract::operator_prompt::OperatorPromptKind;

use crate::operator_prompt_fake::PromptEvent;

use super::*;

#[test]
fn lock_hold_is_framed_by_prepare_act_done_prompts_and_reports_a_static_scene_and_the_catch_up() {
    let mut lock_when_act_shown = None;
    let (harness, result) = run_three_a_with(
        short_own_config(|three_a| three_a.lock.hold_ms = 400),
        HalFaults::default(),
        |_| {},
        |harness, _| {
            // В момент подсказки «закрой объектив» блокировка уже должна стоять: надпись означает «окно началось».
            let act_shown = harness.shell.prompts.shown_kinds().contains(&OperatorPromptKind::Act);
            if act_shown && lock_when_act_shown.is_none() {
                lock_when_act_shown = Some(harness.rig().handle().status().three_a.lock);
            }
        },
    );

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        lock_when_act_shown,
        Some(AeAfLock::Locked),
        "подсказка начала удержания показана до блокировки"
    );
    // Состояние на экране: отсчёт -> «закрой» на всё удержание -> «убери руку» -> снято.
    let events = harness.shell.prompts.events();
    let kinds: Vec<OperatorPromptKind> = harness.shell.prompts.shown_kinds();
    assert_eq!(
        kinds,
        vec![
            OperatorPromptKind::Prepare,
            OperatorPromptKind::Prepare,
            OperatorPromptKind::Act,
            OperatorPromptKind::Done
        ],
        "отсчёт 2 -> 1, затем закрыть, затем убрать: {events:?}"
    );
    let headlines: Vec<String> = harness
        .shell
        .prompts
        .shown()
        .iter()
        .map(|prompt| prompt.headline().to_owned())
        .collect();
    assert_eq!(headlines[0], "ПРИГОТОВЬСЯ: через 2 с");
    assert_eq!(headlines[1], "ПРИГОТОВЬСЯ: через 1 с");
    assert_eq!(headlines[2], "ЗАКРОЙ ОБЪЕКТИВ РУКОЙ — держи");
    assert_eq!(headlines[3], "УБЕРИ РУКУ");
    assert!(
        harness.shell.prompts.clear_count() == 1 && events.last() == Some(&PromptEvent::Cleared),
        "«УБЕРИ РУКУ» не снимается шагом сразу (мелькнула бы незаметно) и снимается один раз в конце сценария: {events:?}"
    );
    assert!(
        harness.shell.haptics_played.lock().expect("замок").is_empty(),
        "вибрация как сигнал владельцу больше не используется"
    );
    let lock = &result["report"]["lock"];
    for key in ["prompt_prepare", "prompt_hold_start", "prompt_hold_end"] {
        assert_eq!(lock[key], "queued", "{key}: {lock}");
    }
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
    // Стадии: первый шаг отсчёта, начало и конец удержания.
    assert_eq!(stages.iter().filter(|stage| *stage == "three_a_lock_cue").count(), 3);
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
            if cue_at.is_none() && harness.shell.prompts.shown_kinds().contains(&OperatorPromptKind::Act) {
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
fn prompts_that_cannot_be_shown_are_reported_in_the_lock_report_and_do_not_fail_the_run() {
    let mut forced = false;
    let (harness, result) = run_three_a_with(
        short_own_config(|_| {}),
        HalFaults::default(),
        |_| {},
        |harness, _| {
            if !forced {
                forced = true;
                harness.shell.prompts.set_outcome(Some(CommandOutcome::NotAttached));
            }
        },
    );

    assert_eq!(
        result["status"], "passed",
        "экран подсказок — не предмет проверки камеры (ноты ставит скрипт): {result}"
    );
    let lock = &result["report"]["lock"];
    for key in ["prompt_prepare", "prompt_hold_start", "prompt_hold_end"] {
        assert_eq!(lock[key], "not_attached", "{key}: {lock}");
    }
    assert!(
        harness.shell.prompts.clear_count() >= 1,
        "и недоставленная подсказка снимается"
    );
}

#[test]
fn lock_without_countdown_starts_the_hold_at_once_and_reports_no_prepare() {
    let (harness, result) = run_three_a_with(
        short_own_config(|three_a| three_a.lock.prepare_countdown_ms = 0),
        HalFaults::default(),
        |_| {},
        |_, _| {},
    );

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(
        harness.shell.prompts.shown_kinds(),
        vec![OperatorPromptKind::Act, OperatorPromptKind::Done]
    );
    assert!(result["report"]["lock"]["prompt_prepare"].is_null(), "{result}");
}

#[test]
fn lock_that_never_comes_clears_the_countdown_instead_of_leaving_it_on_screen() {
    let (harness, result) = run_three_a_with(
        // Вендорский AE: у фейкового HAL блокировку можно сломать (`ae_never_locks`); путь подсказок у шага общий.
        short_config(|_| {}),
        HalFaults {
            ae_never_locks: true,
            ..HalFaults::default()
        },
        |_| {},
        |_, _| {},
    );

    assert_eq!(result["report"]["lock"]["locked"], false, "{result}");
    let events = harness.shell.prompts.events();
    // Только отсчёт, без «ЗАКРОЙ»/«УБЕРИ»: удержания не было.
    assert!(
        harness
            .shell
            .prompts
            .shown_kinds()
            .iter()
            .all(|kind| *kind == OperatorPromptKind::Prepare),
        "{events:?}"
    );
    // Шаг снимает отсчёт сам (1) и маршрутизатор ещё раз в конце (2): без снятия шагом «через 1 с» висело бы
    // на экране все оставшиеся шаги.
    assert_eq!(harness.shell.prompts.clear_count(), 2, "{events:?}");
}
