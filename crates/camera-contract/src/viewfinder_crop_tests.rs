use super::*;

/// Полезная область 4:3 основной камеры OnePlus 11.
const SENSOR_4_3: PixelRect = PixelRect {
    left: 0,
    top: 0,
    size: PixelSize::new(4096, 3072),
};

/// Режим 16:9: строки 0–2303 буфера 4096×3072 (00 C2).
const SENSOR_16_9: PixelRect = PixelRect {
    left: 0,
    top: 0,
    size: PixelSize::new(4096, 2304),
};

fn crop(aspect: CropAspect, zoom: f32) -> ViewfinderCrop {
    ViewfinderCrop {
        aspect,
        zoom: ZoomFactor::new(zoom).unwrap(),
    }
}

fn rect(left: u32, top: u32, width: u32, height: u32) -> PixelRect {
    PixelRect {
        left,
        top,
        size: PixelSize::new(width, height),
    }
}

/// Прямоугольник внутри области, все края чётные.
fn assert_inside_and_even(result: PixelRect, area: PixelRect) {
    for value in [result.left, result.top, result.size.width, result.size.height] {
        assert_eq!(value % 2, 0, "{result:?}: нечётный край");
    }
    assert!(
        result.left >= area.left && result.top >= area.top,
        "{result:?} вне {area:?}"
    );
    assert!(result.left + result.size.width <= area.left + area.size.width);
    assert!(result.top + result.size.height <= area.top + area.size.height);
}

#[test]
fn full_sensor_without_zoom_is_the_whole_valid_area() {
    assert_eq!(ViewfinderCrop::FULL_SENSOR.resolve(SENSOR_4_3), Ok(SENSOR_4_3));
    let offset = rect(64, 32, 4000, 3000);
    assert_eq!(ViewfinderCrop::FULL_SENSOR.resolve(offset), Ok(offset));
}

#[test]
fn aspect_ratios_cut_the_largest_centered_rect() {
    // 4:3 на 4:3 — вся область.
    assert_eq!(crop(CropAspect::FOUR_BY_THREE, 1.0).resolve(SENSOR_4_3), Ok(SENSOR_4_3));
    // 16:9 на 4:3: ширина вся, высота 4096·9/16 = 2304, по центру: (3072 − 2304)/2 = 384.
    assert_eq!(
        crop(CropAspect::SIXTEEN_BY_NINE, 1.0).resolve(SENSOR_4_3),
        Ok(rect(0, 384, 4096, 2304))
    );
    // 1:1 — квадрат по короткой стороне: 3072, по центру по ширине: (4096 − 3072)/2 = 512.
    assert_eq!(
        crop(CropAspect::SQUARE, 1.0).resolve(SENSOR_4_3),
        Ok(rect(512, 0, 3072, 3072))
    );
    // 4:3 на 16:9: высота вся, ширина 2304·4/3 = 3072, сдвиг 512.
    assert_eq!(
        crop(CropAspect::FOUR_BY_THREE, 1.0).resolve(SENSOR_16_9),
        Ok(rect(512, 0, 3072, 2304))
    );
}

#[test]
fn aspect_follows_sensor_orientation_not_the_numbers_order() {
    // Портретная область (высокая): 16:9 кладётся длинной стороной вдоль высоты.
    let portrait = rect(0, 0, 3072, 4096);
    assert_eq!(
        crop(CropAspect::SIXTEEN_BY_NINE, 1.0).resolve(portrait),
        Ok(rect(384, 0, 2304, 4096))
    );
}

#[test]
fn zoom_shrinks_both_sides_around_the_center() {
    // 2× на 4:3: 2048×1536, по центру (1024, 768).
    assert_eq!(
        crop(CropAspect::SensorNative, 2.0).resolve(SENSOR_4_3),
        Ok(rect(1024, 768, 2048, 1536))
    );
    // 3× даёт 1365,33×1024 → 1365 → чётное 1364; центр: (4096 − 1364)/2 = 1366 (уже чётно).
    let zoomed = crop(CropAspect::SensorNative, 3.0).resolve(SENSOR_4_3).unwrap();
    assert_eq!(zoomed, rect(1366, 1024, 1364, 1024));
    assert_inside_and_even(zoomed, SENSOR_4_3);
}

#[test]
fn every_result_is_cell_aligned_and_inside_an_offset_area() {
    let area = rect(10, 6, 1002, 774);
    for aspect in [
        CropAspect::SensorNative,
        CropAspect::FOUR_BY_THREE,
        CropAspect::SIXTEEN_BY_NINE,
        CropAspect::SQUARE,
        CropAspect::LongToShort { long: 7, short: 5 },
    ] {
        for zoom in [1.0, 1.1, 1.5, 2.7, 10.0, 77.7] {
            let result = crop(aspect, zoom).resolve(area).unwrap();
            assert_inside_and_even(result, area);
            // Зум не увеличивает кадрирование.
            let unzoomed = crop(aspect, 1.0).resolve(area).unwrap();
            assert!(result.size.width <= unzoomed.size.width && result.size.height <= unzoomed.size.height);
        }
    }
}

#[test]
fn invalid_inputs_are_explicit_errors() {
    assert!(ZoomFactor::new(0.99).is_err());
    assert!(ZoomFactor::new(f32::NAN).is_err());
    assert!(ZoomFactor::new(f32::INFINITY).is_err());
    assert_eq!(ZoomFactor::new(1.0).map(ZoomFactor::value), Ok(1.0));

    let bad_aspect = ViewfinderCrop {
        aspect: CropAspect::LongToShort { long: 3, short: 4 },
        zoom: ZoomFactor::NONE,
    };
    assert_eq!(
        bad_aspect.resolve(SENSOR_4_3),
        Err(ViewfinderCropError::InvalidAspect { long: 3, short: 4 })
    );
    let zero_short = ViewfinderCrop {
        aspect: CropAspect::LongToShort { long: 4, short: 0 },
        zoom: ZoomFactor::NONE,
    };
    assert!(matches!(
        zero_short.resolve(SENSOR_4_3),
        Err(ViewfinderCropError::InvalidAspect { .. })
    ));

    let odd_area = rect(1, 0, 100, 100);
    assert_eq!(
        ViewfinderCrop::FULL_SENSOR.resolve(odd_area),
        Err(ViewfinderCropError::ValidAreaNotCellAligned { area: odd_area })
    );
    // Зум, после которого не остаётся ни одной ячейки 2×2.
    assert!(matches!(
        crop(CropAspect::SensorNative, 3.0).resolve(rect(0, 0, 4, 4)),
        Err(ViewfinderCropError::CropTooSmall { .. })
    ));
}
