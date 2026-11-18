use camera_contract::three_a::SensorPoint;

use super::*;

const MAIN_ARRAY: PixelSize = PixelSize::new(4096, 3072);

fn region(x: f32, y: f32, size: f32) -> MeteringRegion {
    MeteringRegion::new(SensorPoint::new(x, y).unwrap(), size).unwrap()
}

#[test]
fn centered_region_is_a_square_around_the_center_with_max_weight() {
    // Сторона = 10 % от меньшей стороны 3072 = 307 px; центр (2048, 1536).
    let entry = region_to_active_array(region(0.5, 0.5, 0.1), MAIN_ARRAY).unwrap();
    assert_eq!(entry, [1895, 1383, 2202, 1690, METERING_WEIGHT_MAX]);
    // Квадрат: ширина и высота совпадают, центр совпадает с центром тапа.
    assert_eq!(entry[2] - entry[0], entry[3] - entry[1]);
}

#[test]
fn corner_regions_are_clipped_to_the_border_not_shifted() {
    let top_left = region_to_active_array(region(0.0, 0.0, 0.2), MAIN_ARRAY).unwrap();
    // 20 % от 3072 = 614 px; половина (307) уходит за край и обрезается: остаётся 307.
    assert_eq!(top_left, [0, 0, 307, 307, METERING_WEIGHT_MAX]);
    let bottom_right = region_to_active_array(region(1.0, 1.0, 0.2), MAIN_ARRAY).unwrap();
    assert_eq!(bottom_right, [3789, 2765, 4096, 3072, METERING_WEIGHT_MAX]);
}

#[test]
fn full_frame_region_spans_the_whole_array_with_exclusive_end() {
    let entry = region_to_active_array(region(0.5, 0.5, 1.0), MAIN_ARRAY).unwrap();
    // Сторона 3072 по обеим осям вокруг центра: по X обрезка не нужна, по Y — ровно весь кадр.
    assert_eq!(entry, [512, 0, 3584, 3072, METERING_WEIGHT_MAX]);
    assert_eq!(
        entry[3], 3072,
        "ymax исключительный: равен высоте, а не высоте минус один"
    );
}

#[test]
fn tiny_region_is_never_smaller_than_one_pixel() {
    let entry = region_to_active_array(region(0.5, 0.5, f32::MIN_POSITIVE), MAIN_ARRAY).unwrap();
    assert_eq!(entry[2] - entry[0], 1);
    assert_eq!(entry[3] - entry[1], 1);
}

#[test]
fn region_at_the_far_edge_keeps_one_pixel_inside_the_array() {
    let entry = region_to_active_array(region(1.0, 1.0, f32::MIN_POSITIVE), MAIN_ARRAY).unwrap();
    assert_eq!(entry, [4095, 3071, 4096, 3072, METERING_WEIGHT_MAX]);
}

#[test]
fn degenerate_active_array_gives_no_region() {
    assert_eq!(
        region_to_active_array(region(0.5, 0.5, 0.1), PixelSize::new(0, 3072)),
        None
    );
    assert_eq!(
        region_to_active_array(region(0.5, 0.5, 0.1), PixelSize::new(4096, 0)),
        None
    );
}

#[test]
fn portrait_sized_array_uses_the_shorter_side_for_the_square() {
    let entry = region_to_active_array(region(0.5, 0.5, 0.5), PixelSize::new(1000, 2000)).unwrap();
    // Короткая сторона 1000 → квадрат 500×500 вокруг (500, 1000).
    assert_eq!(entry, [250, 750, 750, 1250, METERING_WEIGHT_MAX]);
}
