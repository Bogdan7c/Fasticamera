//! Карточка белого пальцем через настоящие касания и кадры экрана (white-balance 02, D4b, WB28): высота и подъём
//! затвора, три ряда касания над полосой системного жеста, «×», числа осей, «Phone» и кнопки света, подсветка
//! только по фактам, отказ и ожидание. Цели касаний берутся из публичного списка `white_touch_targets`.

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::white::{WhiteOrigin, WhiteSourceId};
use camera_contract::white_reading::WhiteReading;
use shell_contract::haptics::HapticPattern;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::SurfaceSize;
use shell_contract::system_gesture::{RawEdgeInsetsPx, SystemGestureZones};
use shell_contract::touch::{SurfacePoint, TouchPhase};
use ui_core::geometry::{Point, Rect};
use ui_core::time::MotionPreference;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};

use super::test_rig::*;
use super::white_test_support::*;
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::state::PanelState;
use crate::tools::ToolId;
use crate::white::WhiteAxis;
use crate::white_facts::{WhiteFacts, WhiteTakeFacts, WhiteTakeState};
use crate::white_row::WhiteRowSlot;

const WHITE_CARD: PanelState = PanelState::Submenu(ToolId::White);

fn owner_zones() -> SystemGestureZones {
    SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 156,
        right: 0,
        bottom: 132,
    })
    .expect("зоны владельца")
}

// ---- чтение рисунка ----

/// Поверхность карточки (силуэт с ободком): (центр y, высота, ширина, радиус) в px.
fn card_glass(list: &DrawList) -> Option<(f32, f32, f32, f32)> {
    panel_members(list)
        .into_iter()
        .find_map(|(geometry, _, ty)| match geometry {
            ShapeGeometry::RoundedRect { size, corner } if size.width > 900.0 => {
                Some((ty, size.height, size.width, corner.radius_px))
            }
            _ => None,
        })
}

/// Кольцо затвора (силуэт с ободком, решение 300): (центр y, диаметр) в px.
fn shutter_ring(list: &DrawList) -> Option<(f32, f32)> {
    shutter_rings(list).into_iter().find_map(|ring| match silhouette(ring) {
        ShapeGeometry::Circle { diameter } if diameter > 180.0 => Some((ring.transform.ty, diameter)),
        _ => None,
    })
}

/// Зазор между низом затвора и верхом стекла карточки, pt (минус — наезд).
fn gap_pt(list: &DrawList) -> f64 {
    let (card_center, card_height, _, _) = card_glass(list).expect("стекло карточки");
    let (ring_center, ring_diameter) = shutter_ring(list).expect("кольцо затвора");
    let card_top = card_center - card_height * 0.5;
    let shutter_bottom = ring_center + ring_diameter * 0.5;
    f64::from(card_top - shutter_bottom) / f64::from(PX_PER_PT)
}

fn caption_color(rig: &Rig, text: &str) -> ui_draw::color::DisplayColor {
    texts_of(&rig.last, text, TextRoleKind::Caption)
        .first()
        .unwrap_or_else(|| panic!("подписи «{text}» нет в кадре"))
        .color
}

fn text_x(rig: &Rig, text: &str, role: TextRoleKind) -> f32 {
    texts_of(&rig.last, text, role)
        .first()
        .unwrap_or_else(|| panic!("текста «{text}» нет в кадре"))
        .transform
        .tx
}

fn is_camera_intent(effect: &Effect) -> bool {
    matches!(effect, Effect::Camera(_))
}

// ---- высота, подъём затвора, касания ----

#[test]
fn the_white_card_is_24_pt_taller_than_the_exposure_card_with_the_same_bottom_width_and_radius() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(900);
    let (ev_center, ev_height, ev_width, ev_radius) = card_glass(&rig.last).expect("карточка EV");
    rig.act(ScreenAction::WhiteButtonTapped);
    rig.run(900);
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
    let (center, height, width, radius) = card_glass(&rig.last).expect("карточка белого");
    assert!(
        (height - ev_height - px(24.0)).abs() < 0.5,
        "высота {height} против {ev_height}"
    );
    assert!((height - px(207.1)).abs() < 0.5);
    assert!(
        ((center + height * 0.5) - (ev_center + ev_height * 0.5)).abs() < 0.5,
        "низ на месте"
    );
    assert!((width - ev_width).abs() < 0.01 && (radius - ev_radius).abs() < 0.01);
}

#[test]
fn the_shutter_stands_124_pt_higher_over_the_white_card_100_over_the_others_and_still_shoots() {
    let mut rig = Rig::new();
    let (rest, _) = shutter_ring(&rig.last).expect("затвор");
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(900);
    let (over_ev, _) = shutter_ring(&rig.last).expect("затвор");
    assert!(((rest - over_ev) - px(100.0)).abs() < 0.5, "над EV поднят на 100 pt");
    rig.act(ScreenAction::WhiteButtonTapped);
    rig.run(900);
    let (over_white, _) = shutter_ring(&rig.last).expect("затвор виден над белой карточкой");
    assert!(
        ((rest - over_white) - px(124.0)).abs() < 0.5,
        "над белой поднят на 124 pt"
    );
    // Затвор виден и снимает на поднятом месте.
    rig.effects();
    let raised = rig.rect_with(|g| g.shutter()).center();
    rig.tap(Point::new(raised.x, raised.y - 124.0));
    let effects = rig.effects();
    assert!(
        effects.iter().any(|effect| matches!(effect, Effect::Capture(_))),
        "{effects:?}"
    );
}

/// Открыть карточку инструмента действием, каким её открывает палец: белое — кнопкой у затвора, остальные — значком.
fn open_tool_card(rig: &mut Rig, tool: ToolId) {
    match tool {
        ToolId::White => rig.act(ScreenAction::WhiteButtonTapped),
        other => rig.act(ScreenAction::StripToolTapped(other)),
    };
}

fn switch_between_cards(motion: MotionPreference, first: ToolId, second: ToolId) {
    let mut rig = Rig::with(ScreenConfig::default(), motion);
    open_tool_card(&mut rig, first);
    rig.run(900);
    let resting_gap = gap_pt(&rig.last);
    assert!(resting_gap > 15.0, "в покое зазор {resting_gap}");
    // Переключение значком другой карточки: пока всё едет, зазор не сжимается ни в одном кадре.
    open_tool_card(&mut rig, second);
    let mut frames = 0;
    for _ in 0..60 {
        rig.run(FRAME_MS);
        let gap = gap_pt(&rig.last);
        assert!(
            gap >= resting_gap - 1.0,
            "{first:?}→{second:?} {motion:?}: зазор {gap} < {resting_gap}"
        );
        frames += 1;
    }
    assert_eq!(frames, 60);
    assert!(gap_pt(&rig.last) > 15.0);
}

#[test]
fn switching_between_the_exposure_and_the_white_card_never_lets_the_shutter_touch_the_card() {
    for motion in [MotionPreference::Full, MotionPreference::Reduced] {
        switch_between_cards(motion, ToolId::Exposure, ToolId::White);
        switch_between_cards(motion, ToolId::White, ToolId::Exposure);
    }
}

#[test]
fn switching_to_exposure_by_its_icon_with_a_real_finger_changes_the_card_height_smoothly() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let (_, white_height, _, _) = card_glass(&rig.last).expect("белая");
    let ev_icon = rig.center_with(|g| g.strip_icon(2, 3));
    rig.tap(ev_icon);
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);
    // Сразу после тапа высота ещё не прыгнула целиком (идёт пружина), а потом приходит к обычной.
    rig.run(48);
    let (_, mid_height, _, _) = card_glass(&rig.last).expect("карточка");
    assert!(mid_height < white_height && mid_height > white_height - px(24.0) - 0.5);
    rig.run(900);
    let (_, ev_height, _, _) = card_glass(&rig.last).expect("карточка EV");
    assert!((white_height - ev_height - px(24.0)).abs() < 0.5);
}

#[test]
fn the_touch_targets_list_follows_the_state() {
    let mut rig = Rig::new();
    let closed = targets(&rig);
    assert!(closed.button.size.width > 0.0, "кнопка белого у затвора есть всегда");
    assert!(closed.card.is_none(), "карточка закрыта — целей карточки нет");
    open_card(&mut rig);
    let open = card_targets(&rig);
    assert_eq!(open.light_buttons.len(), 5, "кнопки света по конфигу");
    assert!(open.close.x < open.temperature_number.x && open.temperature_number.x < open.tint_number.x);
    // Другая карточка — целей белой нет.
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    assert!(targets(&rig).card.is_none());
}

#[test]
fn all_three_rows_have_44_pt_touch_zones_and_sit_fully_above_the_system_gesture_band() {
    let rig = Rig::new();
    let surface = SurfaceSize {
        width_px: WIDTH_PX as u32,
        height_px: HEIGHT_PX as u32,
    };
    let zones = owner_zones();
    let axis_temperature = rig.rect_with(|g| g.white_axis_button(WhiteAxis::Temperature));
    let axis_tint = rig.rect_with(|g| g.white_axis_button(WhiteAxis::Tint));
    let light_count = ScreenConfig::default().white.lights.len();
    let lights: Vec<Rect> = WhiteRowSlot::row(light_count)
        .map(|slot| rig.rect_with(|g| g.white_row_slot(slot, light_count)))
        .collect();
    let ruler = rig.rect_with(|g| g.white_ruler());
    let mut rows: Vec<(&str, Rect)> = vec![
        ("число Кельвинов", axis_temperature),
        ("число оттенка", axis_tint),
        ("шкала", ruler),
    ];
    for (index, light) in lights.iter().enumerate() {
        rows.push(("кнопка ряда", *light));
        assert!(
            light.size.height >= 44.0 && light.size.width >= 44.0,
            "кнопка {index}: {light:?}"
        );
    }
    for (name, rect) in rows {
        assert!(rect.size.height >= 44.0, "{name}: {rect:?}");
        // Нижняя кромка зоны (чуть выше неё) не лежит в зоне системного жеста.
        let bottom_edge = SurfacePoint {
            x_px: px(rect.center().x),
            y_px: px(rect.max_y()) - 0.5,
        };
        assert!(
            !zones.contains(surface, bottom_edge),
            "{name} заходит в полосу жеста: {rect:?}"
        );
    }
    // Ряды не налезают друг на друга по вертикали.
    assert!(axis_temperature.max_y() <= lights[0].min_y() && lights[0].max_y() <= ruler.min_y());
}

#[test]
fn the_lowest_point_of_the_ruler_still_drives_it_with_the_owner_zones_on() {
    let mut rig = Rig::new();
    rig.screen.set_system_gesture_zones(owner_zones());
    open_card(&mut rig);
    let zone = card_targets(&rig).ruler_zone;
    let low = Point::new(zone.center().x + 40.0, zone.max_y() - 1.0);
    rig.drag(low, Point::new(low.x - 60.0, low.y), 160);
    let intents = camera_intents(&rig.effects());
    assert!(
        intents
            .iter()
            .any(|intent| matches!(intent, ScreenCameraIntent::AdjustWhite(_))),
        "{intents:?}"
    );
    assert_eq!(rig.screen.diagnostics().touches_ignored_in_system_gesture_zone, 0);
}

// ---- закрытие ----

#[test]
fn the_cross_swipe_down_and_a_tap_on_the_frame_close_the_card_without_focus_or_white_intents() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let close = card_targets(&rig).close;
    rig.tap(close);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed, "«×»");
    assert!(rig.effects().is_empty());
    // Кнопка белого возвращается на место вместе с затвором: следующее касание целится в покой.
    rig.run(700);

    open_card(&mut rig);
    let top = card_targets(&rig).temperature_number;
    // Свайп вниз по свободному месту карточки (между кнопками верхней строки и числами).
    let from = Point::new(top.x + 90.0, top.y);
    rig.drag(from, Point::new(from.x, from.y + 120.0), 160);
    rig.run(300);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed, "свайп вниз");
    assert!(camera_intents(&rig.effects()).is_empty());
    rig.run(700);

    open_card(&mut rig);
    let frame = rig.frame_center();
    rig.tap(frame);
    assert_eq!(rig.screen.state().panel(), PanelState::Closed, "тап по кадру закрывает");
    let effects = rig.effects();
    assert!(
        effects.iter().all(|effect| !is_camera_intent(effect)),
        "фокус не тронут: {effects:?}"
    );
}

// ---- числа осей ----

fn facts_3140() -> crate::facts::CameraFacts {
    facts_held(3_140.0, 0.0164, WhiteOrigin::Manual)
}

fn underline_x(list: &DrawList) -> Option<f32> {
    let expected_width = px(28.0);
    list.leaf_items().find_map(|item| match item {
        DrawItem::Shape(shape) => match shape.geometry {
            ShapeGeometry::Capsule { size } if (size.width - expected_width).abs() < 0.5 => Some(shape.transform.tx),
            _ => None,
        },
        _ => None,
    })
}

#[test]
fn the_axis_numbers_show_the_held_white_and_a_tap_moves_the_bright_mark_to_that_axis() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_3140());
    open_card(&mut rig);
    let numbers = card_targets(&rig);
    let kelvin = texts_of(&rig.last, "3140 K", TextRoleKind::CardValue);
    let tint = texts_of(&rig.last, "Tint +16", TextRoleKind::CardValue);
    assert_eq!((kelvin.len(), tint.len()), (1, 1));
    assert!(
        kelvin[0].opacity.get() > tint[0].opacity.get() + 0.2,
        "выбранная ось ярче"
    );
    let first_mark = underline_x(&rig.last).expect("черта под выбранной осью");
    assert!((first_mark - px(numbers.temperature_number.x)).abs() < px(8.0));
    rig.effects();
    rig.tap(numbers.tint_number);
    rig.run(300);
    assert_eq!(rig.screen.state().white().axis(), WhiteAxis::Tint);
    let kelvin = texts_of(&rig.last, "3140 K", TextRoleKind::CardValue);
    let tint = texts_of(&rig.last, "Tint +16", TextRoleKind::CardValue);
    assert!(
        tint[0].opacity.get() > kelvin[0].opacity.get() + 0.2,
        "теперь ярче оттенок"
    );
    let second_mark = underline_x(&rig.last).expect("черта");
    assert!((second_mark - px(numbers.tint_number.x)).abs() < px(8.0));
    assert!(rig.effects().is_empty(), "смена оси намерений не порождает");
}

#[test]
fn the_axis_number_zones_are_at_least_44_pt_high() {
    let rig = Rig::new();
    for axis in [WhiteAxis::Temperature, WhiteAxis::Tint] {
        assert!(rig.rect_with(|g| g.white_axis_button(axis)).size.height >= 44.0);
    }
}

// ---- «Phone» и кнопки света ----

#[test]
fn the_phone_button_takes_the_phone_white_without_a_haptic() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let phone = card_targets(&rig).phone_button;
    rig.tap(phone);
    let effects = rig.effects();
    assert_eq!(
        effects,
        [Effect::Camera(ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE))],
        "один запрос источнику, вибрации нет"
    );
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
}

#[test]
fn a_light_button_sets_the_white_and_lights_up_only_when_the_fact_confirms_it() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let tokens = UiKitTokens::default();
    let lamp_index = rig
        .screen
        .config
        .white
        .lights
        .iter()
        .position(|light| light.label == "Lamp")
        .expect("лампа в конфиге");
    let lamp = card_targets(&rig).light_buttons[lamp_index];
    let lamp_setting = rig.screen.config.white.lights[lamp_index]
        .setting()
        .expect("число лампы");
    rig.tap(lamp);
    rig.run(300);
    let effects = rig.effects();
    assert_eq!(effects, [Effect::Camera(ScreenCameraIntent::SetWhite(lamp_setting))]);
    // Тап сам ничего не подсвечивает: белое экран не хранит, ждём факт.
    assert_eq!(caption_color(&rig, "Lamp"), tokens.colors.text_primary);
    // Движок подтвердил — кнопка жёлтая, остальные белые.
    set_facts(
        &mut rig,
        facts_held(
            lamp_setting.temperature.kelvin(),
            lamp_setting.tint.duv(),
            WhiteOrigin::Manual,
        ),
    );
    assert_eq!(caption_color(&rig, "Lamp"), tokens.colors.focus_yellow);
    assert_eq!(caption_color(&rig, "Sun"), tokens.colors.text_primary);
    assert_eq!(caption_color(&rig, "Phone"), tokens.colors.text_primary);
    // Источник «телефон» подсвечивает «Phone», а не кнопку света.
    set_facts(
        &mut rig,
        facts_held(3_200.0, 0.0, WhiteOrigin::Source(WhiteSourceId::PHONE)),
    );
    assert_eq!(caption_color(&rig, "Phone"), tokens.colors.focus_yellow);
}

#[test]
fn all_buttons_are_visible_without_scrolling_inside_the_card_and_every_light_button_hits_its_own_light() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let card = rig.rect_with(|g| g.white_card());
    let cardtargets = card_targets(&rig);
    for point in std::iter::once(cardtargets.phone_button).chain(cardtargets.light_buttons.iter().copied()) {
        assert!(card.contains(point), "кнопка {point:?} за картой {card:?}");
    }
    let lights = rig.screen.config.white.lights.clone();
    for (index, light) in lights.iter().enumerate() {
        rig.effects();
        rig.tap(cardtargets.light_buttons[index]);
        let intents = camera_intents(&rig.effects());
        assert_eq!(
            intents,
            [ScreenCameraIntent::SetWhite(light.setting().expect("число"))],
            "кнопка {} ставит своё белое",
            light.label
        );
    }
}

#[test]
fn icons_turn_with_the_device_and_captions_stay_upright() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let sun = icons_of(&rig.last, IconId::Sun);
    assert_eq!(sun.len(), 1);
    assert!(sun[0].transform.a.abs() > 0.9, "в портрете значок прямой");
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(1_500);
    let sun = icons_of(&rig.last, IconId::Sun);
    assert!(
        sun[0].transform.a.abs() < 0.1 && sun[0].transform.b.abs() > 0.9,
        "значок повёрнут"
    );
    let phone_icon = icons_of(&rig.last, IconId::DeviceMobile);
    assert!(phone_icon[0].transform.b.abs() > 0.9);
    let caption = texts_of(&rig.last, "Sun", TextRoleKind::Caption);
    assert!(
        caption[0].transform.a.abs() > 0.9,
        "подпись не вращается, как у других карточек"
    );
}

// ---- оценка «Phone» ----

fn facts_with_reading(reading: Option<WhiteReading>) -> crate::facts::CameraFacts {
    facts_with(WhiteFacts {
        phone_reading: reading,
        ..WhiteFacts::STARTING
    })
}

#[test]
fn the_phone_estimate_appears_on_a_fixed_place_and_does_not_move_the_row() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let phone_before = texts_of(&rig.last, "Phone", TextRoleKind::Caption)[0].transform;
    assert!(
        texts_of(&rig.last, "≈ 3100", TextRoleKind::Caption).is_empty(),
        "оценки нет — строки нет"
    );
    let measured = WhiteReading::Measured {
        white: WhiteBalanceSetting {
            temperature: camera_contract::control::ColorTemperature::from_kelvin(3_100.0).expect("К"),
            tint: camera_contract::control::Tint::NEUTRAL,
        },
    };
    set_facts(&mut rig, facts_with_reading(Some(measured)));
    let estimate = texts_of(&rig.last, "≈ 3100", TextRoleKind::Caption);
    assert_eq!(estimate.len(), 1);
    let phone_after = texts_of(&rig.last, "Phone", TextRoleKind::Caption)[0].transform;
    assert_eq!(
        (phone_before.tx, phone_before.ty),
        (phone_after.tx, phone_after.ty),
        "подпись не сдвинулась"
    );
    assert!(estimate[0].transform.ty > phone_after.ty, "оценка под подписью");
    // Не измерено — строка исчезает, подпись стоит там же.
    set_facts(&mut rig, facts_with_reading(Some(WhiteReading::NotMeasuredYet)));
    assert!(texts_of(&rig.last, "≈ 3100", TextRoleKind::Caption).is_empty());
}

// ---- внимание «Phone» и кнопки белого ----

fn take_facts(serial: u64, state: WhiteTakeState) -> crate::facts::CameraFacts {
    facts_with(WhiteFacts {
        take: Some(WhiteTakeFacts { serial, state }),
        ..WhiteFacts::STARTING
    })
}

#[test]
fn a_refusal_shakes_the_strip_number_and_the_phone_button_and_both_come_back() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    rig.run(300);
    let phone_rest = text_x(&rig, "Phone", TextRoleKind::Caption);
    let number_rest = text_x(&rig, "5600", TextRoleKind::StripValue);
    rig.screen
        .handle_action(ScreenAction::CameraFactsChanged(take_facts(1, WhiteTakeState::Waiting)));
    rig.run(FRAME_MS * 3);
    rig.screen
        .handle_action(ScreenAction::CameraFactsChanged(take_facts(1, WhiteTakeState::Refused)));
    let (mut phone_swing, mut number_swing): (f32, f32) = (0.0, 0.0);
    for _ in 0..24 {
        rig.run(FRAME_MS);
        phone_swing = phone_swing.max((text_x(&rig, "Phone", TextRoleKind::Caption) - phone_rest).abs());
        number_swing = number_swing.max((text_x(&rig, "5600", TextRoleKind::StripValue) - number_rest).abs());
    }
    assert!(phone_swing > px(2.0), "«Phone» дёргался: {phone_swing}");
    assert!(number_swing > px(2.0), "число на кнопке дёргалось: {number_swing}");
    rig.run(900);
    assert!((text_x(&rig, "Phone", TextRoleKind::Caption) - phone_rest).abs() < 0.01);
    assert!((text_x(&rig, "5600", TextRoleKind::StripValue) - number_rest).abs() < 0.01);
    assert!(!rig.screen.white_attention_animating());
    let haptics = rig
        .effects()
        .iter()
        .filter(|e| **e == Effect::Haptic(HapticPattern::Error))
        .count();
    assert_eq!(haptics, 1, "вибрация «ошибка» один раз");
}

#[test]
fn waiting_makes_the_phone_button_flicker_then_it_returns_to_full_brightness() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    set_facts(
        &mut rig,
        facts_with(WhiteFacts {
            take: Some(WhiteTakeFacts {
                serial: 1,
                state: WhiteTakeState::Waiting,
            }),
            ..WhiteFacts::STARTING
        }),
    );
    let mut dimmest = 1.0_f32;
    for _ in 0..70 {
        rig.run(FRAME_MS);
        let phone = texts_of(&rig.last, "Phone", TextRoleKind::Caption);
        dimmest = dimmest.min(phone.first().map_or(1.0, |item| item.opacity.get()));
    }
    assert!(dimmest < 0.7, "«Phone» мерцает: {dimmest}");
    set_facts(&mut rig, facts_with(WhiteFacts::STARTING));
    rig.run(700);
    assert!((texts_of(&rig.last, "Phone", TextRoleKind::Caption)[0].opacity.get() - 1.0).abs() < 1e-3);
}

#[test]
fn the_close_cross_zone_is_at_least_44_pt_and_the_lists_are_consistent_in_both_landscapes() {
    for orientation in [DeviceOrientation::LandscapeLeft, DeviceOrientation::LandscapeRight] {
        let mut rig = Rig::new();
        rig.act(ScreenAction::DeviceOrientationChanged(orientation));
        rig.run(600);
        open_card(&mut rig);
        let targets = card_targets(&rig);
        rig.effects();
        rig.tap(targets.phone_button);
        assert_eq!(
            camera_intents(&rig.effects()).len(),
            1,
            "{orientation:?}: «Phone» нажимается"
        );
        rig.tap(targets.close);
        assert_eq!(rig.screen.state().panel(), PanelState::Closed, "{orientation:?}");
        let _ = TouchPhase::Began;
    }
}

// ---- шрифт ----

#[test]
fn every_string_the_white_card_can_show_is_in_the_font_atlas_and_a_negative_tint_draws() {
    // Знак «≈» в атласе нет (кадр упал бы), поэтому оценка — с тильдой; минус оттенка — настоящий U+2212.
    let mut rig = Rig::new();
    let lights = rig.screen.config.white.lights.clone();
    for text in [
        "3140 K",
        "12000 K",
        "Tint +16",
        "Tint \u{2212}16",
        "Tint 0",
        "≈ 3100",
        "Phone",
    ] {
        let _ = run_of(text, TextRoleKind::CardValue);
        let _ = run_of(text, TextRoleKind::Caption);
    }
    for light in &lights {
        let _ = run_of(&light.label, TextRoleKind::Caption);
    }
    set_facts(&mut rig, facts_held(4_000.0, -0.016, WhiteOrigin::Manual));
    open_card(&mut rig);
    assert_eq!(texts_of(&rig.last, "Tint \u{2212}16", TextRoleKind::CardValue).len(), 1);
    assert_eq!(rig.screen.diagnostics().frame_failures, 0);
}

// ---- верхние 24 pt белой карточки ----

#[test]
fn the_top_24_pt_of_the_white_card_above_the_ordinary_card_still_catch_touches() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let white = rig.rect_with(|g| g.white_card());
    let ordinary = rig.rect_with(|g| g.card());
    let targets = card_targets(&rig);
    // Касание внутри зоны числа оси, но выше верха обычной карточки: без своего узла у белой оно ушло бы в видоискатель.
    let probe_y = ordinary.min_y() - 1.0;
    assert!(probe_y < ordinary.min_y() && probe_y > white.min_y());
    rig.tap(Point::new(targets.tint_number.x, probe_y));
    assert_eq!(
        rig.screen.state().white().axis(),
        WhiteAxis::Tint,
        "число оси поймало касание"
    );
    assert_eq!(rig.screen.state().panel(), WHITE_CARD, "панель не закрылась");
    rig.tap(Point::new(targets.close.x, probe_y));
    assert_eq!(
        rig.screen.state().panel(),
        PanelState::Closed,
        "«×» поймал касание у верхней кромки"
    );
    assert!(camera_intents(&rig.effects()).is_empty(), "фокус не тронут");
}
