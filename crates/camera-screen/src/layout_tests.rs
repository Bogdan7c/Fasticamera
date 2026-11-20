//! Тесты раскладки: числа `layout.md` 4.3, привязка к низу, границы и ошибки.

use camera_contract::viewfinder_crop::CropAspect;
use shell_contract::surface::SurfaceSize;

use crate::layout::{LayoutConfig, LayoutError, ScreenLayout};
use crate::settings::AspectRatio;
use crate::test_support::OUR_SURFACE;

fn our_layout() -> ScreenLayout {
    ScreenLayout::new(&LayoutConfig::default(), OUR_SURFACE).expect("раскладка")
}

fn frame_px(layout: &ScreenLayout, aspect: AspectRatio) -> (f64, f64) {
    let rect = layout.frame_rect(aspect);
    let scale = layout.point_scale();
    (scale.to_pixels(rect.min_y()), scale.to_pixels(rect.max_y()))
}

#[test]
fn screen_is_440_pt_wide_and_taller_than_iphone_by_26_7_pt() {
    let layout = our_layout();
    assert!((layout.screen_size().width - 440.0).abs() < 1e-9);
    assert!((layout.screen_size().height - 982.67).abs() < 0.01);
    assert!((layout.bottom_anchor_shift_pt() - 26.67).abs() < 0.01);
}

#[test]
fn frame_16x9_matches_layout_md_table_4_3() {
    // layout.md 4.3: 16:9 → 102,4–884,9 pt, 251–2172 px. Расхождение с таблицей — только округление
    // самой таблицы до целых пикселей (в расчёте 251,3 и 2171,9).
    let layout = our_layout();
    let rect = layout.frame_rect(AspectRatio::SixteenByNine);
    assert!((rect.min_y() - 102.4).abs() < 0.1, "верх {} pt", rect.min_y());
    assert!((rect.max_y() - 884.9).abs() < 0.1, "низ {} pt", rect.max_y());
    let (top_px, bottom_px) = frame_px(&layout, AspectRatio::SixteenByNine);
    assert!((top_px - 251.0).abs() < 1.0, "верх {top_px} px");
    assert!((bottom_px - 2172.0).abs() < 1.0, "низ {bottom_px} px");
}

#[test]
fn frame_4x3_matches_layout_md_table_4_3_within_rounding() {
    // [measured: R00 t=10.027] кадр 4:3 = 159,3–746,0 pt эталона; наш экран выше на 26,7 pt, раскладка привязана
    // к низу: 186,0–772,7 pt, то есть 456,5–1896,6 px (ширина 1080 px = 440 pt).
    let layout = our_layout();
    let rect = layout.frame_rect(AspectRatio::FourByThree);
    assert!((rect.min_y() - 186.0).abs() < 0.1, "верх {} pt", rect.min_y());
    assert!((rect.max_y() - 772.7).abs() < 0.1, "низ {} pt", rect.max_y());
    let (top_px, bottom_px) = frame_px(&layout, AspectRatio::FourByThree);
    assert!((top_px - 456.5).abs() < 1.0, "верх {top_px} px");
    assert!((bottom_px - 1896.6).abs() < 1.0, "низ {bottom_px} px");
    // Кадр 4:3 — ровно 4:3 при ширине 440 pt.
    assert!((rect.size.height / rect.size.width - 4.0 / 3.0).abs() < 1e-9);
}

#[test]
fn square_frame_is_440_pt_and_centered_on_4x3_frame() {
    let layout = our_layout();
    let square = layout.frame_rect(AspectRatio::Square);
    let four_by_three = layout.frame_rect(AspectRatio::FourByThree);
    assert!((square.size.width - 440.0).abs() < 1e-9);
    assert!((square.size.height - 440.0).abs() < 1e-9);
    assert!((square.center().y - four_by_three.center().y).abs() < 1e-9);
}

#[test]
fn layout_is_anchored_to_the_bottom_for_any_screen_height() {
    // Расстояние от низа кадра до низа экрана одно и то же на любой высоте (решение 82).
    let tall = ScreenLayout::new(
        &LayoutConfig::default(),
        SurfaceSize {
            width_px: 1080,
            height_px: 2412,
        },
    )
    .expect("tall");
    let shorter = ScreenLayout::new(
        &LayoutConfig::default(),
        SurfaceSize {
            width_px: 1080,
            height_px: 2340,
        },
    )
    .expect("short");
    for aspect in AspectRatio::ALL {
        let gap = |layout: &ScreenLayout| layout.screen_size().height - layout.frame_rect(aspect).max_y();
        assert!((gap(&tall) - gap(&shorter)).abs() < 1e-9, "{aspect:?}");
    }
    // На эталонном iPhone (440×956 pt) сдвига нет и кадр 16:9 стоит на измеренных 75,7–858,2.
    let iphone = ScreenLayout::new(
        &LayoutConfig::default(),
        SurfaceSize {
            width_px: 1320,
            height_px: 2868,
        },
    )
    .expect("iphone");
    let rect = iphone.frame_rect(AspectRatio::SixteenByNine);
    assert!(iphone.bottom_anchor_shift_pt().abs() < 1e-9);
    assert!((rect.min_y() - 75.7).abs() < 1e-9 && (rect.max_y() - 858.2).abs() < 1e-9);
}

#[test]
fn normalized_destination_is_the_frame_share_of_the_surface() {
    let layout = our_layout();
    let rect = layout.frame_rect(AspectRatio::FourByThree);
    let normalized = layout.frame_rect_normalized(AspectRatio::FourByThree);
    assert_eq!(normalized.left, 0.0);
    assert_eq!(normalized.width, 1.0);
    let expected_top = (rect.min_y() / layout.screen_size().height) as f32;
    assert!((normalized.top - expected_top).abs() < 1e-6);
    assert!((normalized.top + normalized.height) < 1.0);
}

#[test]
fn framing_intent_carries_sensor_crop_and_destination_without_zoom() {
    let layout = our_layout();
    for (aspect, crop) in [
        (AspectRatio::FourByThree, CropAspect::FOUR_BY_THREE),
        (AspectRatio::SixteenByNine, CropAspect::SIXTEEN_BY_NINE),
        (AspectRatio::Square, CropAspect::SQUARE),
    ] {
        let intent = crate::test_support::plain_framing(&layout, aspect);
        assert_eq!(intent.zoom.aspect, crop);
        assert_eq!(intent.zoom.ratio, camera_contract::zoom::ZoomRatio::ONE);
        assert_eq!(intent.destination, layout.frame_rect_normalized(aspect));
    }
}

#[test]
fn degenerate_surface_and_too_short_screen_are_errors() {
    let config = LayoutConfig::default();
    assert!(matches!(
        ScreenLayout::new(
            &config,
            SurfaceSize {
                width_px: 1080,
                height_px: 0
            }
        ),
        Err(LayoutError::EmptySurface { .. })
    ));
    assert!(matches!(
        ScreenLayout::new(
            &config,
            SurfaceSize {
                width_px: 0,
                height_px: 2412
            }
        ),
        Err(LayoutError::Scale(_))
    ));
    // Экран 1080×1000: 407 pt высотой — кадр 16:9 уехал бы выше верха экрана.
    assert!(matches!(
        ScreenLayout::new(
            &config,
            SurfaceSize {
                width_px: 1080,
                height_px: 1000
            }
        ),
        Err(LayoutError::FrameOutsideScreen { .. })
    ));
}

#[test]
fn invalid_layout_config_is_rejected() {
    let default = LayoutConfig::default();
    let flat_16x9 = LayoutConfig {
        frame_16x9_top_pt: default.frame_16x9_bottom_pt,
        ..default.clone()
    };
    assert!(flat_16x9.validate().is_err());
    let not_a_number = LayoutConfig {
        reference_height_pt: f64::NAN,
        ..default.clone()
    };
    assert!(not_a_number.validate().is_err());
    let below_screen = LayoutConfig {
        frame_4x3_bottom_pt: 2000.0,
        ..default.clone()
    };
    assert!(below_screen.validate().is_err());
    assert!(default.validate().is_ok());
}
