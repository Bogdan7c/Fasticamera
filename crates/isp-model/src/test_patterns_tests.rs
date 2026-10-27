use super::*;
use crate::cfa::RgbChannel;

fn size(width: u32, height: u32) -> PixelSize {
    PixelSize::new(width, height)
}

#[test]
fn mosaic_keeps_only_the_filter_channel_for_every_layout() {
    let image = color_bars(size(14, 4), 1.0).unwrap();
    for pattern in [
        BayerPattern::RGGB,
        BayerPattern::GRBG,
        BayerPattern::GBRG,
        BayerPattern::BGGR,
    ] {
        let mosaic = mosaic_from_rgb(&image, pattern).unwrap();
        assert_eq!(mosaic.pattern(), pattern);
        for y in 0..4 {
            for x in 0..14 {
                assert_eq!(mosaic.sample(x, y), image.pixel(x, y)[pattern.channel_at(x, y).index()]);
            }
        }
    }
}

#[test]
fn mosaic_rejects_odd_sizes() {
    let image = flat_color(size(3, 2), [0.1, 0.2, 0.3]).unwrap();
    assert_eq!(
        mosaic_from_rgb(&image, BayerPattern::RGGB),
        Err(ImageError::OddMosaicSize { width: 3, height: 2 })
    );
}

#[test]
fn zone_plate_is_gray_in_range_and_bright_in_the_centre() {
    let image = zone_plate(size(64, 64), 0.5).unwrap();
    for pixel in image.pixels() {
        assert_eq!(pixel[0], pixel[1]);
        assert_eq!(pixel[1], pixel[2]);
        assert!((-1e-6..=1.0 + 1e-6).contains(&pixel[0]));
    }
    // Центр кадра (r ≈ 0) — максимум косинуса: яркость ≈ 1.
    assert!(image.pixel(32, 32)[0] > 0.99);
}

#[test]
fn zone_plate_frequency_grows_with_radius() {
    // Считаем смены знака отклонения от среднего вдоль средней строки: у края их больше, чем у центра.
    let image = zone_plate(size(128, 128), 0.5).unwrap();
    let crossings = |range: std::ops::Range<u32>| {
        range
            .filter(|&x| (image.pixel(x, 64)[0] - 0.5) * (image.pixel(x + 1, 64)[0] - 0.5) < 0.0)
            .count()
    };
    assert!(crossings(88..127) > 2 * crossings(64..103).max(1));
}

#[test]
fn slanted_edge_splits_dark_and_bright_with_the_requested_slope() {
    let image = slanted_edge(size(32, 32), 0.0, 0.2, 0.8).unwrap();
    assert_eq!(image.pixel(2, 10), [0.2; 3]);
    assert_eq!(image.pixel(29, 10), [0.8; 3]);
    // Вертикальный край: строка не влияет на значение.
    assert_eq!(image.pixel(10, 3), image.pixel(10, 28));
    // Наклон 45°: по главной диагонали, перпендикулярной нормали, значение постоянно.
    let tilted = slanted_edge(size(32, 32), 45.0, 0.0, 1.0).unwrap();
    assert!((tilted.pixel(8, 8)[0] - tilted.pixel(20, 20)[0]).abs() < 1e-5);
    assert!(tilted.pixel(2, 2)[0] != tilted.pixel(29, 2)[0]);
}

#[test]
fn color_bars_use_all_seven_saturated_colors() {
    let image = color_bars(size(14, 2), 0.5).unwrap();
    assert_eq!(image.pixel(0, 0), [0.5, 0.5, 0.5]);
    assert_eq!(image.pixel(13, 1), [0.0, 0.0, 0.5]);
    let distinct: std::collections::HashSet<[u32; 3]> =
        image.pixels().iter().map(|pixel| pixel.map(f32::to_bits)).collect();
    assert_eq!(distinct.len(), 7);
    assert_eq!(BayerPattern::RGGB.channel_at(0, 0), RgbChannel::Red);
}

#[test]
fn empty_size_is_an_error_not_a_panic() {
    assert!(zone_plate(size(0, 4), 0.5).is_err());
}
