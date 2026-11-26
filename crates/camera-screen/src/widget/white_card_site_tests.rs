//! Тесты дома карточки белого: номера кнопок, состав ряда, оси пана и порядок приложения смещений.

use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use ui_core::geometry::Vector;
use ui_core::gesture::{PanDirection, PanTranslation};
use ui_draw::geometry::PxPoint;
use ui_draw::transform::Affine2;
use ui_kit::pan::PanUpdate;
use ui_kit::tokens::UiKitTokens;

use super::{WhiteCardButton, WhiteCardSite};
use crate::calibration::CalibrationConfig;
use crate::config::ScreenConfig;
use crate::layout::ScreenLayout;
use crate::mode_behaviour::LayoutPolicy;
use crate::test_support::OUR_SURFACE;
use crate::white::WhiteAxis;
use crate::white_row::{WhiteRowLayout, WhiteRowSlot};
use crate::widget::calibration::space::SectionSpace;
use crate::widget::geometry::ScreenGeometry;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widgets_config::WidgetLayoutConfig;

fn geometry() -> ScreenGeometry {
    let layout = ScreenLayout::new(&ScreenConfig::default().layout, OUR_SURFACE).expect("раскладка");
    ScreenGeometry::new(
        &layout,
        &WidgetLayoutConfig::default(),
        &crate::zoom_config::ZoomConfig::default(),
        &UiKitTokens::default(),
    )
}

/// Дом раздела на поверхности 440 × 983 pt в положении `orientation`.
fn section_site(orientation: DeviceOrientation) -> WhiteCardSite {
    let base = geometry();
    let layout = ScreenLayout::new(&ScreenConfig::default().layout, OUR_SURFACE)
        .expect("раскладка")
        .calibration_layout(
            &CalibrationConfig::default(),
            LayoutPolicy::FollowsOrientation,
            orientation,
            SafeAreaInsets::NONE,
        )
        .expect("раскладка раздела");
    let surface = base.screen();
    let space = SectionSpace::new(&layout, surface);
    WhiteCardSite::section(&base.in_screen(space.root_frame()), &space)
}

fn every_button() -> Vec<WhiteCardButton> {
    let mut buttons = vec![
        WhiteCardButton::Close,
        WhiteCardButton::Axis(WhiteAxis::Temperature),
        WhiteCardButton::Axis(WhiteAxis::Tint),
    ];
    buttons.extend(WhiteRowSlot::row(5).map(WhiteCardButton::Row));
    buttons
}

#[test]
fn photo_keeps_its_old_button_numbers_and_every_button_round_trips() {
    let site = WhiteCardSite::photo(&geometry());
    assert_eq!(site.button_id(WhiteCardButton::Close), Ok(ButtonId::WhiteClose));
    assert_eq!(
        site.button_id(WhiteCardButton::Axis(WhiteAxis::Tint)),
        Ok(ButtonId::WhiteAxis(WhiteAxis::Tint))
    );
    assert_eq!(
        site.button_id(WhiteCardButton::Row(WhiteRowSlot::Auto)),
        Ok(ButtonId::WhiteAuto)
    );
    assert_eq!(
        site.button_id(WhiteCardButton::Row(WhiteRowSlot::Light(3))),
        Ok(ButtonId::WhiteLight(3))
    );
    for button in every_button() {
        let id = site.button_id(button).expect("номер");
        assert_eq!(site.button_of(id), Some(button), "{button:?}");
    }
}

#[test]
fn the_section_numbers_are_its_own_and_never_collide_with_photo() {
    let photo = WhiteCardSite::photo(&geometry());
    let section = section_site(DeviceOrientation::Portrait);
    for button in every_button() {
        let section_id = section.button_id(button).expect("номер раздела");
        assert_eq!(section_id, ButtonId::SectionWhite(button));
        assert_eq!(
            photo.button_of(section_id),
            None,
            "{button:?}: карточка PHOTO чужой номер не берёт"
        );
        let photo_id = photo.button_id(button).expect("номер PHOTO");
        assert_eq!(
            section.button_of(photo_id),
            None,
            "{button:?}: раздел номер PHOTO не берёт"
        );
    }
    assert_ne!(photo.ruler_pan().1, section.ruler_pan().1);
    assert_eq!(section.ruler_pan().1, ScreenGesture::SectionWhiteRulerPan);
}

#[test]
fn only_the_photo_row_has_auto() {
    assert_eq!(WhiteCardSite::photo(&geometry()).row_layout(), WhiteRowLayout::WithAuto);
    assert_eq!(
        section_site(DeviceOrientation::Portrait).row_layout(),
        WhiteRowLayout::WithoutAuto
    );
}

#[test]
fn the_ruler_pans_along_the_eye_horizontal_in_every_orientation() {
    assert_eq!(
        WhiteCardSite::photo(&geometry()).ruler_pan().0,
        PanDirection::Horizontal
    );
    assert_eq!(
        section_site(DeviceOrientation::Portrait).ruler_pan().0,
        PanDirection::Horizontal
    );
    // В альбоме глазная горизонталь — вертикаль поверхности.
    assert_eq!(
        section_site(DeviceOrientation::LandscapeLeft).ruler_pan().0,
        PanDirection::Vertical
    );
    assert_eq!(
        section_site(DeviceOrientation::LandscapeRight).ruler_pan().0,
        PanDirection::Vertical
    );
}

#[test]
fn a_section_pan_is_turned_into_eye_axes_and_a_photo_pan_is_left_alone() {
    let along_surface_y = PanUpdate::Changed {
        translation: PanTranslation {
            from_touch_down: Vector::new(0.0, 30.0),
            from_recognition: Vector::new(0.0, 30.0),
        },
    };
    assert_eq!(
        WhiteCardSite::photo(&geometry()).scale_pan(along_surface_y),
        along_surface_y
    );
    let PanUpdate::Changed { translation } = section_site(DeviceOrientation::LandscapeLeft).scale_pan(along_surface_y)
    else {
        panic!("пан остался паном");
    };
    // Палец по вертикали поверхности в альбоме — это движение вдоль глазной горизонтали.
    assert!(translation.from_recognition.dx.abs() > 29.0, "{translation:?}");
    assert!(translation.from_recognition.dy.abs() < 1.0, "{translation:?}");
}

#[test]
fn photo_offsets_follow_the_placement_and_section_offsets_precede_it() {
    let placement = Affine2::rotate(std::f64::consts::FRAC_PI_2 as f32).then(Affine2::translate(100.0, 100.0));
    let offset = Affine2::translate(0.0, 10.0);
    let photo = WhiteCardSite::photo(&geometry()).offset_placed(placement, offset);
    let section = section_site(DeviceOrientation::Portrait).offset_placed(placement, offset);
    let origin = PxPoint::new(0.0, 0.0);
    let photo_at = photo.apply_point(origin);
    let section_at = section.apply_point(origin);
    // PHOTO: сдвиг после поворота — вдоль оси поверхности (вниз на 10); раздел: сдвиг в осях узла — после поворота
    // он уходит вбок.
    assert!(
        (photo_at.x - 100.0).abs() < 1e-3 && (photo_at.y - 110.0).abs() < 1e-3,
        "{photo_at:?}"
    );
    assert!(
        (section_at.x - 90.0).abs() < 1e-3 && (section_at.y - 100.0).abs() < 1e-3,
        "{section_at:?}"
    );
}
