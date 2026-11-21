//! Спрос экрана на тёплого соседа (13b, решения 117 и 124): когда уходит `KeepNeighbourWarm`, когда `NotNeeded`
//! и что последнее намерение всегда несёт актуальный спрос.

use camera_contract::ids::LensRole;
use camera_contract::zoom::{LensIntent, StandbyDemand, ZoomGesture};

use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::test_support::{Screen, at_ms, ratio};

fn lens_intents(effects: &[Effect]) -> Vec<LensIntent> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            Effect::Lens(intent) => Some(*intent),
            _ => None,
        })
        .collect()
}

fn demands(effects: &[Effect]) -> Vec<StandbyDemand> {
    lens_intents(effects).iter().map(|intent| intent.standby).collect()
}

fn tick(screen: &mut Screen, ms: i64) -> Vec<Effect> {
    screen.step(ScreenAction::Tick(at_ms(ms))).effects
}

#[test]
fn a_long_press_on_the_strip_wakes_the_neighbour_only_after_the_tap_threshold() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let began = screen.step(ScreenAction::ZoomStripPressBegan { at: at_ms(1000) });
    assert!(lens_intents(&began.effects).is_empty(), "палец лёг: спроса ещё нет");
    // До порога (200 мс) — тишина.
    assert!(lens_intents(&tick(&mut screen, 1100)).is_empty());
    assert!(lens_intents(&tick(&mut screen, 1199)).is_empty());
    // Порог пройден: ровно одно намерение со спросом и текущими направлением и коэффициентом.
    let crossed = lens_intents(&tick(&mut screen, 1200));
    assert_eq!(crossed.len(), 1, "{crossed:?}");
    assert_eq!(crossed[0].standby, StandbyDemand::KeepNeighbourWarm);
    assert_eq!(crossed[0].gesture, ZoomGesture::Continuous);
    assert_eq!(crossed[0].ratio, screen.state.zoom().ratio());
    assert_eq!(crossed[0].facing, screen.state.zoom().facing());
    // Дальнейшие кадры спрос не повторяют.
    assert!(lens_intents(&tick(&mut screen, 1216)).is_empty());
    // Отпустили без колеса: спрос снят.
    let released = screen.step(ScreenAction::ZoomStripPressEnded { at: at_ms(1300) });
    assert_eq!(demands(&released.effects), [StandbyDemand::NotNeeded]);
}

#[test]
fn a_short_tap_never_asks_for_a_warm_neighbour() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let mut effects = screen
        .step(ScreenAction::ZoomStripPressBegan { at: at_ms(1000) })
        .effects;
    effects.extend(tick(&mut screen, 1100));
    effects.extend(
        screen
            .step(ScreenAction::ZoomStripPressEnded { at: at_ms(1120) })
            .effects,
    );
    let tapped = screen.step(ScreenAction::ZoomStopTapped {
        stop: 0,
        at: at_ms(1120),
    });
    effects.extend(tapped.effects);
    effects.extend(tick(&mut screen, 1400));
    let intents = lens_intents(&effects);
    assert_eq!(intents.len(), 1, "только само намерение тапа: {intents:?}");
    assert_eq!(intents[0].gesture, ZoomGesture::Discrete);
    assert_eq!(intents[0].standby, StandbyDemand::NotNeeded);
    assert_eq!(screen.state.zoom().standby_transitions(), 0);
}

#[test]
fn the_wheel_keeps_the_demand_through_value_changes_and_drops_it_when_it_closes_idle() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let touched = screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(1000) });
    assert_eq!(demands(&touched.effects), [StandbyDemand::KeepNeighbourWarm]);
    // Изменения значения несут тот же спрос и не порождают отдельных переходов.
    let dragged = screen.step(ScreenAction::ZoomWheelDragged {
        requested: ratio(1.4),
        at: at_ms(1016),
    });
    let intents = lens_intents(&dragged.effects);
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert_eq!(intents[0].standby, StandbyDemand::KeepNeighbourWarm);
    screen.step(ScreenAction::ZoomWheelReleased { at: at_ms(1100) });
    // Колесо ещё на экране (покой < 2,15 с): спрос держится.
    assert!(lens_intents(&tick(&mut screen, 2000)).is_empty());
    assert!(screen.state.zoom().is_wheel_open());
    // Колесо ушло сразу по порогу покоя: ровно один переход в «не нужно».
    let closed = tick(&mut screen, 3300);
    assert!(!screen.state.zoom().is_wheel_open());
    assert_eq!(demands(&closed), [StandbyDemand::NotNeeded]);
    assert_eq!(screen.state.zoom().standby_transitions(), 2);
}

#[test]
fn a_pinch_asks_from_its_start_to_its_end() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    let began = screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(1000) });
    assert_eq!(demands(&began.effects), [StandbyDemand::KeepNeighbourWarm]);
    let changed = screen.step(ScreenAction::ZoomPinchChanged {
        scale: 1.5,
        at: at_ms(1016),
    });
    let intents = lens_intents(&changed.effects);
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert_eq!(intents[0].standby, StandbyDemand::KeepNeighbourWarm);
    // Колесо при щипке не открывалось: конец щипка снимает спрос.
    let ended = screen.step(ScreenAction::ZoomPinchEnded { at: at_ms(1100) });
    assert_eq!(demands(&ended.effects), [StandbyDemand::NotNeeded]);
}

#[test]
fn a_stop_tap_while_the_wheel_is_open_carries_the_current_demand() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(1000) });
    let tapped = screen.step(ScreenAction::ZoomStopTapped {
        stop: 2,
        at: at_ms(1050),
    });
    let intents = lens_intents(&tapped.effects);
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert_eq!(intents[0].gesture, ZoomGesture::Discrete);
    assert_eq!(intents[0].standby, StandbyDemand::KeepNeighbourWarm);
}

#[test]
fn flipping_to_the_front_camera_drops_the_demand_in_the_flip_intent_itself() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(1000) });
    assert_eq!(
        screen.state.zoom().published_standby_demand(),
        StandbyDemand::KeepNeighbourWarm
    );
    let flipped = screen.step(ScreenAction::FlipCameraRequested { at: at_ms(1100) });
    let intents = lens_intents(&flipped.effects);
    // Одно намерение — само переворота (Discrete), и оно уже несёт «не нужно»: отдельного перехода нет.
    assert_eq!(intents.len(), 1, "{intents:?}");
    assert_eq!(intents[0].gesture, ZoomGesture::Discrete);
    assert_eq!(intents[0].standby, StandbyDemand::NotNeeded);
}

#[test]
fn the_front_camera_never_asks_for_a_neighbour() {
    let mut screen = Screen::zoomed(LensRole::Wide);
    screen.step(ScreenAction::FlipCameraRequested { at: at_ms(1000) });
    let before = screen.state.zoom().standby_transitions();
    let began = screen.step(ScreenAction::ZoomPinchBegan { at: at_ms(1100) });
    let touched = screen.step(ScreenAction::ZoomWheelTouched { at: at_ms(1200) });
    assert!(demands(&began.effects).is_empty(), "{:?}", began.effects);
    assert!(demands(&touched.effects).is_empty(), "{:?}", touched.effects);
    assert_eq!(screen.state.zoom().standby_transitions(), before);
    assert_eq!(screen.state.zoom().published_standby_demand(), StandbyDemand::NotNeeded);
}
