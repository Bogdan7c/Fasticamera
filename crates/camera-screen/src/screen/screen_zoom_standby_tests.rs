//! Сквозные тесты спроса на тёплого соседа (13b): настоящие касания через арену жестов и кадры. Проверяется то,
//! что уходит приложению: тап по ленте тёплую камеру не будит, нажатие дольше порога будит, колесо держит спрос
//! и снимает его, когда уходит.

use camera_contract::ids::LensRole;
use camera_contract::zoom::{LensIntent, StandbyDemand};
use shell_contract::touch::TouchPhase;
use ui_core::time::MotionPreference;

use super::test_rig::{FRAME_MS, Rig};
use crate::effect::Effect;

fn demands(effects: &[Effect]) -> Vec<StandbyDemand> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Lens(LensIntent { standby, .. }) => Some(*standby),
            _ => None,
        })
        .collect()
}

/// Кадры по `duration_ms`; эффекты забираются каждый кадр, как это делает приложение.
fn run_collecting(rig: &mut Rig, duration_ms: i64) -> Vec<Effect> {
    let mut effects = Vec::new();
    for _ in 0..(duration_ms / FRAME_MS).max(1) {
        rig.run(FRAME_MS);
        effects.extend(rig.effects());
    }
    effects
}

fn strip_button(rig: &Rig) -> ui_core::geometry::Point {
    rig.center_with(|geometry| geometry.zoom_button(0, 1))
}

#[test]
fn a_quick_tap_on_the_strip_never_wakes_the_warm_camera() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    rig.tap(strip_button(&rig));
    let mut effects = rig.effects();
    effects.extend(run_collecting(&mut rig, 600));
    let seen = demands(&effects);
    assert_eq!(
        seen,
        [StandbyDemand::NotNeeded],
        "один тап — одно намерение и без спроса: {seen:?}"
    );
    assert_eq!(rig.screen.diagnostics().standby_demand_transitions, 0);
}

#[test]
fn holding_the_strip_past_the_tap_threshold_wakes_it_once_and_releasing_drops_it() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let point = strip_button(&rig);
    rig.touch(TouchPhase::Began, point, rig.now_ms);
    rig.effects();
    // 100 мс — ещё тап: спроса нет.
    let early = run_collecting(&mut rig, 100);
    assert!(demands(&early).is_empty(), "{early:?}");
    // К 250 мс порог 200 мс пройден, а долгое нажатие колеса (500 мс) ещё нет: спрос ровно один.
    let later = run_collecting(&mut rig, 150);
    assert_eq!(demands(&later), [StandbyDemand::KeepNeighbourWarm]);
    assert!(!rig.screen.state().zoom().is_wheel_open());
    rig.touch(TouchPhase::Ended, point, rig.now_ms);
    let released = run_collecting(&mut rig, 64);
    assert_eq!(
        demands(&released).last(),
        Some(&StandbyDemand::NotNeeded),
        "{released:?}"
    );
    assert_eq!(rig.screen.diagnostics().standby_demand_transitions, 2);
}

#[test]
fn a_long_press_that_opens_the_wheel_keeps_the_demand_until_the_wheel_leaves() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.run(32);
    let point = strip_button(&rig);
    rig.touch(TouchPhase::Began, point, rig.now_ms);
    let held = run_collecting(&mut rig, 700);
    assert!(
        rig.screen.state().zoom().is_wheel_open(),
        "долгое нажатие открыло колесо"
    );
    assert_eq!(
        demands(&held).last(),
        Some(&StandbyDemand::KeepNeighbourWarm),
        "{held:?}"
    );
    assert!(
        !demands(&held).contains(&StandbyDemand::NotNeeded),
        "спрос не мигал: {held:?}"
    );
    rig.touch(TouchPhase::Ended, point, rig.now_ms);
    // Колесо ещё на экране: спрос держится.
    let lingering = run_collecting(&mut rig, 1000);
    assert!(rig.screen.state().zoom().is_wheel_open());
    assert!(
        !demands(&lingering).contains(&StandbyDemand::NotNeeded),
        "{lingering:?}"
    );
    // Колесо ушло по покою ≈ 2,15 с: спрос снят одним переходом.
    let after = run_collecting(&mut rig, 1500);
    assert!(!rig.screen.state().zoom().is_wheel_open());
    assert_eq!(demands(&after), [StandbyDemand::NotNeeded]);
}
