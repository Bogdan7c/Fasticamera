//! Миниатюра в списке рисования (14g): настоящая картинка снимка внутри стеклянного круга, перекрёстное
//! растворение по времени кадров, поворот вместе со значками, серый диск без картинки.

use shell_contract::orientation::DeviceOrientation;
use ui_draw::draw_list::{DrawItem, DrawList, ImageClip, ImageFit, ImageItem};
use ui_draw::icons::IconId;
use ui_draw::picture::UiPicture;
use ui_draw::shape::ShapeGeometry;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::shutter::{CaptureOutcome, ShotRequestId};
use crate::test_support::test_picture;

/// Диаметр стеклянного круга миниатюры (Ø 48 pt) и доля круга под картинку (`bottom.thumbnail_dot_scale`).
const THUMBNAIL_DIAMETER_PX: f32 = 48.0 * PX_PER_PT;
const PICTURE_SCALE: f32 = 0.86;

fn images(list: &DrawList) -> Vec<&ImageItem> {
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Image(image) => Some(image),
            _ => None,
        })
        .collect()
}

/// Стеклянный круг миниатюры: центр (px) члена группы.
fn thumbnail_glass_center(list: &DrawList) -> (f32, f32) {
    glass_members(list)
        .into_iter()
        .find(|(geometry, _, y)| {
            near(*y, 2270.0, 3.0)
                && matches!(geometry, ShapeGeometry::Circle { diameter } if near(*diameter, THUMBNAIL_DIAMETER_PX, 2.0))
        })
        .map(|(_, x, y)| (x, y))
        .expect("стеклянный круг миниатюры на месте")
}

/// Снимок «нажат → захвачен»: затвор снова свободен, итог ждёт сохранения.
fn shoot(rig: &mut Rig, request: u64) {
    rig.act(ScreenAction::ShutterTapped { at: ms(rig.now_ms) });
    rig.screen.drain_effects();
    rig.screen.handle_action(ScreenAction::CaptureSecured {
        request: ShotRequestId(request),
    });
}

/// Снимок целиком: нажатие, захват и «сохранён» с картинкой `seed` (без кадров после «сохранён»).
fn shoot_and_save(rig: &mut Rig, request: u64, seed: u8) -> UiPicture {
    shoot(rig, request);
    let picture = test_picture(seed);
    rig.screen
        .handle_saved_with_picture(ShotRequestId(request), picture.clone());
    picture
}

#[test]
fn before_the_first_shot_the_thumbnail_circle_is_empty() {
    let mut rig = Rig::new();
    let list = rig.frame().clone();
    assert!(images(&list).is_empty());
    thumbnail_glass_center(&list);
}

#[test]
fn the_saved_picture_is_drawn_inside_the_glass_circle_at_86_percent_cover_clipped_to_a_circle() {
    let mut rig = Rig::new();
    let picture = shoot_and_save(&mut rig, 1, 10);
    // Первый кадр защёлкивает старт растворения, дальше картинка видна.
    rig.frame();
    rig.run(150);

    let list = rig.last.clone();
    let shown = images(&list);
    assert_eq!(shown.len(), 1);
    let image = shown[0];
    assert_eq!(
        image.source.as_picture(),
        Some(&picture),
        "рисуется картинка именно этого снимка (те же пиксели)"
    );
    assert_eq!((image.fit, image.clip), (ImageFit::Cover, ImageClip::Circle));
    let (glass_x, glass_y) = thumbnail_glass_center(&list);
    assert!(
        near(image.size_px.width, THUMBNAIL_DIAMETER_PX * PICTURE_SCALE, 1.5),
        "{:?}",
        image.size_px
    );
    assert!(near(image.size_px.height, image.size_px.width, 1e-3));
    assert!(near(image.transform.tx, glass_x, 0.5) && near(image.transform.ty, glass_y, 0.5));
    assert_eq!(image.opacity.get(), 1.0, "растворение закончилось");
}

#[test]
fn the_first_picture_fades_in_linearly_over_150_ms_starting_at_the_first_frame_that_draws_it() {
    let mut rig = Rig::new();
    rig.run(1_000);
    shoot_and_save(&mut rig, 1, 10);
    let start = rig.now_ms + 16;
    rig.now_ms = start;
    let first_frame = rig.frame().clone();
    assert!(
        images(&first_frame).is_empty(),
        "в момент старта картинка ещё прозрачна"
    );

    let mut opacity_at = |elapsed: i64| {
        rig.now_ms = start + elapsed;
        let list = rig.frame().clone();
        images(&list).first().map(|image| image.opacity.get())
    };
    let samples: Vec<Option<f32>> = [75, 150, 300].into_iter().map(&mut opacity_at).collect();

    assert!(
        matches!(samples[0], Some(value) if near(value, 0.5, 1e-3)),
        "середина 75 мс: {samples:?}"
    );
    assert_eq!(samples[1], Some(1.0), "конец 150 мс");
    assert_eq!(samples[2], Some(1.0));
}

#[test]
fn a_fade_started_after_a_long_gap_without_frames_still_runs_visibly_from_the_first_frame_back() {
    // Итог пришёл, пока экрана на поверхности не было: кадров нет долго (сворачивание, 14e).
    let mut rig = Rig::new();
    rig.run(1_000);
    shoot_and_save(&mut rig, 1, 10);
    let back = rig.now_ms + 10 * 60 * 1_000;
    rig.now_ms = back;
    assert!(
        images(rig.frame()).is_empty(),
        "первый кадр после возврата: растворение только началось"
    );

    rig.now_ms = back + 75;
    let list = rig.frame().clone();

    let shown = images(&list);
    assert_eq!(
        shown.len(),
        1,
        "через 75 мс после ВОЗВРАТА картинка уже проявляется (а не мгновенно полная)"
    );
    assert!(near(shown[0].opacity.get(), 0.5, 1e-3), "{}", shown[0].opacity.get());
}

#[test]
fn a_second_picture_dissolves_over_the_first_with_exactly_two_images_and_the_old_one_fully_opaque() {
    let mut rig = Rig::new();
    let first = shoot_and_save(&mut rig, 1, 10);
    rig.frame();
    rig.run(300);
    let second = shoot_and_save(&mut rig, 2, 20);
    rig.now_ms += 16;
    rig.frame();
    rig.now_ms += 75;

    let list = rig.frame().clone();

    let shown = images(&list);
    assert_eq!(shown.len(), 2, "старая и новая — не больше двух");
    assert_eq!(shown[0].source.as_picture(), Some(&first));
    assert_eq!(
        shown[0].opacity.get(),
        1.0,
        "старая полностью непрозрачна: стекло не просвечивает"
    );
    assert_eq!(shown[1].source.as_picture(), Some(&second));
    assert!(near(shown[1].opacity.get(), 0.5, 1e-3));
}

#[test]
fn a_new_saved_during_a_fade_keeps_two_images_with_the_fading_in_picture_underneath() {
    let mut rig = Rig::new();
    shoot_and_save(&mut rig, 1, 10);
    rig.frame();
    rig.run(300);
    let second = shoot_and_save(&mut rig, 2, 20);
    rig.now_ms += 16;
    rig.frame();
    rig.now_ms += 75;
    rig.frame();
    let third = shoot_and_save(&mut rig, 3, 30);
    rig.now_ms += 16;
    rig.frame();
    rig.now_ms += 75;

    let list = rig.frame().clone();

    let shown = images(&list);
    assert_eq!(shown.len(), 2);
    assert_eq!(shown[0].source.as_picture(), Some(&second), "внизу та, что проявлялась");
    assert_eq!(shown[1].source.as_picture(), Some(&third));
    assert!(near(shown[1].opacity.get(), 0.5, 1e-3), "растворение пошло заново");
}

#[test]
fn saved_without_a_picture_draws_the_grey_disc_and_no_image() {
    let mut rig = Rig::new();
    shoot(&mut rig, 1);
    rig.screen.handle_action(ScreenAction::CaptureFinished {
        request: ShotRequestId(1),
        outcome: CaptureOutcome::Saved,
    });
    rig.frame();
    let list = rig.run(200).clone();

    assert!(images(&list).is_empty());
    let (glass_x, glass_y) = thumbnail_glass_center(&list);
    let disc_diameter = THUMBNAIL_DIAMETER_PX * PICTURE_SCALE;
    let disc = list.leaf_items().any(|item| match item {
        DrawItem::Shape(shape) => {
            matches!(shape.geometry, ShapeGeometry::Circle { diameter } if near(diameter, disc_diameter, 1.5))
                && near(shape.transform.tx, glass_x, 0.5)
                && near(shape.transform.ty, glass_y, 0.5)
        }
        _ => false,
    });
    assert!(disc, "серый диск 86 % внутри круга");
}

#[test]
fn the_picture_turns_with_the_icons_when_the_phone_is_held_sideways_and_stays_put_in_portrait() {
    let mut rig = Rig::new();
    shoot_and_save(&mut rig, 1, 10);
    rig.frame();
    rig.run(300);
    let portrait = images(&rig.last)[0].clone();
    assert!(
        near(portrait.transform.a, 1.0, 1e-3) && near(portrait.transform.b, 0.0, 1e-3),
        "{:?}",
        portrait.transform
    );

    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(1_000);

    let list = rig.last.clone();
    let landscape = images(&list)[0].transform;
    let icon = icon_transforms(&list, IconId::FlashOff)[0];
    assert!(
        near(landscape.a, 0.0, 1e-3) && near(landscape.c, 1.0, 1e-3),
        "{landscape:?}"
    );
    // Тот же угол, что у значков (решение 165), центр остался в круге.
    assert!(near(landscape.a, icon.a, 1e-3) && near(landscape.b, icon.b, 1e-3));
    assert!(near(landscape.c, icon.c, 1e-3) && near(landscape.d, icon.d, 1e-3));
    assert!(near(landscape.tx, portrait.transform.tx, 0.5) && near(landscape.ty, portrait.transform.ty, 0.5));
}

#[test]
fn the_picture_dissolves_together_with_the_circle_while_a_card_is_open_and_returns_after() {
    let mut rig = Rig::new();
    shoot_and_save(&mut rig, 1, 10);
    rig.frame();
    rig.run(300);
    assert_eq!(images(&rig.last).len(), 1);

    rig.act(ScreenAction::StripToolTapped(crate::tools::ToolId::Exposure));
    rig.run(700);
    let hidden = images(&rig.last)
        .iter()
        .map(|image| image.opacity.get())
        .fold(0.0_f32, f32::max);
    assert!(hidden < 0.05, "картинка ушла вместе с кругом: {hidden}");

    rig.act(ScreenAction::ExposurePanelCloseRequested);
    rig.run(700);
    assert_eq!(images(&rig.last).first().map(|image| image.opacity.get()), Some(1.0));
}

#[test]
fn the_draw_list_with_a_picture_passes_validation_like_the_renderer_would() {
    let mut rig = Rig::new();
    shoot_and_save(&mut rig, 1, 10);
    rig.frame();
    rig.run(50);
    rig.last.validate().expect("словарь с картинкой проходит проверку");
    assert_eq!(rig.screen.diagnostics().frame_failures, 0);
}
