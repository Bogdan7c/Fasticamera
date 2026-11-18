//! Делёж «выдержка × ISO»: кратность периоду мерцания, пределы и честный остаток.

use std::time::Duration;

use super::split::{Anchor, Split, SplitLimits, split};

const PERIOD_50HZ: Duration = Duration::from_millis(10);
const PERIOD_60HZ: Duration = Duration::from_nanos(8_333_333);

/// Типичный сенсор: ISO 50–6400, выдержка от 100 мкс, кадр 60 к/с (предел 16,667 мс).
fn limits() -> SplitLimits {
    SplitLimits {
        iso_min: 50,
        iso_max: 6_400,
        shutter_shortest: Duration::from_micros(100),
        shutter_cap: Duration::from_nanos(16_666_667),
    }
}

fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() <= expected.abs() * 0.01,
        "{what}: {actual} вместо {expected}"
    );
}

#[test]
fn protected_shutter_is_whole_half_periods_and_iso_pays_the_difference() {
    // Нужно 16 мс при ISO 50 (0,8 с·ISO): без защиты выдержка 16 мс, с защитой 50 Гц — 10 мс.
    let divided = split(0.8, Anchor::Free, limits(), Some(PERIOD_50HZ));
    assert_eq!(divided.shutter, PERIOD_50HZ);
    assert_eq!(divided.iso, 80);
    assert_close(divided.product_seconds_iso(), 0.8, "произведение сохранено");
}

#[test]
fn cleared_protection_lets_the_shutter_use_the_whole_frame() {
    let divided = split(0.8, Anchor::Free, limits(), None);
    assert_eq!(divided.shutter, Duration::from_millis(16));
    assert_eq!(divided.iso, 50);
    // Свет, которому не хватает и кадра: выдержка упирается в предел кадра, ISO доплачивает.
    let dim = split(2.0, Anchor::Free, limits(), None);
    assert_eq!(dim.shutter, limits().shutter_cap);
    assert_eq!(dim.iso, 120);
}

#[test]
fn protection_floors_the_capped_shutter_to_a_period_multiple() {
    // Желаемые 40 мс обрезаны кадром до 16,667 мс, а потом — до одного периода 10 мс.
    let divided = split(2.0, Anchor::Free, limits(), Some(PERIOD_50HZ));
    assert_eq!(divided.shutter, PERIOD_50HZ);
    assert_eq!(divided.iso, 200);
}

#[test]
fn shutter_shorter_than_one_period_stays_free() {
    let divided = split(0.25, Anchor::Free, limits(), Some(PERIOD_50HZ));
    assert_eq!(
        divided.shutter,
        Duration::from_millis(5),
        "короче периода — защита не нужна"
    );
    assert_eq!(divided.iso, 50);
}

#[test]
fn sixty_hertz_period_is_a_third_shorter_than_fifty() {
    let divided = split(0.5, Anchor::Free, limits(), Some(PERIOD_60HZ));
    assert_eq!(divided.shutter, PERIOD_60HZ);
    assert_eq!(divided.iso, 60);
}

#[test]
fn period_longer_than_the_frame_cap_cannot_be_honoured() {
    // Кадр 120 к/с (8,33 мс) и сеть 50 Гц: период 10 мс не влезает — защита невозможна,
    // выдержка просто режется по кадру, а не нулевая.
    let short_frame = SplitLimits {
        shutter_cap: Duration::from_nanos(8_333_333),
        ..limits()
    };
    let divided = split(0.8, Anchor::Free, short_frame, Some(PERIOD_50HZ));
    assert_eq!(divided.shutter, short_frame.shutter_cap);
}

#[test]
fn out_of_iso_range_leaves_an_honest_shortfall() {
    // ISO упёрся в 6400 при выдержке 16,667 мс: набрано 106,7 из 200 — темнее нужного.
    let divided = split(200.0, Anchor::Free, limits(), None);
    assert_eq!(divided.iso, 6_400);
    assert!(divided.product_seconds_iso() < 200.0 * 0.6);
}

#[test]
fn too_bright_scene_is_floored_by_the_shortest_shutter_and_lowest_iso() {
    let divided = split(0.000_001, Anchor::Free, limits(), Some(PERIOD_50HZ));
    assert_eq!(divided.shutter, limits().shutter_shortest);
    assert_eq!(divided.iso, 50);
    assert!(
        divided.product_seconds_iso() > 0.000_001,
        "светлее нужного — нехватка со знаком +"
    );
}

#[test]
fn fixed_iso_makes_the_shutter_compensate() {
    let divided = split(1.0, Anchor::Iso(100), limits(), Some(PERIOD_50HZ));
    assert_eq!(
        divided,
        Split {
            shutter: PERIOD_50HZ,
            iso: 100
        }
    );
    // 15 мс: с защитой округлится вниз до 10 мс (недобор остаётся честным), без — ровно 15.
    let protected = split(1.5, Anchor::Iso(100), limits(), Some(PERIOD_50HZ));
    assert_eq!(protected.shutter, PERIOD_50HZ);
    let free = split(1.5, Anchor::Iso(100), limits(), None);
    assert_eq!(free.shutter, Duration::from_millis(15));
}

#[test]
fn fixed_iso_is_clamped_into_the_camera_range() {
    let divided = split(1.0, Anchor::Iso(10), limits(), None);
    assert_eq!(divided.iso, 50);
}

#[test]
fn fixed_shutter_makes_iso_compensate_and_ignores_flicker() {
    let divided = split(
        0.8,
        Anchor::Shutter(Duration::from_millis(4)),
        limits(),
        Some(PERIOD_50HZ),
    );
    assert_eq!(divided.shutter, Duration::from_millis(4));
    assert_eq!(divided.iso, 200);
    let long = split(
        0.75,
        Anchor::Shutter(Duration::from_millis(15)),
        limits(),
        Some(PERIOD_50HZ),
    );
    assert_eq!(
        long.shutter,
        Duration::from_millis(15),
        "ручную выдержку защита не трогает"
    );
}

#[test]
fn degenerate_camera_limits_do_not_panic() {
    let broken = SplitLimits {
        iso_min: 0,
        iso_max: 0,
        shutter_shortest: Duration::ZERO,
        shutter_cap: Duration::ZERO,
    };
    let divided = split(f64::MAX, Anchor::Free, broken, Some(PERIOD_50HZ));
    assert_eq!(divided.iso, 0);
}

#[test]
fn fixed_iso_rounds_to_the_nearest_period_multiple_but_free_split_rounds_down() {
    let roomy = SplitLimits {
        shutter_cap: Duration::from_millis(40),
        ..limits()
    };
    // Нужно 19 мс при ISO 100: ближайшая кратная 20 мс (ошибка 0,07 EV), а не 10 мс (−0,93 EV).
    let fixed = split(1.9, Anchor::Iso(100), roomy, Some(PERIOD_50HZ));
    assert_eq!(fixed.shutter, Duration::from_millis(20));
    // Ближе к нижней кратной — вниз (14 мс → 10 мс).
    assert_eq!(
        split(1.4, Anchor::Iso(100), roomy, Some(PERIOD_50HZ)).shutter,
        PERIOD_50HZ
    );
    // Ближайшая выше предела кадра (16,667 мс) недопустима — вниз от предела.
    assert_eq!(
        split(1.6, Anchor::Iso(100), limits(), Some(PERIOD_50HZ)).shutter,
        PERIOD_50HZ
    );
    // Свободный делёж: вниз, ISO доплачивает.
    let free = split(0.95, Anchor::Free, roomy, Some(PERIOD_50HZ));
    assert_eq!(free.shutter, PERIOD_50HZ);
    assert_eq!(free.iso, 95);
}
