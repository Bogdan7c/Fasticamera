#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K4, вид карусели (`view::carousel_items`): подписи стоят по `placements(now)`, выбранная другого
//! цвета, типографика `ModeLabel`, несовпадение числа подписей — ошибка; список проходит `validate()`.

use ui_core::time::MotionPreference;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::geometry::PxPoint;
use ui_draw::light::LightDirection;
use ui_draw::transform::Affine2;
use ui_kit::carousel::{CarouselEntry, CarouselSpec, SnapCarousel};
use ui_kit::tokens::{TextRoleKind, UiKitTokens};
use ui_kit::view::{CarouselLabels, LabelPainter, PxPerPt, ViewError, carousel_items};

mod common;
use common::*;

const SCALE: f32 = 3.0;
const HOST: (f32, f32) = (660.0, 2700.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Video,
    Photo,
    Portrait,
}

fn px() -> PxPerPt {
    PxPerPt::new(SCALE).unwrap()
}

fn carousel(selected: Item, tokens: &UiKitTokens) -> SnapCarousel<Item> {
    let entries = vec![
        CarouselEntry {
            item: Item::Video,
            width_points: 61.1,
        },
        CarouselEntry {
            item: Item::Photo,
            width_points: 50.0,
        },
        CarouselEntry {
            item: Item::Portrait,
            width_points: 70.0,
        },
    ];
    SnapCarousel::new(
        entries,
        &selected,
        CarouselSpec::default(),
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

fn labels(tokens: &UiKitTokens) -> CarouselLabels {
    let painter = LabelPainter::new(&tokens.typography, px());
    CarouselLabels::layout(&painter, &["VIDEO", "PHOTO", "PORTRAIT"]).unwrap()
}

/// Центр подписи на экране и цвет: обращаем сдвиг `centered_text` (начало строки — слева на базовой линии).
fn placed(item: &DrawItem) -> (PxPoint, DisplayColor) {
    let DrawItem::Text(text) = item else {
        panic!("карусель рисует только подписи: {item:?}")
    };
    let centre = text
        .transform
        .apply_point(PxPoint::new(text.run.advance_px * 0.5, -text.run.cap_height_px * 0.5));
    (centre, text.color)
}

fn draw(carousel: &SnapCarousel<Item>, now: i64, tokens: &UiKitTokens) -> Vec<DrawItem> {
    carousel_items(
        Affine2::translate(HOST.0, HOST.1),
        Opacity::OPAQUE,
        carousel,
        ms(now),
        &labels(tokens),
        px(),
        &tokens.colors,
    )
    .unwrap()
}

#[test]
fn labels_stand_at_their_placements_and_only_the_selected_one_has_its_own_colour() {
    let tokens = UiKitTokens::default();
    let carousel = carousel(Item::Photo, &tokens);
    let items = draw(&carousel, 0, &tokens);
    assert_eq!(items.len(), 3);
    let placements = carousel.placements(ms(0));
    for (index, item) in items.iter().enumerate() {
        let (centre, color) = placed(item);
        let expected_x = HOST.0 + placements[index].center_offset_points as f32 * SCALE;
        assert!(
            (centre.x - expected_x).abs() < 0.01,
            "подпись {index}: {} != {expected_x}",
            centre.x
        );
        assert!((centre.y - HOST.1).abs() < 0.01, "подпись {index} по вертикали");
        let expected_color = if index == 1 {
            tokens.colors.selected_label
        } else {
            tokens.colors.text_muted
        };
        assert_eq!(color, expected_color, "цвет подписи {index}");
    }
    // Выбранная и невыбранная действительно различимы.
    assert_ne!(tokens.colors.selected_label, tokens.colors.text_muted);
}

#[test]
fn labels_follow_the_carousel_while_it_scrolls_and_colour_follows_the_selection() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Video, &tokens);
    let before = draw(&carousel, 0, &tokens);
    // Свайп влево: контент едет вправо-налево, выбранным становится «PHOTO».
    carousel.drag_began(ms(0), pan_same(0.0)).unwrap();
    carousel.drag_moved(pan_same(-60.0)).unwrap();
    carousel.drag_ended(ms(16), horizontal(-800.0)).unwrap();
    // Через время успокоения подписи стоят на новых местах: PHOTO — по центру.
    let after = draw(&carousel, 3000, &tokens);
    let (video_before, _) = placed(&before[0]);
    let (video_after, _) = placed(&after[0]);
    assert!(
        video_after.x < video_before.x - 10.0,
        "PHOTO стал центром — VIDEO ушёл влево"
    );
    let (photo_after, photo_color) = placed(&after[1]);
    assert!((photo_after.x - HOST.0).abs() < 0.5, "выбранная подпись по центру");
    assert_eq!(photo_color, tokens.colors.selected_label);
    assert_eq!(placed(&after[0]).1, tokens.colors.text_muted);
}

#[test]
fn labels_use_the_mode_label_role_and_the_host_transform_and_opacity() {
    let tokens = UiKitTokens::default();
    let carousel = carousel(Item::Photo, &tokens);
    let items = carousel_items(
        Affine2::translate(HOST.0, HOST.1),
        Opacity::new(0.25).unwrap(),
        &carousel,
        ms(0),
        &labels(&tokens),
        px(),
        &tokens.colors,
    )
    .unwrap();
    let style = tokens.typography.role(TextRoleKind::ModeLabel).style(SCALE);
    let expected_run = ui_draw::text::layout_line("PHOTO", &style).unwrap();
    let DrawItem::Text(photo) = &items[1] else { panic!() };
    assert_eq!(photo.run, expected_run, "роль текста ModeLabel");
    assert!((photo.opacity.get() - 0.25).abs() < 1e-6);
    let mut list = DrawList::new(LightDirection::from_angle_degrees(45.0));
    list.items = items;
    list.validate().unwrap();
}

#[test]
fn label_count_must_match_the_carousel() {
    let tokens = UiKitTokens::default();
    let carousel = carousel(Item::Photo, &tokens);
    let painter = LabelPainter::new(&tokens.typography, px());
    let short = CarouselLabels::layout(&painter, &["VIDEO", "PHOTO"]).unwrap();
    assert_eq!(short.len(), 2);
    let result = carousel_items(
        Affine2::IDENTITY,
        Opacity::OPAQUE,
        &carousel,
        ms(0),
        &short,
        px(),
        &tokens.colors,
    );
    assert_eq!(
        result.unwrap_err(),
        ViewError::CarouselLabelsMismatch { items: 3, labels: 2 }
    );
}

#[test]
fn unsupported_character_in_a_label_is_reported_not_dropped() {
    let tokens = UiKitTokens::default();
    let painter = LabelPainter::new(&tokens.typography, px());
    let result = CarouselLabels::layout(&painter, &["PHOTO", "ФОТО"]);
    assert!(matches!(result, Err(ViewError::Text(_))));
}
