#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! K4 «Карусель с прилипанием»: листание на ±1, тап, один элемент, замена списка, «ведение за пальцем».

use shell_contract::haptics::HapticPattern;
use ui_core::time::MotionPreference;
use ui_kit::carousel::{
    CarouselEntry, CarouselError, CarouselEvent, CarouselSpec, SelectionCause, SnapCarousel, TapOutcome,
};
use ui_kit::drive::ScrollTransition;
use ui_kit::drive::{DragOrigin, MotionPhase};
use ui_kit::tokens::{HapticSetting, UiKitTokens};

mod common;
use common::*;

/// Тестовые «режимы» — карусель про них ничего не знает, ей нужны только равенство и клон.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Item {
    Video,
    Photo,
    Portrait,
    Pano,
}

const GAP: f64 = 35.4; // токен `carousel_item_gap_points`
/// Смещения точек прилипания для ширин 61,1 / 50 / 70, посчитанные руками.
const OFFSET_1: f64 = 61.1 / 2.0 + GAP + 50.0 / 2.0; // 90,95
const OFFSET_2: f64 = OFFSET_1 + 50.0 / 2.0 + GAP + 70.0 / 2.0; // 186,35

fn entry(item: Item, width_points: f64) -> CarouselEntry<Item> {
    CarouselEntry { item, width_points }
}

fn three_items() -> Vec<CarouselEntry<Item>> {
    vec![
        entry(Item::Video, 61.1),
        entry(Item::Photo, 50.0),
        entry(Item::Portrait, 70.0),
    ]
}

fn carousel_with(
    entries: Vec<CarouselEntry<Item>>,
    selected: Item,
    drag_origin: DragOrigin,
    tokens: &UiKitTokens,
) -> SnapCarousel<Item> {
    let spec = CarouselSpec {
        drag_origin,
        ..CarouselSpec::default()
    };
    SnapCarousel::new(entries, &selected, spec, &env(tokens, MotionPreference::Full), ms(0)).unwrap()
}

fn carousel(selected: Item, tokens: &UiKitTokens) -> SnapCarousel<Item> {
    carousel_with(three_items(), selected, DragOrigin::FromRecognition, tokens)
}

/// Палец идёт справа налево (`FingerLeft` по умолчанию — следующий элемент), контент растёт.
fn swipe_left(carousel: &mut SnapCarousel<Item>, start_ms: i64, distance: f64, velocity: f64) {
    carousel.drag_began(ms(start_ms), pan_same(0.0)).unwrap();
    carousel.drag_moved(pan_same(-distance)).unwrap();
    carousel.drag_ended(ms(start_ms + 16), horizontal(-velocity)).unwrap();
}

#[test]
fn snap_points_are_item_centers_relative_to_the_first_one() {
    let tokens = UiKitTokens::default();
    let carousel = carousel(Item::Video, &tokens);
    let offsets = carousel.snap_offsets();
    assert_eq!(offsets.len(), 3);
    assert_eq!(offsets[0], 0.0);
    assert!((offsets[1] - OFFSET_1).abs() < 1e-9);
    assert!((offsets[2] - OFFSET_2).abs() < 1e-9);
    // Размещение для рисования: выбранный по центру, соседи — на расстоянии точки прилипания.
    let placements = carousel.placements(ms(0));
    assert_eq!(placements[0].center_offset_points, 0.0);
    assert!((placements[1].center_offset_points - OFFSET_1).abs() < 1e-9);
    assert_eq!(placements[1].width_points, 50.0);
}

#[test]
fn carousel_content_follows_the_finger_one_to_one_without_hysteresis_jump() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Photo, &tokens);
    // Палец прошёл 8 pt гистерезиса до распознавания.
    carousel.drag_began(ms(0), pan(-8.0, 0.0)).unwrap();
    assert!(
        (carousel.offset_points(ms(0)) - OFFSET_1).abs() < 1e-9,
        "на Began скачка нет"
    );
    for frame in 1..=20 {
        let finger = -3.0 * frame as f64;
        carousel.drag_moved(pan(finger - 8.0, finger)).unwrap();
        let now = ms(frame * 16);
        // Палец влево на 3·frame pt — контент вправо ровно на столько же.
        assert!(
            (carousel.offset_points(now) - (OFFSET_1 + 3.0 * frame as f64)).abs() < 1e-9,
            "кадр {frame}"
        );
    }
    // Вариант «от касания» прыгает на гистерезис.
    let mut jumping = carousel_with(three_items(), Item::Photo, DragOrigin::FromTouchDown, &tokens);
    jumping.drag_began(ms(0), pan(-8.0, 0.0)).unwrap();
    assert!((jumping.offset_points(ms(0)) - (OFFSET_1 + 8.0)).abs() < 1e-9);
}

#[test]
fn release_speed_equals_finger_speed_and_selection_is_decided_at_release() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Video, &tokens);
    swipe_left(&mut carousel, 0, 40.0, 700.0);
    let release = ms(16);
    assert!(
        (carousel.velocity_points_per_second(release) - 700.0).abs() < 1e-6,
        "ступеньки скорости нет"
    );
    assert!((carousel.offset_points(release) - 40.0).abs() < 1e-9);
    // Решение принято в момент отпускания: выбор и хаптика уже есть, пружина ещё едет.
    assert_eq!(carousel.selected(), &Item::Photo);
    assert_eq!(carousel.phase(release), MotionPhase::Settling);
    let events = carousel.drain_events();
    assert_eq!(
        events,
        vec![
            CarouselEvent::SelectionChanged {
                item: Item::Photo,
                index: 1,
                cause: SelectionCause::Swipe
            },
            CarouselEvent::Haptic {
                pattern: HapticPattern::SelectionTick
            },
        ]
    );
    assert!((carousel.offset_points(ms(16 + 5_000)) - OFFSET_1).abs() < 1e-9);
    assert_eq!(carousel.phase(ms(16 + 5_000)), MotionPhase::Resting);
}

#[test]
fn a_hard_fling_pages_exactly_one_item_never_more() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Video, &tokens);
    // Бросок, который по проекции улетел бы за все три элемента.
    swipe_left(&mut carousel, 0, 60.0, 20_000.0);
    assert_eq!(carousel.selected_index(), 1);
    assert!((carousel.offset_points(ms(10_000)) - OFFSET_1).abs() < 1e-9);
    // И назад — тоже ровно на один.
    carousel.drag_began(ms(20_000), pan_same(0.0)).unwrap();
    carousel.drag_moved(pan_same(80.0)).unwrap();
    carousel.drag_ended(ms(20_016), horizontal(30_000.0)).unwrap();
    assert_eq!(carousel.selected_index(), 0);
    let changes = carousel
        .drain_events()
        .into_iter()
        .filter(|event| matches!(event, CarouselEvent::SelectionChanged { .. }))
        .count();
    assert_eq!(changes, 2, "по одному событию на каждую смену");
}

#[test]
fn a_short_drag_returns_to_the_same_item_without_any_event() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Photo, &tokens);
    swipe_left(&mut carousel, 0, 20.0, 0.0);
    assert!(
        carousel.drain_events().is_empty(),
        "меньше половины и без скорости — остаёмся"
    );
    assert_eq!(carousel.selected(), &Item::Photo);
    assert!((carousel.offset_points(ms(5_000)) - OFFSET_1).abs() < 1e-9);
}

#[test]
fn swiping_again_while_settling_continues_from_the_new_selection() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Video, &tokens);
    swipe_left(&mut carousel, 0, 60.0, 900.0);
    // Через 100 мс контент ещё едет к Photo; хватаем и листаем дальше.
    swipe_left(&mut carousel, 116, 30.0, 900.0);
    assert_eq!(carousel.selected(), &Item::Portrait);
    assert!((carousel.offset_points(ms(10_000)) - OFFSET_2).abs() < 1e-9);
    let kinds: Vec<usize> = carousel
        .drain_events()
        .into_iter()
        .filter_map(|event| match event {
            CarouselEvent::SelectionChanged { index, .. } => Some(index),
            _ => None,
        })
        .collect();
    assert_eq!(kinds, vec![1, 2]);
}

#[test]
fn touching_a_moving_carousel_stops_it_in_place_without_a_second_selection_event() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Video, &tokens);
    swipe_left(&mut carousel, 0, 60.0, 900.0);
    carousel.drain_events();
    let catch_time = ms(16 + 120);
    let before = carousel.offset_points(catch_time);
    // Пружина с большой начальной скоростью может проскочить цель; важно, что карусель в движении.
    assert_eq!(carousel.phase(catch_time), MotionPhase::Settling);
    assert!(carousel.velocity_points_per_second(catch_time).abs() > 10.0);
    assert!((before - OFFSET_1).abs() > 1.0, "ещё не на месте: {before}");
    carousel.catch(catch_time);
    assert!(
        (carousel.offset_points(catch_time) - before).abs() < 1e-9,
        "положение до и после захвата совпадает"
    );
    assert_eq!(carousel.phase(ms(16 + 5_000)), MotionPhase::Touched);
    assert!((carousel.offset_points(ms(16 + 5_000)) - before).abs() < 1e-9);
    assert_eq!(carousel.velocity_points_per_second(ms(16 + 5_000)), 0.0);
    // Отпустили без движения: доезжает до ближайшего элемента (Photo), выбор уже был.
    carousel.release_without_drag(ms(6_000)).unwrap();
    assert!((carousel.offset_points(ms(20_000)) - OFFSET_1).abs() < 1e-9);
    assert!(carousel.drain_events().is_empty(), "повторного события выбора нет");
}

#[test]
fn tap_on_an_item_rides_to_it_and_selects_it_once() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Video, &tokens);
    assert_eq!(carousel.tapped_item(2, ms(0)).unwrap(), TapOutcome::Selected);
    assert_eq!(carousel.selected(), &Item::Portrait);
    assert_eq!(carousel.phase(ms(100)), MotionPhase::Settling);
    assert!(carousel.offset_points(ms(100)) > 0.0 && carousel.offset_points(ms(100)) < OFFSET_2);
    assert!((carousel.offset_points(ms(10_000)) - OFFSET_2).abs() < 1e-9);
    let events = carousel.drain_events();
    assert_eq!(events.len(), 2);
    assert!(matches!(
        events[0],
        CarouselEvent::SelectionChanged {
            item: Item::Portrait,
            index: 2,
            cause: SelectionCause::Tap
        }
    ));
    assert_eq!(
        events[1],
        CarouselEvent::Haptic {
            pattern: HapticPattern::SelectionTick
        }
    );
    // Тап по уже выбранному — отдельный исход, без событий выбора.
    assert_eq!(
        carousel.tapped_item(2, ms(11_000)).unwrap(),
        TapOutcome::AlreadySelected
    );
    assert!(carousel.drain_events().is_empty());
    // Индекс вне списка — явная ошибка, состояние не меняется.
    assert_eq!(
        carousel.tapped_item(3, ms(12_000)),
        Err(CarouselError::IndexOutOfRange { index: 3, length: 3 })
    );
    assert_eq!(carousel.selected_index(), 2);
}

#[test]
fn selection_haptic_follows_the_token_table() {
    let mut tokens = UiKitTokens::default();
    tokens.haptics.carousel_selection = HapticSetting::Silent;
    let mut carousel = carousel(Item::Video, &tokens);
    carousel.tapped_item(1, ms(0)).unwrap();
    let events = carousel.drain_events();
    assert_eq!(events.len(), 1, "только выбор, вибрации нет");
    assert!(matches!(events[0], CarouselEvent::SelectionChanged { .. }));
}

#[test]
fn single_item_carousel_only_stretches_like_rubber_and_never_changes_selection() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel_with(
        vec![entry(Item::Photo, 61.1)],
        Item::Photo,
        DragOrigin::FromRecognition,
        &tokens,
    );
    assert_eq!(carousel.snap_offsets(), &[0.0]);
    carousel.drag_began(ms(0), pan_same(0.0)).unwrap();
    carousel.drag_moved(pan_same(-120.0)).unwrap();
    // Границы вырождены в точку: любой сдвиг — за краем, значит резинка по формуле motion.md.
    assert!((carousel.offset_points(ms(16)) - rubber_formula(120.0, VIEWPORT_POINTS)).abs() < 1e-9);
    carousel.drag_ended(ms(16), horizontal(-2_000.0)).unwrap();
    assert_eq!(carousel.selected(), &Item::Photo);
    assert!(carousel.offset_points(ms(200)) > 0.0, "ещё возвращается");
    assert!(carousel.offset_points(ms(10_000)).abs() < 1e-9, "вернулась в ноль");
    assert!(carousel.drain_events().is_empty());
    assert_eq!(
        carousel.tapped_item(0, ms(11_000)).unwrap(),
        TapOutcome::AlreadySelected
    );
}

#[test]
fn dragging_past_the_last_item_returns_without_a_jump_and_selects_the_last_one() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Photo, &tokens);
    carousel.drag_began(ms(0), pan_same(0.0)).unwrap();
    // От Photo сразу далеко за край: OFFSET_1 + 400 (граница — OFFSET_2).
    carousel.drag_moved(pan_same(-400.0)).unwrap();
    let stretched = carousel.offset_points(ms(16));
    assert!((stretched - (OFFSET_2 + rubber_formula(OFFSET_1 + 400.0 - OFFSET_2, VIEWPORT_POINTS))).abs() < 1e-9);
    carousel.drag_ended(ms(16), horizontal(0.0)).unwrap();
    let mut previous = carousel.offset_points(ms(16));
    assert!((previous - stretched).abs() < 1e-9);
    for step in 1..=3_000 {
        let current = carousel.offset_points(ms(16 + step));
        assert!((current - previous).abs() < 4.0, "скачок {previous} → {current}");
        previous = current;
    }
    assert!((previous - OFFSET_2).abs() < 1e-9);
    assert_eq!(
        carousel.selected(),
        &Item::Portrait,
        "карусель вернулась к последнему — он и выбран"
    );
}

#[test]
fn replacing_the_list_keeps_the_selected_item_by_identity() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Photo, &tokens);
    // Добавили режим перед выбранным: индекс сменился, выбор — нет, и событий нет.
    let new_entries = vec![
        entry(Item::Pano, 40.0),
        entry(Item::Video, 61.1),
        entry(Item::Photo, 50.0),
        entry(Item::Portrait, 70.0),
    ];
    carousel.set_entries(new_entries, ms(0)).unwrap();
    assert_eq!(carousel.selected(), &Item::Photo);
    assert_eq!(carousel.selected_index(), 2);
    let expected_offset = carousel.snap_offsets()[2];
    // Эталон руками: центры (20), (40+35,4+30,55), (… ) → от первого центра.
    let c0 = 20.0;
    let c2 = 40.0 + GAP + 61.1 + GAP + 25.0;
    assert!((expected_offset - (c2 - c0)).abs() < 1e-9);
    assert!(
        (carousel.offset_points(ms(0)) - expected_offset).abs() < 1e-9,
        "покоящаяся карусель переставлена сразу"
    );
    assert!(carousel.drain_events().is_empty());
    assert_eq!(carousel.entries().len(), 4);
    assert_eq!(carousel.phase(ms(0)), MotionPhase::Resting);
}

#[test]
fn replacing_the_list_without_the_selected_item_picks_a_neighbour_and_reports_it() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Portrait, &tokens);
    carousel
        .set_entries(vec![entry(Item::Video, 61.1), entry(Item::Photo, 50.0)], ms(0))
        .unwrap();
    assert_eq!(
        carousel.selected(),
        &Item::Photo,
        "последний, раз места выбранного больше нет"
    );
    let events = carousel.drain_events();
    assert_eq!(
        events,
        vec![CarouselEvent::SelectionChanged {
            item: Item::Photo,
            index: 1,
            cause: SelectionCause::Programmatic
        }],
        "программная замена не вибрирует"
    );
    assert!((carousel.offset_points(ms(0)) - carousel.snap_offsets()[1]).abs() < 1e-9);
}

#[test]
fn replacing_the_list_while_moving_rides_to_the_new_position_and_under_a_finger_does_not_move() {
    let tokens = UiKitTokens::default();
    let mut moving = carousel(Item::Video, &tokens);
    swipe_left(&mut moving, 0, 60.0, 900.0);
    moving.drain_events();
    moving
        .set_entries(
            vec![
                entry(Item::Pano, 40.0),
                entry(Item::Video, 61.1),
                entry(Item::Photo, 50.0),
                entry(Item::Portrait, 70.0),
            ],
            ms(100),
        )
        .unwrap();
    assert_eq!(moving.selected(), &Item::Photo);
    let offset_now = moving.offset_points(ms(100));
    assert!(offset_now > 0.0, "не телепортируется");
    assert!((moving.offset_points(ms(10_000)) - moving.snap_offsets()[2]).abs() < 1e-9);

    let mut touched = carousel(Item::Photo, &tokens);
    touched.drag_began(ms(0), pan_same(0.0)).unwrap();
    touched.drag_moved(pan_same(-10.0)).unwrap();
    let held = touched.offset_points(ms(16));
    touched
        .set_entries(
            vec![
                entry(Item::Pano, 40.0),
                entry(Item::Video, 61.1),
                entry(Item::Photo, 50.0),
            ],
            ms(16),
        )
        .unwrap();
    assert!(
        (touched.offset_points(ms(16)) - held).abs() < 1e-9,
        "под пальцем контент не трогаем"
    );
    touched.drag_moved(pan_same(-20.0)).unwrap();
    assert!(
        (touched.offset_points(ms(32)) - (held + 10.0)).abs() < 1e-9,
        "перетаскивание продолжается"
    );
}

#[test]
fn invalid_input_is_reported_and_leaves_the_carousel_untouched() {
    let tokens = UiKitTokens::default();
    let environment = env(&tokens, MotionPreference::Full);
    let spec = CarouselSpec::default();
    assert!(matches!(
        SnapCarousel::new(
            Vec::<CarouselEntry<Item>>::new(),
            &Item::Photo,
            spec,
            &environment,
            ms(0)
        ),
        Err(CarouselError::EmptyEntries)
    ));
    assert!(matches!(
        SnapCarousel::new(three_items(), &Item::Pano, spec, &environment, ms(0)),
        Err(CarouselError::UnknownItem)
    ));
    assert!(matches!(
        SnapCarousel::new(vec![entry(Item::Photo, 0.0)], &Item::Photo, spec, &environment, ms(0)),
        Err(CarouselError::InvalidWidth { index: 0, .. })
    ));
    assert!(matches!(
        SnapCarousel::new(
            vec![entry(Item::Photo, f64::NAN)],
            &Item::Photo,
            spec,
            &environment,
            ms(0)
        ),
        Err(CarouselError::InvalidWidth { .. })
    ));
    let mut carousel = carousel(Item::Photo, &tokens);
    assert_eq!(carousel.set_entries(vec![], ms(0)), Err(CarouselError::EmptyEntries));
    assert_eq!(
        carousel.set_entries(vec![entry(Item::Photo, -1.0)], ms(0)),
        Err(CarouselError::InvalidWidth {
            index: 0,
            width_points: -1.0
        })
    );
    assert_eq!(carousel.entries().len(), 3);
    assert_eq!(carousel.selected(), &Item::Photo);
    assert!(matches!(
        carousel.drag_moved(pan_same(5.0)),
        Err(CarouselError::Scroll(_))
    ));
    assert!(carousel.select(7, ScrollTransition::Immediate, ms(0)).is_err());
}

#[test]
fn programmatic_selection_moves_the_carousel_without_vibration() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Video, &tokens);
    carousel.select(1, ScrollTransition::Immediate, ms(0)).unwrap();
    assert!((carousel.offset_points(ms(0)) - OFFSET_1).abs() < 1e-9);
    let events = carousel.drain_events();
    assert_eq!(events.len(), 1);
    assert!(matches!(
        events[0],
        CarouselEvent::SelectionChanged {
            cause: SelectionCause::Programmatic,
            ..
        }
    ));
    carousel.select(1, ScrollTransition::Animated, ms(1)).unwrap();
    assert!(carousel.drain_events().is_empty(), "тот же элемент — события нет");
}

#[test]
fn capsule_is_expanded_from_swipe_start_until_the_carousel_settles() {
    let tokens = UiKitTokens::default();
    let mut carousel = carousel(Item::Video, &tokens);
    assert!(!carousel.capsule_expanded(ms(0)));
    carousel.catch(ms(0));
    assert!(!carousel.capsule_expanded(ms(0)), "просто касание капсулу не расширяет");
    carousel.drag_began(ms(10), pan_same(0.0)).unwrap();
    assert!(carousel.capsule_expanded(ms(10)));
    carousel.drag_moved(pan_same(-50.0)).unwrap();
    assert!(carousel.capsule_expanded(ms(20)));
    carousel.drag_ended(ms(26), horizontal(-500.0)).unwrap();
    assert!(carousel.capsule_expanded(ms(300)), "пока едет — расширена");
    assert!(!carousel.capsule_expanded(ms(20_000)), "остановилась — сжалась");
    // Тап и программный выбор капсулу не расширяют.
    carousel.tapped_item(2, ms(21_000)).unwrap();
    assert!(!carousel.capsule_expanded(ms(21_100)));
    carousel.select(0, ScrollTransition::Animated, ms(40_000)).unwrap();
    assert!(!carousel.capsule_expanded(ms(40_100)));
}

#[test]
fn identical_input_gives_bit_identical_carousel_samples() {
    let tokens = UiKitTokens::default();
    let run = || {
        let mut carousel = carousel(Item::Video, &tokens);
        carousel.drag_began(ms(0), pan(-6.0, 0.0)).unwrap();
        let mut trace = Vec::new();
        for frame in 1..=15 {
            carousel.drag_moved(pan_same(-4.1 * frame as f64)).unwrap();
            trace.push(carousel.offset_points(ms(frame * 8)).to_bits());
        }
        carousel.drag_ended(ms(128), horizontal(-640.0)).unwrap();
        for frame in 1..=200 {
            let now = ms(128 + frame * 16);
            trace.push(carousel.offset_points(now).to_bits());
            trace.push(carousel.velocity_points_per_second(now).to_bits());
        }
        (trace, carousel.drain_events())
    };
    assert_eq!(run(), run());
}
