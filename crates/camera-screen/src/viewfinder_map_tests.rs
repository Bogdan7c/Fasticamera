//! Тесты перевода «экран ↔ сенсор»: центры, края, обратимость, ориентации, зеркало, отказы.

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::three_a::SensorPoint;
use camera_contract::viewfinder_crop::ViewfinderCrop;
use ui_core::geometry::{Point, Rect};

use crate::layout::{LayoutConfig, ScreenLayout};
use crate::settings::AspectRatio;
use crate::test_support::{OUR_SURFACE, main_camera_stream};
use crate::viewfinder_map::{MappingError, MirrorMode, StreamGeometry, ViewfinderMapping};

const ORIENTATIONS: [SensorOrientation; 4] = [
    SensorOrientation::Degrees0,
    SensorOrientation::Degrees90,
    SensorOrientation::Degrees180,
    SensorOrientation::Degrees270,
];

fn layout() -> ScreenLayout {
    ScreenLayout::new(&LayoutConfig::default(), OUR_SURFACE).expect("раскладка")
}

fn mapping_for(aspect: AspectRatio, stream: &StreamGeometry) -> (ViewfinderMapping, Rect) {
    let layout = layout();
    let frame = layout.frame_rect(aspect);
    let crop = layout.framing_intent(aspect).crop;
    (ViewfinderMapping::new(frame, &crop, stream).expect("перевод"), frame)
}

fn stream_with(orientation: SensorOrientation, mirror: MirrorMode) -> StreamGeometry {
    StreamGeometry {
        sensor_orientation: orientation,
        mirror,
        ..main_camera_stream()
    }
}

/// Доли кадрирования (0..1 по сенсору) → `SensorPoint` в долях активной области.
fn sensor_point_of_crop_fraction(crop: PixelRect, active: PixelSize, fraction_x: f64, fraction_y: f64) -> (f64, f64) {
    let x = f64::from(crop.left) + fraction_x * f64::from(crop.size.width);
    let y = f64::from(crop.top) + fraction_y * f64::from(crop.size.height);
    (x / f64::from(active.width), y / f64::from(active.height))
}

fn assert_close(actual: (f32, f32), expected: (f64, f64), context: &str) {
    assert!(
        (f64::from(actual.0) - expected.0).abs() < 1e-5 && (f64::from(actual.1) - expected.1).abs() < 1e-5,
        "{context}: получено {actual:?}, ожидалось {expected:?}"
    );
}

#[test]
fn frame_center_maps_to_crop_center_for_every_aspect_and_orientation() {
    for orientation in ORIENTATIONS {
        for aspect in AspectRatio::ALL {
            let stream = stream_with(orientation, MirrorMode::None);
            let (mapping, frame) = mapping_for(aspect, &stream);
            let sensor = mapping.screen_to_sensor(frame.center()).expect("центр кадра");
            let expected = sensor_point_of_crop_fraction(mapping.crop_rect(), stream.active_array_size, 0.5, 0.5);
            assert_close(
                (sensor.x(), sensor.y()),
                expected,
                &format!("{aspect:?} {orientation:?}"),
            );
        }
    }
}

#[test]
fn frame_corners_map_to_crop_corners_for_90_and_270_degree_sensors() {
    // Углы прямого кадра: (u, v) → доли кадрирования сенсора. Таблица — из поворота кадра по часовой
    // (90°: левый нижний угол сенсора встаёт вверх-влево) и против часовой (270°).
    let corners_by_turns: [(SensorOrientation, [(f64, f64); 4]); 2] = [
        // порядок углов экрана: левый верхний, правый верхний, левый нижний, правый нижний
        (
            SensorOrientation::Degrees90,
            [(0.0, 1.0), (0.0, 0.0), (1.0, 1.0), (1.0, 0.0)],
        ),
        (
            SensorOrientation::Degrees270,
            [(1.0, 0.0), (1.0, 1.0), (0.0, 0.0), (0.0, 1.0)],
        ),
    ];
    for (orientation, expected_fractions) in corners_by_turns {
        for aspect in AspectRatio::ALL {
            let stream = stream_with(orientation, MirrorMode::None);
            let (mapping, frame) = mapping_for(aspect, &stream);
            let screen_corners = [
                Point::new(frame.min_x(), frame.min_y()),
                Point::new(frame.max_x(), frame.min_y()),
                Point::new(frame.min_x(), frame.max_y()),
                Point::new(frame.max_x(), frame.max_y()),
            ];
            for (corner, (fraction_x, fraction_y)) in screen_corners.into_iter().zip(expected_fractions) {
                let sensor = mapping.screen_to_sensor(corner).expect("угол кадра");
                let expected = sensor_point_of_crop_fraction(
                    mapping.crop_rect(),
                    stream.active_array_size,
                    fraction_x,
                    fraction_y,
                );
                assert_close(
                    (sensor.x(), sensor.y()),
                    expected,
                    &format!("{aspect:?} {orientation:?} {corner:?}"),
                );
            }
        }
    }
}

#[test]
fn crop_rects_on_a_4096_by_3072_sensor_match_the_aspects() {
    let stream = stream_with(SensorOrientation::Degrees90, MirrorMode::None);
    let size_of = |aspect| mapping_for(aspect, &stream).0.crop_rect().size;
    assert_eq!(size_of(AspectRatio::FourByThree), PixelSize::new(4096, 3072));
    assert_eq!(size_of(AspectRatio::SixteenByNine), PixelSize::new(4096, 2304));
    assert_eq!(size_of(AspectRatio::Square), PixelSize::new(3072, 3072));
}

#[test]
fn screen_to_sensor_and_back_returns_the_same_point_within_one_screen_pixel() {
    let layout = layout();
    let pixels_per_point = layout.point_scale().pixels_per_point();
    let mirrors = [MirrorMode::None, MirrorMode::Horizontal];
    for orientation in ORIENTATIONS {
        for mirror in mirrors {
            for aspect in AspectRatio::ALL {
                let stream = stream_with(orientation, mirror);
                let (mapping, frame) = mapping_for(aspect, &stream);
                for step_x in 0..=8 {
                    for step_y in 0..=8 {
                        let point = Point::new(
                            frame.min_x() + frame.size.width * f64::from(step_x) / 8.0,
                            frame.min_y() + frame.size.height * f64::from(step_y) / 8.0,
                        );
                        let sensor = mapping.screen_to_sensor(point).expect("точка в кадре");
                        let back = mapping.sensor_to_screen(sensor).expect("точка в кадрировании");
                        let error_px = point.distance_to(back) * pixels_per_point;
                        assert!(
                            error_px < 1.0,
                            "{aspect:?} {orientation:?} {mirror:?} {point:?}: ошибка {error_px} px"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn mirror_flips_horizontally_on_screen_before_rotation() {
    let plain = stream_with(SensorOrientation::Degrees0, MirrorMode::None);
    let mirrored = stream_with(SensorOrientation::Degrees0, MirrorMode::Horizontal);
    let (plain_mapping, frame) = mapping_for(AspectRatio::FourByThree, &plain);
    let (mirrored_mapping, _) = mapping_for(AspectRatio::FourByThree, &mirrored);
    let left = Point::new(frame.min_x() + frame.size.width * 0.25, frame.center().y);
    let right = Point::new(frame.min_x() + frame.size.width * 0.75, frame.center().y);
    let plain_left = plain_mapping.screen_to_sensor(left).expect("левая");
    let mirrored_right = mirrored_mapping.screen_to_sensor(right).expect("правая");
    assert!((plain_left.x() - mirrored_right.x()).abs() < 1e-6);
    assert!((plain_left.y() - mirrored_right.y()).abs() < 1e-6);
}

#[test]
fn valid_area_shorter_than_buffer_is_measured_in_active_array_pixels() {
    // 16:9@60 основной камеры: годны строки 0–2303 из 3072. Центр кадра — строка 1152 из 3072 = 0,375.
    let mut stream = stream_with(SensorOrientation::Degrees90, MirrorMode::None);
    stream.valid_area = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(4096, 2304),
    };
    let (mapping, frame) = mapping_for(AspectRatio::FourByThree, &stream);
    // 4:3 внутри 4096×2304 — вся ширина невозможна: кадрирование 3072×2304 по центру по горизонтали.
    assert_eq!(mapping.crop_rect().size, PixelSize::new(3072, 2304));
    let sensor = mapping.screen_to_sensor(frame.center()).expect("центр");
    assert!((sensor.x() - 0.5).abs() < 1e-6);
    assert!((sensor.y() - 0.375).abs() < 1e-6);
}

#[test]
fn tap_outside_the_frame_is_an_explicit_error() {
    let (mapping, frame) = mapping_for(AspectRatio::FourByThree, &main_camera_stream());
    let above = Point::new(frame.center().x, frame.min_y() - 1.0);
    let below = Point::new(frame.center().x, frame.max_y() + 1.0);
    assert_eq!(mapping.screen_to_sensor(above), Err(MappingError::OutsideFrame));
    assert_eq!(mapping.screen_to_sensor(below), Err(MappingError::OutsideFrame));
}

#[test]
fn sensor_point_outside_the_visible_crop_is_an_explicit_error() {
    // В 16:9 верхняя часть сенсора (y < 0,125) в кадр не попадает.
    let (mapping, _) = mapping_for(AspectRatio::SixteenByNine, &main_camera_stream());
    let hidden = SensorPoint::new(0.5, 0.05).expect("точка");
    assert_eq!(mapping.sensor_to_screen(hidden), Err(MappingError::OutsideCrop));
    let visible = SensorPoint::new(0.5, 0.5).expect("точка");
    assert!(mapping.sensor_to_screen(visible).is_ok());
}

#[test]
fn degenerate_active_array_and_bad_valid_area_are_errors() {
    let layout = layout();
    let frame = layout.frame_rect(AspectRatio::FourByThree);
    let crop = ViewfinderCrop::FULL_SENSOR;
    let mut stream = main_camera_stream();
    stream.active_array_size = PixelSize::new(0, 3072);
    assert!(matches!(
        ViewfinderMapping::new(frame, &crop, &stream),
        Err(MappingError::DegenerateActiveArray { .. })
    ));
    let mut stream = main_camera_stream();
    stream.valid_area.left = 1;
    assert!(matches!(
        ViewfinderMapping::new(frame, &crop, &stream),
        Err(MappingError::Crop(_))
    ));
}

// ===== Независимая проверка по числам, выведенным вручную =========================================
//
// Таблицы выше строятся из того же `quarter_turns`, что и реализация, поэтому ошибка в самой таблице
// поворота их бы не поймала. Эти тесты держат конкретные числа, выведенные из шейдера
// `isp-vulkan/shaders/viewfinder_isp.glsl::cell_of_output`.
//
// Вывод для OnePlus 11 (сенсор 4096×3072, ориентация 90° = 1 четверть по часовой, `turns == 1`):
//   шейдер: ячейка сенсора для пикселя прямого кадра `(x, y)` = `(y, H − 1 − x)`, `H` — высота сенсора;
//   в долях: прямой кадр `(u, v)` ↔ сенсор `(sx, sy) = (v, 1 − u)`.
//   Проверка на углах прямого кадра (portrait, верх телефона — вверху экрана):
//     левый верхний `(0, 0)` → `(sx, sy) = (0, 1)`: левый нижний угол активной области;
//     правый верхний `(1, 0)` → `(0, 0)`; левый нижний `(0, 1)` → `(1, 1)`; правый нижний `(1, 1)` → `(1, 0)`.
//   Кадрирование считается от буфера: 4:3 — весь буфер; 16:9 — 4096×2304 со сдвигом по высоте
//   (3072 − 2304) / 2 = 384; 1:1 — 3072×3072 со сдвигом по ширине (4096 − 3072) / 2 = 512.
//   Доли активной области: `sx_active = (crop.left + sx · crop.width) / 4096`,
//   `sy_active = (crop.top + sy · crop.height) / 3072`.

fn corner_points(frame: Rect) -> [Point; 4] {
    [
        Point::new(frame.min_x(), frame.min_y()),
        Point::new(frame.max_x(), frame.min_y()),
        Point::new(frame.min_x(), frame.max_y()),
        Point::new(frame.max_x(), frame.max_y()),
    ]
}

fn point_in_frame(frame: Rect, u: f64, v: f64) -> Point {
    Point::new(
        frame.min_x() + u * frame.size.width,
        frame.min_y() + v * frame.size.height,
    )
}

fn assert_sensor(actual: SensorPoint, expected: (f32, f32), context: &str) {
    assert!(
        (actual.x() - expected.0).abs() < 1e-5 && (actual.y() - expected.1).abs() < 1e-5,
        "{context}: получено ({}, {}), ожидалось {expected:?}",
        actual.x(),
        actual.y()
    );
}

#[test]
fn four_by_three_frame_corners_land_on_hand_derived_corners_of_the_active_array() {
    let (mapping, frame) = mapping_for(AspectRatio::FourByThree, &main_camera_stream());
    // Порядок: левый верхний, правый верхний, левый нижний, правый нижний (вывод — в комментарии выше).
    let expected = [(0.0, 1.0), (0.0, 0.0), (1.0, 1.0), (1.0, 0.0)];
    for (corner, want) in corner_points(frame).into_iter().zip(expected) {
        let sensor = mapping.screen_to_sensor(corner).expect("угол кадра");
        assert_sensor(sensor, want, &format!("{corner:?}"));
        // И обратно: точка сенсора возвращается в тот же угол кадра.
        let back = mapping
            .sensor_to_screen(SensorPoint::new(want.0, want.1).expect("точка"))
            .expect("в кадре");
        assert!(
            back.distance_to(corner) < 1e-3,
            "{want:?} → {back:?}, ожидался {corner:?}"
        );
    }
}

#[test]
fn off_diagonal_tap_matches_hand_derived_numbers_for_every_aspect() {
    // Прямой кадр (u, v) = (0,1; 0,3) → кадрирование (sx, sy) = (0,3; 0,9).
    let stream = main_camera_stream();
    let cases = [
        // 4:3: (0,3·4096)/4096 = 0,3; 0,9·3072/3072 = 0,9.
        (AspectRatio::FourByThree, (0.3, 0.9)),
        // 16:9: x как у 4:3 (0,3); y = (384 + 0,9·2304)/3072 = 2457,6/3072 = 0,8.
        (AspectRatio::SixteenByNine, (0.3, 0.8)),
        // 1:1: x = (512 + 0,3·3072)/4096 = 1433,6/4096 = 0,35; y = 0,9.
        (AspectRatio::Square, (0.35, 0.9)),
    ];
    for (aspect, want) in cases {
        let (mapping, frame) = mapping_for(aspect, &stream);
        let sensor = mapping
            .screen_to_sensor(point_in_frame(frame, 0.1, 0.3))
            .expect("точка в кадре");
        assert_sensor(sensor, want, &format!("{aspect:?}"));
        let back = mapping
            .sensor_to_screen(SensorPoint::new(want.0, want.1).expect("точка"))
            .expect("в кадрировании");
        assert!(
            back.distance_to(point_in_frame(frame, 0.1, 0.3)) < 1e-3,
            "{aspect:?}: обратно {back:?}"
        );
    }
}

#[test]
fn sixteen_by_nine_top_left_corner_skips_the_cropped_strip_of_the_sensor() {
    // Левый верхний угол кадра 16:9 → кадрирование (0, 1) → буфер (0; 384 + 2304 = 2688) → y = 2688 / 3072 = 0,875.
    let (mapping, frame) = mapping_for(AspectRatio::SixteenByNine, &main_camera_stream());
    let sensor = mapping
        .screen_to_sensor(Point::new(frame.min_x(), frame.min_y()))
        .expect("угол");
    assert_sensor(sensor, (0.0, 0.875), "16:9, левый верхний");
}

#[test]
fn rotation_270_and_180_match_hand_derived_corners() {
    // turns == 3: ячейка = (W − 1 − y, x) → (sx, sy) = (1 − v, u); левый верхний (0,0) → (1, 0).
    let (mapping, frame) = mapping_for(
        AspectRatio::FourByThree,
        &stream_with(SensorOrientation::Degrees270, MirrorMode::None),
    );
    let top_left = mapping
        .screen_to_sensor(Point::new(frame.min_x(), frame.min_y()))
        .expect("угол");
    assert_sensor(top_left, (1.0, 0.0), "270°, левый верхний");
    // turns == 2: ячейка = (W − 1 − x, H − 1 − y) → (1 − u, 1 − v); левый верхний → (1, 1).
    let (mapping, frame) = mapping_for(
        AspectRatio::FourByThree,
        &stream_with(SensorOrientation::Degrees180, MirrorMode::None),
    );
    let top_left = mapping
        .screen_to_sensor(Point::new(frame.min_x(), frame.min_y()))
        .expect("угол");
    assert_sensor(top_left, (1.0, 1.0), "180°, левый верхний");
}
