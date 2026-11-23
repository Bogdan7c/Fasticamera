#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Растр конического кольца: стоп i на угле i·360/N, 0° вправо, 90° вверх на картинке; края сглажены.

use ui_draw::picture::{PictureEncoding, PictureSize, UiPicture};
use ui_kit::view::{RingGeometry, RingPictureError, ring_picture};

const RED: [u8; 3] = [255, 0, 0];
const GREEN: [u8; 3] = [0, 255, 0];
const BLUE: [u8; 3] = [0, 0, 255];
const YELLOW: [u8; 3] = [255, 255, 0];
const STOPS: [[u8; 3]; 4] = [RED, GREEN, BLUE, YELLOW];
const DIAMETER: u32 = 200;
const THICKNESS: f32 = 20.0;

fn ring() -> UiPicture {
    ring_picture(
        &STOPS,
        RingGeometry {
            outer_diameter_px: DIAMETER,
            thickness_px: THICKNESS,
        },
    )
    .unwrap()
}

/// Пиксель на середине толщины кольца под углом (градусы, 0° вправо, против часовой, 90° вверх на картинке).
fn pixel_at(picture: &UiPicture, angle_degrees: f32) -> [u8; 4] {
    let center = DIAMETER as f32 / 2.0;
    let mid_radius = center - THICKNESS / 2.0;
    let radians = angle_degrees.to_radians();
    let x = center + mid_radius * radians.cos();
    let y = center - mid_radius * radians.sin();
    picture.texel(x.floor() as u32, y.floor() as u32).unwrap()
}

fn assert_color_near(actual: [u8; 4], expected: [u8; 3], tolerance: i32) {
    for channel in 0..3 {
        let difference = (i32::from(actual[channel]) - i32::from(expected[channel])).abs();
        assert!(
            difference <= tolerance,
            "канал {channel}: {actual:?} против {expected:?}"
        );
    }
    assert_eq!(actual[3], 255, "середина кольца непрозрачна: {actual:?}");
}

#[test]
fn picture_has_requested_size_and_srgb_encoding() {
    let picture = ring();
    assert_eq!(picture.size(), PictureSize::new(DIAMETER, DIAMETER));
    assert_eq!(picture.encoding(), PictureEncoding::Srgb);
    assert_eq!(picture.rgba8().len(), (DIAMETER * DIAMETER * 4) as usize);
}

#[test]
fn stops_sit_at_their_angles_counter_clockwise_with_up_at_quarter() {
    let picture = ring();
    assert_color_near(pixel_at(&picture, 0.5), RED, 3); // 0° вправо = стоп 0
    assert_color_near(pixel_at(&picture, 90.0), GREEN, 3); // 90° ВВЕРХ на картинке = стоп N/4
    assert_color_near(pixel_at(&picture, 180.0), BLUE, 3); // влево
    assert_color_near(pixel_at(&picture, 270.0), YELLOW, 3); // вниз
}

#[test]
fn midway_between_stops_is_a_blend() {
    let picture = ring();
    // 45° — ровно середина между красным (0°) и зелёным (90°).
    let pixel = pixel_at(&picture, 45.0);
    assert!(
        (i32::from(pixel[0]) - 128).abs() <= 4 && (i32::from(pixel[1]) - 128).abs() <= 4,
        "{pixel:?}"
    );
    assert_eq!(pixel[2], 0);
    // Круг замкнут: 315° — середина между жёлтым (270°) и красным (360° = 0°).
    let wrap = pixel_at(&picture, 315.0);
    assert_eq!(wrap[0], 255);
    assert!((i32::from(wrap[1]) - 128).abs() <= 4, "{wrap:?}");
}

#[test]
fn center_and_corners_are_transparent() {
    let picture = ring();
    let last = DIAMETER - 1;
    for (x, y) in [(DIAMETER / 2, DIAMETER / 2), (0, 0), (last, 0), (0, last), (last, last)] {
        assert_eq!(picture.texel(x, y).unwrap()[3], 0, "({x}, {y}) должен быть прозрачным");
    }
}

#[test]
fn edges_are_antialiased_with_partial_alpha() {
    let picture = ring();
    // Края идут по окружности, а не по сетке пикселей: частичная альфа встречается на сотнях пикселей обоих краёв.
    let alphas: Vec<u8> = picture.rgba8().chunks(4).map(|px| px[3]).collect();
    let partial = alphas.iter().filter(|&&alpha| alpha > 0 && alpha < 255).count();
    assert!(partial >= 200, "края не сглажены: {partial} частичных пикселей");
    assert!(alphas.contains(&255), "середина кольца непрозрачна");
}

#[test]
fn thin_ring_never_exceeds_full_alpha_and_single_stop_is_uniform() {
    let picture = ring_picture(
        &[[10, 20, 30]],
        RingGeometry {
            outer_diameter_px: 64,
            thickness_px: 0.5,
        },
    )
    .unwrap();
    let opaque_pixels = picture.rgba8().chunks(4).filter(|px| px[3] > 0).collect::<Vec<_>>();
    assert!(!opaque_pixels.is_empty(), "тонкое кольцо всё же видно");
    assert!(opaque_pixels.iter().all(|px| px[..3] == [10, 20, 30]));
}

#[test]
fn invalid_inputs_are_distinct_errors() {
    let make = |stops: &[[u8; 3]], diameter: u32, thickness: f32| {
        ring_picture(
            stops,
            RingGeometry {
                outer_diameter_px: diameter,
                thickness_px: thickness,
            },
        )
        .err()
    };
    assert_eq!(make(&[], 100, 10.0), Some(RingPictureError::NoStops));
    assert_eq!(make(&STOPS, 0, 10.0), Some(RingPictureError::ZeroDiameter));
    assert_eq!(make(&STOPS, 100, 0.0), Some(RingPictureError::InvalidThickness(0.0)));
    assert!(matches!(
        make(&STOPS, 100, f32::NAN),
        Some(RingPictureError::InvalidThickness(_))
    ));
    assert_eq!(
        make(&STOPS, 100, 60.0),
        Some(RingPictureError::ThicknessExceedsRadius {
            thickness_px: 60.0,
            radius_px: 50.0
        })
    );
    assert_eq!(
        make(&STOPS, 100_000, 10.0),
        Some(RingPictureError::DiameterTooLarge(100_000))
    );
}

#[test]
fn edge_alpha_is_half_when_pixel_center_lies_exactly_on_the_edge() {
    // Нечётный диаметр: центры пикселей на осях лежат на целых расстояниях от центра картинки.
    // Внешний радиус 100,5, толщина 20,5 → внутренний радиус ровно 80: пиксель (100 + 80, 100) стоит на краю.
    let picture = ring_picture(
        &STOPS,
        RingGeometry {
            outer_diameter_px: 201,
            thickness_px: 20.5,
        },
    )
    .unwrap();
    let on_inner_edge = picture.texel(180, 100).unwrap()[3];
    assert!(
        (i32::from(on_inner_edge) - 128).abs() <= 1,
        "внутренний край: {on_inner_edge}"
    );
    assert_eq!(
        picture.texel(179, 100).unwrap()[3],
        0,
        "на пиксель внутрь — уже снаружи кольца"
    );
    assert_eq!(
        picture.texel(181, 100).unwrap()[3],
        255,
        "на пиксель вглубь кольца — полное покрытие"
    );
    // Внешний край: пиксель (171, 171) смещён на (71, 71), расстояние 100,409, до края 100,5 остаётся 0,091 px →
    // покрытие 0,5 + 0,091 ≈ 0,59 → альфа ≈ 151.
    let near_outer_edge = picture.texel(171, 171).unwrap()[3];
    assert!(
        (i32::from(near_outer_edge) - 151).abs() <= 1,
        "внешний край: {near_outer_edge}"
    );
}

#[test]
fn transparent_pixels_carry_their_angle_color_not_black() {
    let picture = ring();
    // Левый верхний угол лежит под углом 135°: ровно середина между зелёным (90°) и синим (180°).
    let corner = picture.texel(0, 0).unwrap();
    assert_eq!(corner[3], 0);
    assert_eq!(corner[0], 0);
    assert!(
        (i32::from(corner[1]) - 128).abs() <= 2 && (i32::from(corner[2]) - 128).abs() <= 2,
        "{corner:?}"
    );
}
