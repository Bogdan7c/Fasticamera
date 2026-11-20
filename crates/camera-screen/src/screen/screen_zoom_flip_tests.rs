//! Кнопка переворота и завеса перехода на геометрии золотых снимков (1080×2412 px): кнопка без камеры
//! другого направления, повторный переворот после неудачи, покрытие кадра завесой до последнего пикселя.

use camera_contract::ids::LensRole;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::frame_source::BackdropContinuity;
use ui_draw::icons::IconId;

use super::test_rig::{PX_PER_PT, Rig, icons_of, ms};
use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::test_support::{zoom_catalog, zoom_facts};

/// Экран, где в каталоге есть только задние камеры (устройство без фронтальной).
fn back_only_rig() -> Rig {
    let mut rig = Rig::with(crate::ScreenConfig::default(), MotionPreference::Full);
    let mut facts = zoom_facts(LensRole::Wide);
    let mut catalog = (*zoom_catalog()).clone();
    catalog.front = None;
    facts.lens.catalog = Some(std::sync::Arc::new(catalog));
    rig.screen.handle_action(ScreenAction::CameraFactsChanged(facts));
    rig.run(500);
    rig.effects();
    rig
}

#[test]
fn the_flip_button_is_drawn_and_tappable_when_the_other_direction_exists() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    assert_eq!(icons_of(rig.run(500), IconId::FlipCamera).len(), 1);
    let center = rig.center_with(|geometry| geometry.flip_button());
    rig.tap(center);
    assert!(
        rig.effects().iter().any(|effect| matches!(effect, Effect::Lens(_))),
        "тап по кнопке публикует намерение"
    );
}

#[test]
fn without_a_front_camera_the_flip_button_has_no_pixels_and_no_hit_area() {
    let mut rig = back_only_rig();
    assert!(
        icons_of(rig.run(500), IconId::FlipCamera).is_empty(),
        "кнопка не рисуется"
    );

    let center = rig.center_with(|geometry| geometry.flip_button());
    rig.tap(center);
    let effects = rig.effects();
    assert!(
        !effects.iter().any(|effect| matches!(effect, Effect::Lens(_))),
        "тап в месте кнопки ничего не публикует: {effects:?}"
    );
    assert!(rig.screen.state().zoom().last_flip().is_none());
}

/// Окно размытия первой группы стекла: границы в px (левый, верхний, правый, нижний).
fn frost_bounds_px(list: &DrawList) -> (f64, f64, f64, f64) {
    let bounds = list
        .items
        .iter()
        .find_map(|item| match item {
            DrawItem::Glass(group) => group.members.first().and_then(|member| member.shape_bounds_px()),
            _ => None,
        })
        .expect("окно размытия в списке");
    let (x, y) = (f64::from(bounds.origin.x), f64::from(bounds.origin.y));
    (
        x,
        y,
        x + f64::from(bounds.size.width),
        y + f64::from(bounds.size.height),
    )
}

fn same(actual: f64, expected: f64) -> bool {
    (actual - expected).abs() < 1.0e-3
}

#[test]
fn the_veil_covers_the_frame_to_the_last_pixel_on_the_golden_geometry() {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.backdrop = BackdropContinuity::HeldLastFrame { since: ms(0) };
    let list = rig.run(64).clone();
    let frame = rig.rect_with(|geometry| geometry.frame(rig.screen.state().settings().aspect));
    let scale = f64::from(PX_PER_PT);
    let (frame_left, frame_top) = (frame.min_x() * scale, frame.min_y() * scale);
    let (frame_right, frame_bottom) = (frame.max_x() * scale, frame.max_y() * scale);

    let (left, top, right, bottom) = frost_bounds_px(&list);
    // Низ кадра лежит посреди пикселя (≈ 1896,6): раньше окно кончалось там же и оставляло строку неразмытой.
    assert!(
        frame_bottom.fract() > 0.05,
        "геометрия золота: край кадра не на границе px"
    );
    assert!(
        same(bottom, (frame_bottom - 1.0e-3).ceil()),
        "низ окна {bottom}, низ кадра {frame_bottom}"
    );
    assert!(
        same(top, (frame_top + 1.0e-3).floor()),
        "верх окна {top}, верх кадра {frame_top}"
    );
    assert!(
        same(left, (frame_left + 1.0e-3).floor()) && same(right, (frame_right - 1.0e-3).ceil()),
        "{left} {right} / {frame_left} {frame_right}"
    );
    // Не меньше кадра ни с одной стороны и избыток меньше пикселя.
    assert!(bottom >= frame_bottom && top <= frame_top && right >= frame_right - 1.0e-3 && left <= frame_left + 1.0e-3);
    assert!(bottom - frame_bottom < 1.0 && frame_top - top < 1.0);
}
