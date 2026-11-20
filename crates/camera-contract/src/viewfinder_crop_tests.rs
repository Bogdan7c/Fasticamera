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
        center: CropCenter::ValidAreaCenter,
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
        center: CropCenter::ValidAreaCenter,
    };
    assert_eq!(
        bad_aspect.resolve(SENSOR_4_3),
        Err(ViewfinderCropError::InvalidAspect { long: 3, short: 4 })
    );
    let zero_short = ViewfinderCrop {
        aspect: CropAspect::LongToShort { long: 4, short: 0 },
        zoom: ZoomFactor::NONE,
        center: CropCenter::ValidAreaCenter,
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

#[test]
fn center_on_optical_axis_shifts_crop_toward_axis() {
    // Ширик: ось 1759,3 / 1494,8 из 4000×3000; зум 1,69 (≈ 1× основной) — кадрирование 2366×1774.
    let area = rect(0, 0, 4000, 3000);
    let crop = ViewfinderCrop {
        aspect: CropAspect::FOUR_BY_THREE,
        zoom: ZoomFactor::new(1.69).unwrap(),
        center: CropCenter::BufferPixel { x: 1759.3, y: 1494.8 },
    };
    let result = crop.resolve(area).unwrap();
    assert_inside_and_even(result, area);
    let center_x = f64::from(result.left) + f64::from(result.size.width) / 2.0;
    let center_y = f64::from(result.top) + f64::from(result.size.height) / 2.0;
    // Чётное выравнивание сдвигает центр не больше чем на 1 пиксель.
    assert!((center_x - 1759.3).abs() <= 1.5, "центр x {center_x}");
    assert!((center_y - 1494.8).abs() <= 1.5, "центр y {center_y}");
}

#[test]
fn center_near_edge_keeps_crop_inside_area() {
    // Ось у самого края: прямоугольник сдвигается внутрь, а не обрезается и не уменьшается.
    let area = rect(0, 0, 4000, 3000);
    let crop = ViewfinderCrop {
        aspect: CropAspect::FOUR_BY_THREE,
        zoom: ZoomFactor::new(2.0).unwrap(),
        center: CropCenter::BufferPixel { x: 10.0, y: 2990.0 },
    };
    let result = crop.resolve(area).unwrap();
    assert_eq!(result, rect(0, 1500, 2000, 1500));
}

#[test]
fn non_finite_center_falls_back_to_area_center() {
    let centered = crop(CropAspect::FOUR_BY_THREE, 2.0).resolve(SENSOR_4_3).unwrap();
    let broken = ViewfinderCrop {
        center: CropCenter::BufferPixel { x: f32::NAN, y: 0.0 },
        ..crop(CropAspect::FOUR_BY_THREE, 2.0)
    };
    let result = broken.resolve(SENSOR_4_3).unwrap();
    assert_eq!(result.left, centered.left, "NaN по x — центр области");
    assert_eq!(result.top, 0, "y = 0 — край, сдвиг внутрь");
}
