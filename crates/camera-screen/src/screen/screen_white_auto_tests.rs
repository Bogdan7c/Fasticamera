//! Кнопка «Auto» (слежение за белым телефона, WB44, WB48, WB49) через настоящие касания и кадры экрана: тап шлёт
//! ровно одно намерение, подсветка и число у затвора считаются по ФАКТУ слежения, подписи помещаются в 60 pt и в
//! стеклянный круг, невручённая команда даёт вибрацию без тряски, места ряда не налезают друг на друга.

use camera_contract::white::{WhiteOrigin, WhiteSourceId};
use camera_contract::white_reading::WhiteReading;
use shell_contract::haptics::HapticPattern;
use ui_draw::icons::IconId;
use ui_kit::tokens::TextRoleKind;

use super::test_rig::*;
use super::white_test_support::*;
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::facts::CameraFacts;
use crate::state::PanelState;
use crate::tools::ToolId;
use crate::white_display::{AUTO_LABEL, AUTO_UNIT, KELVIN_UNIT, PHONE_LABEL, phone_estimate_text};
use crate::white_facts::{WhiteFacts, WhiteFollowFacts};
use crate::white_row::WhiteRowSlot;

const WHITE_CARD: PanelState = PanelState::Submenu(ToolId::White);

/// Факты с белым `held` и слежением `follow`.
fn follow_facts(kelvin: f64, duv: f64, origin: WhiteOrigin, follow: WhiteFollowFacts) -> CameraFacts {
    facts_with(WhiteFacts {
        held: held(kelvin, duv, origin),
        follow,
        ..WhiteFacts::STARTING
    })
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

// ---- тап ----

#[test]
fn a_tap_on_auto_sends_follow_the_phone_and_nothing_else_and_leaves_the_card_open() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let auto = card_targets(&rig).auto_button;
    rig.tap(auto);
    assert_eq!(
        rig.effects(),
        [Effect::Camera(ScreenCameraIntent::FollowWhite(WhiteSourceId::PHONE))],
        "одно намерение «следить», ни вибрации, ни «взять»"
    );
    assert_eq!(rig.screen.state().panel(), WHITE_CARD);
}

#[test]
fn a_repeated_tap_on_auto_while_follow_is_on_sends_the_same_harmless_intent_again() {
    // Экран режим не хранит: решает движок (ответит «уже так»), поэтому повторный тап уходит как первый.
    let mut rig = Rig::new();
    set_facts(
        &mut rig,
        follow_facts(5_600.0, 0.0, WhiteOrigin::Manual, WhiteFollowFacts::On),
    );
    open_card(&mut rig);
    let auto = card_targets(&rig).auto_button;
    rig.tap(auto);
    assert_eq!(
        camera_intents(&rig.effects()),
        [ScreenCameraIntent::FollowWhite(WhiteSourceId::PHONE)]
    );
}

#[test]
fn with_the_card_closed_the_place_of_auto_does_not_send_follow() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let auto = card_targets(&rig).auto_button;
    rig.tap(card_targets(&rig).close);
    rig.run(700);
    rig.effects();
    rig.tap(auto);
    assert!(
        !camera_intents(&rig.effects())
            .iter()
            .any(|intent| matches!(intent, ScreenCameraIntent::FollowWhite(_))),
        "закрытая карточка кнопок не ловит"
    );
}

// ---- подсветка по факту ----

#[test]
fn the_follow_fact_lights_auto_and_darkens_phone_and_every_light_even_when_their_numbers_match() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let tokens = ui_kit::tokens::UiKitTokens::default();
    let lamp = rig
        .screen
        .config
        .white
        .lights
        .iter()
        .find(|light| light.label == "Lamp")
        .expect("Lamp");
    let (lamp_kelvin, lamp_duv) = (lamp.kelvin, lamp.duv);
    // Белое взято у телефона и равно числам «Lamp»: без слежения горели бы «Phone» и «Lamp».
    set_facts(
        &mut rig,
        follow_facts(
            lamp_kelvin,
            lamp_duv,
            WhiteOrigin::Source(WhiteSourceId::PHONE),
            WhiteFollowFacts::Off,
        ),
    );
    assert_eq!(caption_color(&rig, PHONE_LABEL), tokens.colors.focus_yellow);
    assert_eq!(caption_color(&rig, "Lamp"), tokens.colors.focus_yellow);
    assert_eq!(caption_color(&rig, AUTO_LABEL), tokens.colors.text_primary);
    // Слежение включено: горит одна «Auto» (подпись и значок), остальные белые.
    set_facts(
        &mut rig,
        follow_facts(
            lamp_kelvin,
            lamp_duv,
            WhiteOrigin::Source(WhiteSourceId::PHONE),
            WhiteFollowFacts::On,
        ),
    );
    assert_eq!(caption_color(&rig, AUTO_LABEL), tokens.colors.focus_yellow);
    assert_eq!(
        icons_of(&rig.last, IconId::MagicWand)[0].color,
        tokens.colors.focus_yellow
    );
    assert_eq!(caption_color(&rig, PHONE_LABEL), tokens.colors.text_primary);
    for light in rig.screen.config.white.lights.clone() {
        assert_eq!(
            caption_color(&rig, &light.label),
            tokens.colors.text_primary,
            "{}",
            light.label
        );
    }
}

#[test]
fn a_tap_on_auto_lights_nothing_until_the_engine_reports_the_follow() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let tokens = ui_kit::tokens::UiKitTokens::default();
    let auto = card_targets(&rig).auto_button;
    rig.tap(auto);
    rig.run(300);
    assert_eq!(
        caption_color(&rig, AUTO_LABEL),
        tokens.colors.text_primary,
        "режима экран не хранит"
    );
}

// ---- число и подпись у затвора ----

#[test]
fn under_follow_the_shutter_button_shows_a_white_live_number_over_auto_instead_of_k() {
    let mut rig = Rig::new();
    let tokens = ui_kit::tokens::UiKitTokens::default();
    // Держимое происхождение ещё «ручное» (ответа источника на включение не было): без слежения число жёлтое.
    set_facts(
        &mut rig,
        follow_facts(3_200.0, 0.0, WhiteOrigin::Manual, WhiteFollowFacts::Off),
    );
    assert_eq!(
        texts_of(&rig.last, "3200", TextRoleKind::StripValue)[0].color,
        tokens.colors.focus_yellow
    );
    assert_eq!(text_count_of(&rig.last, KELVIN_UNIT, TextRoleKind::StripUnit), 1);
    assert_eq!(
        text_count_of(&rig.last, AUTO_UNIT, TextRoleKind::StripUnit),
        0,
        "«AUTO» только при слежении"
    );

    set_facts(
        &mut rig,
        follow_facts(3_200.0, 0.0, WhiteOrigin::Manual, WhiteFollowFacts::On),
    );
    let number = texts_of(&rig.last, "3200", TextRoleKind::StripValue);
    assert_eq!(number.len(), 1, "число живое: то, что держит движок");
    assert_eq!(number[0].color, tokens.colors.text_primary, "при слежении число белое");
    let unit = texts_of(&rig.last, AUTO_UNIT, TextRoleKind::StripUnit);
    assert_eq!(unit.len(), 1);
    assert_eq!(unit[0].color, tokens.colors.text_primary);
    assert_eq!(
        text_count_of(&rig.last, KELVIN_UNIT, TextRoleKind::StripUnit),
        0,
        "«K» заменена на «AUTO»"
    );

    // Слежение выключено — возвращается прежний вид.
    set_facts(
        &mut rig,
        follow_facts(3_200.0, 0.0, WhiteOrigin::Manual, WhiteFollowFacts::Off),
    );
    assert_eq!(text_count_of(&rig.last, AUTO_UNIT, TextRoleKind::StripUnit), 0);
    assert_eq!(text_count_of(&rig.last, KELVIN_UNIT, TextRoleKind::StripUnit), 1);
}

#[test]
fn the_auto_unit_fits_inside_the_glass_circle_at_its_height() {
    let mut rig = Rig::new();
    set_facts(
        &mut rig,
        follow_facts(5_600.0, 0.0, WhiteOrigin::Manual, WhiteFollowFacts::On),
    );
    let unit = texts_of(&rig.last, AUTO_UNIT, TextRoleKind::StripUnit).remove(0);
    let run = run_of(AUTO_UNIT, TextRoleKind::StripUnit);
    let config = &rig.screen.config.widgets;
    // Ширина после сжатия, pt (поворота в портрете нет: масштаб лежит в `a`).
    let drawn_width_pt = f64::from(run.advance_px * unit.transform.a.abs() / PX_PER_PT);
    let radius_pt = config.bottom.menu_button.diameter_pt * 0.5;
    // Нижняя кромка надписи: смещение строки от центра круга плюс половина высоты заглавных.
    let cap_pt = f64::from(run.cap_height_px * unit.transform.a.abs() / PX_PER_PT);
    let lowest_pt = config.strip.ev_unit_offset_pt.abs() + cap_pt * 0.5;
    let chord_pt = 2.0 * (radius_pt * radius_pt - lowest_pt * lowest_pt).sqrt();
    assert!(
        drawn_width_pt <= chord_pt,
        "«AUTO» {drawn_width_pt:.1} pt шире хорды круга {chord_pt:.1} pt на высоте надписи"
    );
    assert!(drawn_width_pt > 4.0, "надпись не схлопнулась в ноль: {drawn_width_pt}");
}

// ---- подписи помещаются в 60 pt ----

#[test]
fn every_caption_of_the_row_and_the_widest_estimate_fit_the_button_pitch() {
    let config = ScreenConfig::default();
    let pitch_pt = config.widgets.white_card.light_pitch_pt;
    let widest_estimate = phone_estimate_text(
        Some(WhiteReading::Measured {
            white: held(12_000.0, 0.0, WhiteOrigin::Manual).setting,
        }),
        &config.white,
    )
    .expect("оценка");
    assert_eq!(widest_estimate, "\u{2248} 12000");
    let mut captions = vec![AUTO_LABEL.to_owned(), PHONE_LABEL.to_owned(), widest_estimate];
    captions.extend(config.white.lights.iter().map(|light| light.label.clone()));
    for caption in captions {
        let width_pt = f64::from(run_of(&caption, TextRoleKind::Caption).advance_px / PX_PER_PT);
        assert!(
            width_pt <= pitch_pt,
            "«{caption}» {width_pt:.1} pt шире кнопки {pitch_pt} pt"
        );
    }
}

// ---- невручённая команда ----

#[test]
fn an_undelivered_follow_vibrates_once_and_shakes_neither_phone_nor_number() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    rig.run(300);
    rig.effects();
    let phone_rest = text_x(&rig, PHONE_LABEL, TextRoleKind::Caption);
    let number_rest = text_x(&rig, "5600", TextRoleKind::StripValue);
    rig.screen.handle_action(ScreenAction::WhiteFollowNotDelivered);
    let mut widest: f32 = 0.0;
    for _ in 0..24 {
        rig.run(FRAME_MS);
        widest = widest
            .max((text_x(&rig, PHONE_LABEL, TextRoleKind::Caption) - phone_rest).abs())
            .max((text_x(&rig, "5600", TextRoleKind::StripValue) - number_rest).abs());
    }
    assert!(widest < 0.01, "ничего не дёргалось: размах {widest} px");
    assert!(!rig.screen.white_attention_animating());
    assert_eq!(
        rig.effects(),
        [Effect::Haptic(HapticPattern::Error)],
        "ровно одна вибрация «ошибка»"
    );
}

// ---- места ряда ----

#[test]
fn the_row_slots_run_auto_phone_lights_inside_the_card_without_overlap_and_are_wide_enough_to_touch() {
    let rig = Rig::new();
    let card = rig.rect_with(|g| g.white_card());
    let light_count = ScreenConfig::default().white.lights.len();
    let slots: Vec<_> = WhiteRowSlot::row(light_count)
        .map(|slot| (slot, rig.rect_with(|g| g.white_row_slot(slot, light_count))))
        .collect();
    assert_eq!(slots.len(), 7, "семь кнопок: Auto, Phone, пять света (WB48)");
    for (slot, rect) in &slots {
        assert!(
            rect.size.width >= 44.0 && rect.size.height >= 44.0,
            "{slot:?}: {rect:?}"
        );
        assert!(
            rect.min_x() >= card.min_x() - 1e-9 && rect.max_x() <= card.max_x() + 1e-9,
            "{slot:?} вне карточки: {rect:?} / {card:?}"
        );
        assert!((rect.size.width - 60.0).abs() < 1e-9, "шаг 60 pt: {slot:?}");
    }
    for pair in slots.windows(2) {
        let (left, right) = (pair[0].1, pair[1].1);
        assert!(
            left.max_x() <= right.min_x() + 1e-9,
            "{:?} налезает на {:?}",
            pair[0].0,
            pair[1].0
        );
        assert!(
            left.center().x < right.center().x,
            "порядок слева направо: {:?}, {:?}",
            pair[0].0,
            pair[1].0
        );
    }
    assert_eq!(slots[0].0, WhiteRowSlot::Auto);
    assert_eq!(slots[1].0, WhiteRowSlot::Phone);
    // Ряд симметричен относительно середины карточки.
    let left_gap = slots[0].1.min_x() - card.min_x();
    let right_gap = card.max_x() - slots[6].1.max_x();
    assert!(
        (left_gap - right_gap).abs() < 1e-9,
        "ряд по центру: {left_gap} / {right_gap}"
    );
}

#[test]
fn the_published_touch_targets_are_the_centres_of_the_row_slots() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let targets = card_targets(&rig);
    let light_count = rig.screen.config.white.lights.len();
    let center = |slot| rig.rect_with(|g| g.white_row_slot(slot, light_count)).center();
    assert_eq!(targets.auto_button, center(WhiteRowSlot::Auto));
    assert_eq!(targets.phone_button, center(WhiteRowSlot::Phone));
    assert_eq!(targets.light_buttons.len(), light_count);
    for (index, point) in targets.light_buttons.iter().enumerate() {
        assert_eq!(*point, center(WhiteRowSlot::Light(index)), "свет {index}");
    }
    assert!(targets.auto_button.x < targets.phone_button.x && targets.phone_button.x < targets.light_buttons[0].x);
}

#[test]
fn each_source_button_sends_its_own_intent_and_the_icons_are_drawn() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    assert_eq!(
        icons_of(&rig.last, IconId::MagicWand).len(),
        1,
        "значок «Auto» нарисован"
    );
    let targets = card_targets(&rig);
    rig.effects();
    rig.tap(targets.phone_button);
    assert_eq!(
        camera_intents(&rig.effects()),
        [ScreenCameraIntent::TakeWhite(WhiteSourceId::PHONE)],
        "«Phone» не шлёт «следить»"
    );
    rig.tap(targets.auto_button);
    assert_eq!(
        camera_intents(&rig.effects()),
        [ScreenCameraIntent::FollowWhite(WhiteSourceId::PHONE)],
        "«Auto» не шлёт «взять»"
    );
}
