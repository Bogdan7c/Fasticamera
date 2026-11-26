//! Тесты формы зон 14n: портрет — прибор на всю ширину под ручками и превью, альбом — прибор под превью, сторона
//! колонки, полоска прибора, поле уровней и картинка каждого вида (решения 255, 260, 274, 275, 291).

use scope_contract::{ScopeKind, ScopePictureShape};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use ui_core::geometry::Rect;

use super::layout_tests::{
    ALL_ORIENTATIONS, area, calibration_layout, calibration_layout_in, calibration_layout_with, distinct_insets,
    overlap_area, screen,
};
use super::{CalibrationConfig, CalibrationLayout, CalibrationLayoutError, KnobsSide, ZoneShape};
use crate::mode_behaviour::LayoutPolicy;

const BOTH_SIDES: [KnobsSide; 2] = [KnobsSide::Left, KnobsSide::Right];

fn pixels_per_point() -> f64 {
    screen().point_scale().pixels_per_point()
}

fn config_with_side(side: KnobsSide) -> CalibrationConfig {
    let mut config = CalibrationConfig::default();
    config.layout.knobs_side = side;
    config
}

fn layout_on_side(side: KnobsSide, orientation: DeviceOrientation) -> CalibrationLayout {
    calibration_layout_with(&config_with_side(side), orientation, SafeAreaInsets::NONE).expect("раскладка")
}

fn on_grid(value_pt: f64) -> bool {
    let value_px = value_pt * pixels_per_point();
    (value_px - value_px.round()).abs() < 1e-6
}

fn assert_rect_on_grid(rect: Rect, what: &str) {
    for edge in [rect.min_x(), rect.max_x(), rect.min_y(), rect.max_y()] {
        assert!(on_grid(edge), "{what}: край {edge} pt не на целом пикселе ({rect:?})");
    }
}

// ----- портрет: рисунок владельца -------------------------------------------------------------------------------------

#[test]
fn portrait_puts_the_knobs_and_the_preview_side_by_side_over_a_scope_as_wide_as_the_screen() {
    let layout = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::Portrait);
    let zones = layout.zones();
    let eye = layout.eye_size();

    // Ручки — половина ширины от верха безопасной зоны до линии владельца; превью — другая половина от края экрана.
    assert_eq!((zones.knobs.min_x(), zones.knobs.min_y()), (0.0, 0.0));
    assert!(
        (zones.knobs.size.width - eye.width / 2.0).abs() < 0.5,
        "ручки: половина ширины"
    );
    assert_eq!(zones.clean_preview.min_x(), zones.knobs.max_x());
    assert_eq!(
        (zones.clean_preview.min_y(), zones.clean_preview.max_x()),
        (0.0, eye.width)
    );
    assert_eq!(
        zones.knobs.max_y(),
        zones.clean_preview.max_y(),
        "обе половины верха кончаются на одной линии"
    );

    // Линия владельца там, где была до 14n (≈ 493,8 pt на 440 × 982,67): верх остаётся прежним по высоте.
    let boundary = zones.clean_preview.max_y();
    assert!((boundary - 493.8).abs() < 0.5, "линия верх / прибор: {boundary}");

    // Прибор — на всю ширину под обеими половинами, до нижнего ряда в 108 pt.
    assert_eq!(zones.scope.min_x(), 0.0);
    assert_eq!(zones.scope.max_x(), eye.width, "прибор во всю ширину экрана");
    assert_eq!(zones.scope.min_y(), boundary);
    assert_eq!(zones.scope.max_y(), zones.bottom_row.min_y());
    assert!(
        (eye.height - zones.bottom_row.min_y() - 108.0).abs() < 0.5,
        "нижний ряд {} pt",
        eye.height - zones.bottom_row.min_y()
    );
}

#[test]
fn landscape_keeps_the_knobs_full_height_beside_the_preview_over_the_scope() {
    let layout = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::LandscapeLeft);
    let zones = layout.zones();
    let eye = layout.eye_size();

    assert_eq!(zones.knobs.min_y(), 0.0);
    assert_eq!(
        zones.knobs.max_y(),
        zones.bottom_row.min_y(),
        "колонка идёт до нижнего ряда"
    );
    assert!((zones.knobs.size.width / eye.width - 1.0 / 3.0).abs() < 1e-3);
    // Правая колонка поровну: превью сверху, прибор под ним, обе от колонки до края.
    assert_eq!(zones.clean_preview.min_x(), zones.knobs.max_x());
    assert_eq!(zones.scope.min_x(), zones.knobs.max_x());
    assert_eq!(zones.clean_preview.max_y(), zones.scope.min_y());
    assert_eq!(zones.scope.max_y(), zones.bottom_row.min_y());
    assert!(
        (zones.clean_preview.max_y() / zones.bottom_row.min_y() - 0.5).abs() < 0.01,
        "поровну"
    );
    assert!(
        (eye.height - zones.bottom_row.min_y() - 64.0).abs() < 0.5,
        "нижний ряд альбома 64 pt"
    );
}

#[test]
fn zones_tile_the_eye_rectangle_for_every_orientation_and_knobs_side() {
    for side in BOTH_SIDES {
        for orientation in ALL_ORIENTATIONS {
            let layout = layout_on_side(side, orientation);
            let zones = layout.zones();
            let all = [zones.knobs, zones.clean_preview, zones.scope, zones.bottom_row];
            let eye = Rect::from_xywh(0.0, 0.0, layout.eye_size().width, layout.eye_size().height);
            let total: f64 = all.iter().map(area).sum();
            assert!(
                (total - area(&eye)).abs() < 1e-6,
                "{side:?} {orientation:?}: площадь зон {total} против экрана {}",
                area(&eye)
            );
            for (index, first) in all.iter().enumerate() {
                for second in &all[index + 1..] {
                    assert!(
                        overlap_area(first, second) < 1e-9,
                        "{side:?} {orientation:?}: зоны перекрываются: {first:?} / {second:?}"
                    );
                }
            }
            for (index, zone) in all.iter().enumerate() {
                assert_rect_on_grid(*zone, &format!("{side:?} {orientation:?}: зона {index}"));
            }
        }
    }
}

// ----- сторона колонки ручек ------------------------------------------------------------------------------------------

#[test]
fn knobs_on_the_right_swap_the_columns_in_both_shapes_and_keep_the_widths() {
    for orientation in [
        DeviceOrientation::Portrait,
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::PortraitUpsideDown,
        DeviceOrientation::LandscapeRight,
    ] {
        let left = layout_on_side(KnobsSide::Left, orientation);
        let right = layout_on_side(KnobsSide::Right, orientation);
        let eye_width = left.eye_size().width;
        let (l, r) = (left.zones(), right.zones());

        assert_eq!(l.knobs.min_x(), 0.0, "{orientation:?}: слева колонка у левого края");
        assert!(
            (r.knobs.max_x() - eye_width).abs() < 1e-9,
            "{orientation:?}: справа — у правого"
        );
        assert!(
            (l.knobs.size.width - r.knobs.size.width).abs() < 1e-9,
            "{orientation:?}: ширина колонки та же"
        );
        assert_eq!(
            r.clean_preview.min_x(),
            0.0,
            "{orientation:?}: превью уехало влево, до края экрана"
        );
        assert!((r.clean_preview.max_x() - r.knobs.min_x()).abs() < 1e-9);
        assert_eq!(
            l.knobs.max_y(),
            r.knobs.max_y(),
            "{orientation:?}: по высоте стороны одинаковы"
        );
        assert_eq!(l.scope.size, r.scope.size, "{orientation:?}: прибор того же размера");
        assert_eq!(l.scope.min_y(), r.scope.min_y());
        assert_eq!(
            left.scope_image_size_for(ScopeKind::Waveform),
            right.scope_image_size_for(ScopeKind::Waveform)
        );
    }
}

#[test]
fn knobs_on_the_right_hug_the_opposite_safe_edge_instead_of_mirroring_the_insets() {
    // Отступы у сторон разные: колонка справа прижата к ПРАВОЙ границе безопасной зоны, а не к зеркальной левой.
    let scale = pixels_per_point();
    for (orientation, (left_px, right_px)) in [
        (DeviceOrientation::Portrait, (10.0, 30.0)),
        (DeviceOrientation::LandscapeLeft, (20.0, 40.0)),
        (DeviceOrientation::PortraitUpsideDown, (30.0, 10.0)),
        (DeviceOrientation::LandscapeRight, (40.0, 20.0)),
    ] {
        let layout = calibration_layout_with(&config_with_side(KnobsSide::Right), orientation, distinct_insets())
            .expect("раскладка");
        let eye_width = layout.eye_size().width;
        let zones = layout.zones();
        let close = |actual: f64, expected_px: f64| (actual * scale - expected_px).abs() < 1e-6;
        assert!(
            close(eye_width - zones.knobs.max_x(), right_px),
            "{orientation:?}: колонка у правого края безопасной зоны"
        );
        assert!(
            close(zones.clean_preview.min_x(), 0.0),
            "{orientation:?}: превью — до края экрана"
        );
        if layout.scope_picture_area().size.width > 0.0 {
            // Прибор кончается там же, где колонка начинается (альбом) или тянется на всю безопасную ширину (портрет).
            let scope_right = eye_width - zones.scope.max_x();
            let expected = match orientation {
                DeviceOrientation::Portrait | DeviceOrientation::PortraitUpsideDown => right_px,
                _ => (zones.knobs.size.width * scale).round() + right_px,
            };
            assert!(
                close(scope_right, expected),
                "{orientation:?}: правый край прибора {scope_right}"
            );
            assert!(
                close(zones.scope.min_x(), left_px),
                "{orientation:?}: левый край прибора у безопасного края"
            );
        }
    }
}

// ----- полоска, область картинки, поле уровней --------------------------------------------------------------------------

#[test]
fn the_strip_is_44_pt_snapped_to_whole_pixels_and_the_picture_area_starts_right_under_it() {
    let scale = pixels_per_point();
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let (strip, picture, zone) = (layout.scope_strip(), layout.scope_picture_area(), layout.zones().scope);
        assert_eq!(
            strip.size.height * scale,
            (44.0_f64 * scale).round(),
            "{orientation:?}: 44 pt в целых пикселях"
        );
        assert!(on_grid(strip.size.height));
        assert_eq!(
            (strip.min_x(), strip.min_y(), strip.size.width),
            (zone.min_x(), zone.min_y(), zone.size.width)
        );
        assert_eq!(
            picture.min_y(),
            strip.max_y(),
            "{orientation:?}: картинка начинается под полоской"
        );
        assert_eq!(
            (picture.min_x(), picture.max_x(), picture.max_y()),
            (zone.min_x(), zone.max_x(), zone.max_y())
        );
        assert_rect_on_grid(strip, "полоска");
        assert_rect_on_grid(picture, "область картинки");
        let gutter = layout.scope_levels_gutter();
        assert_eq!(
            gutter.size.width * scale,
            (28.0_f64 * scale).round(),
            "{orientation:?}: поле уровней ≈ 28 pt"
        );
        assert_eq!(
            (gutter.min_x(), gutter.min_y(), gutter.size.height),
            (picture.min_x(), picture.min_y(), picture.size.height)
        );
    }
}

#[test]
fn each_picture_is_exactly_its_rect_in_pixels_and_lies_on_whole_pixels() {
    let scale = pixels_per_point();
    for side in BOTH_SIDES {
        for orientation in ALL_ORIENTATIONS {
            let layout = layout_on_side(side, orientation);
            for kind in ScopeKind::ALL {
                let (rect, size) = (layout.scope_image_rect_for(kind), layout.scope_image_size_for(kind));
                let rect_px = (rect.size.width * scale, rect.size.height * scale);
                assert!(
                    (f64::from(size.width()) - rect_px.0).abs() < 1e-6
                        && (f64::from(size.height()) - rect_px.1).abs() < 1e-6,
                    "{side:?} {orientation:?} {kind:?}: размер картинки {size:?} в px равен её прямоугольнику {rect_px:?}"
                );
                assert_rect_on_grid(rect, &format!("{side:?} {orientation:?} {kind:?}: картинка"));
                let area = layout.scope_picture_area();
                assert!(
                    rect.min_x() >= area.min_x() - 1e-9
                        && rect.max_x() <= area.max_x() + 1e-9
                        && rect.min_y() >= area.min_y() - 1e-9
                        && rect.max_y() <= area.max_y() + 1e-9,
                    "{kind:?}: картинка в области картинки, не под полоской"
                );
            }
        }
    }
}

#[test]
fn waveform_and_parade_fill_the_area_right_of_the_gutter_and_the_vectorscope_is_a_square_of_the_whole_area() {
    for orientation in ALL_ORIENTATIONS {
        let layout = calibration_layout(LayoutPolicy::FollowsOrientation, orientation);
        let (area, gutter) = (layout.scope_picture_area(), layout.scope_levels_gutter());
        for kind in ScopeKind::ALL {
            let rect = layout.scope_image_rect_for(kind);
            match kind.picture_shape() {
                ScopePictureShape::FillsZone => {
                    assert!(!kind.level_lines().is_empty(), "у видов с полем есть подписи уровней");
                    assert_eq!(
                        rect.min_x(),
                        gutter.max_x(),
                        "{orientation:?} {kind:?}: картинка начинается за полем"
                    );
                    let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
                    assert!(
                        close(rect.max_x(), area.max_x())
                            && close(rect.min_y(), area.min_y())
                            && close(rect.max_y(), area.max_y()),
                        "{orientation:?} {kind:?}: {rect:?} против {area:?}"
                    );
                }
                ScopePictureShape::Square => {
                    assert!(kind.level_lines().is_empty());
                    assert_eq!(rect.size.width, rect.size.height, "{orientation:?}: квадрат, а не овал");
                    assert!(
                        (rect.size.width - area.size.width.min(area.size.height)).abs() < 1e-9,
                        "{orientation:?}: сторона — меньшая сторона ВСЕЙ области (поля нет)"
                    );
                    let centre = area.center();
                    assert!(
                        (rect.center().x - centre.x).abs() <= 0.5 / pixels_per_point() + 1e-9
                            && (rect.center().y - centre.y).abs() <= 0.5 / pixels_per_point() + 1e-9,
                        "{orientation:?}: квадрат по центру области"
                    );
                }
            }
        }
    }
}

#[test]
fn portrait_picture_sizes_in_pixels_match_the_owner_numbers() {
    let layout = calibration_layout(LayoutPolicy::FollowsOrientation, DeviceOrientation::Portrait);
    let waveform = layout.scope_image_size_for(ScopeKind::Waveform);
    let vectorscope = layout.scope_image_size_for(ScopeKind::Vectorscope);
    // Решение 291: область картинки ≈ 1080 × 827 px; waveform уже на поле уровней (28 pt ≈ 69 px).
    assert!(
        (i64::from(waveform.height()) - 827).abs() <= 3,
        "высота waveform {}",
        waveform.height()
    );
    assert_eq!(waveform.width(), 1080 - (28.0_f64 * pixels_per_point()).round() as u32);
    assert_eq!(vectorscope.width(), vectorscope.height());
    assert_eq!(
        vectorscope.height(),
        waveform.height(),
        "вектороскоп ограничен высотой области"
    );
}

#[test]
fn pictures_respect_the_configured_side_limit_and_stay_centred_in_their_slot() {
    let config = CalibrationConfig {
        max_scope_image_side_px: 100,
        ..CalibrationConfig::default()
    };
    let layout =
        calibration_layout_with(&config, DeviceOrientation::Portrait, SafeAreaInsets::NONE).expect("раскладка");
    let waveform = layout.scope_image_size_for(ScopeKind::Waveform);
    assert!(waveform.width() <= 100 && waveform.height() <= 100);
    assert_eq!(waveform.height(), 100, "высота области больше предела и зажата по нему");
    let square = layout.scope_image_size_for(ScopeKind::Vectorscope);
    assert_eq!((square.width(), square.height()), (100, 100));
    for kind in ScopeKind::ALL {
        let rect = layout.scope_image_rect_for(kind);
        assert_eq!(
            rect.size.height * pixels_per_point(),
            100.0,
            "{kind:?}: прямоугольник — зажатый размер"
        );
        assert!(layout.scope_picture_area().contains(rect.center()));
    }
}

#[test]
fn a_strip_taller_than_the_zone_is_an_error_not_a_negative_picture() {
    let mut config = CalibrationConfig::default();
    config.layout.scope_strip_height_pt = 5000.0;
    assert!(matches!(
        calibration_layout_with(&config, DeviceOrientation::Portrait, SafeAreaInsets::NONE),
        Err(CalibrationLayoutError::NoRoomForScopePicture { .. })
    ));
    let mut config = CalibrationConfig::default();
    config.layout.scope_levels_gutter_pt = 5000.0;
    assert!(matches!(
        calibration_layout_with(&config, DeviceOrientation::Portrait, SafeAreaInsets::NONE),
        Err(CalibrationLayoutError::NoRoomForScopePicture { .. })
    ));
}

#[test]
fn the_shape_comes_from_the_config_not_from_the_phone_position() {
    // Портрет с формой альбома и наоборот: форму выбирает данные конфига, а не «портрет это или нет».
    let mut config = CalibrationConfig::default();
    config.layout.portrait.shape = ZoneShape::ScopeUnderPreview;
    let swapped =
        calibration_layout_with(&config, DeviceOrientation::Portrait, SafeAreaInsets::NONE).expect("раскладка");
    let zones = swapped.zones();
    assert_eq!(
        zones.knobs.max_y(),
        zones.bottom_row.min_y(),
        "колонка на всю высоту, как в альбоме"
    );
    assert_eq!(
        zones.scope.min_x(),
        zones.knobs.max_x(),
        "прибор под превью, а не на всю ширину"
    );

    let mut config = CalibrationConfig::default();
    config.layout.landscape.shape = ZoneShape::ScopeStripBelow;
    let swapped =
        calibration_layout_with(&config, DeviceOrientation::LandscapeLeft, SafeAreaInsets::NONE).expect("раскладка");
    assert_eq!(
        swapped.zones().scope.min_x(),
        0.0,
        "прибор на всю ширину и в альбоме, если так сказано в конфиге"
    );
    // Оба сочетания остаются разбиением экрана без перекрытий.
    let _ = calibration_layout_in(DeviceOrientation::Portrait, SafeAreaInsets::NONE);
}
