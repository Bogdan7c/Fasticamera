//! Экран PHOTO без стекла (14p, решения 297–300): панели закрашены тёмной полупрозрачной поверхностью с тонким
//! светлым ободком, у затвора нет подложки. Проверяется то, что видит рендер:
//! * в кадрах PHOTO нет ни одной группы стекла (значит, пирамида размытия не строится) — в покое, со шторкой, с
//!   каждой карточкой, с лентой и колесом зума, во всех положениях значков; исключение — завеса перехода камер,
//!   и только пока рендер держит кадр (решение 299);
//! * все поверхности PHOTO — с ободком, плотность и ободок растут вместе с прежним морфом (решение 270);
//! * под шторкой нижний ряд растворяется и не ловит касаний (раньше его закрывало стекло шторки).

use camera_contract::ids::LensRole;
use shell_contract::orientation::DeviceOrientation;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::{DrawItem, DrawList, ShapeItem};
use ui_draw::frame_source::BackdropContinuity;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::UiKitTokens;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::look_test_support::look_facts;
use crate::modes::CaptureMode;
use crate::state::PanelState;
use crate::tools::ToolId;

const ORIENTATIONS: [DeviceOrientation; 4] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
    DeviceOrientation::PortraitUpsideDown,
];

/// Группы стекла кадра (включая вложенные в обрезку).
fn glass_groups(list: &DrawList) -> usize {
    list.leaf_items()
        .filter(|item| matches!(item, DrawItem::Glass(_)))
        .count()
}

/// Проверка кадра PHOTO: стекла нет, а каждая поверхность — поверхность PHOTO с ободком (не раздела).
fn assert_photo_frame_has_no_glass(list: &DrawList, context: &str) {
    assert_eq!(glass_groups(list), 0, "{context}: в кадре PHOTO нет стекла");
    let surfaces = surface_shapes(list);
    assert!(!surfaces.is_empty(), "{context}: панели PHOTO нарисованы");
    assert!(
        surfaces.iter().all(|shape| has_photo_edge(shape)),
        "{context}: каждая поверхность PHOTO — с ободком"
    );
}

/// Проверяет кадр в каждом из `frames` кадров по 16 мс (раскрытие, покой) и возвращает число проверенных.
fn check_frames(rig: &mut Rig, frames: usize, context: &str) -> usize {
    for frame in 0..frames {
        rig.run(16);
        let list = rig.last.clone();
        assert_photo_frame_has_no_glass(&list, &format!("{context}, кадр {frame}"));
    }
    frames
}

/// Экран PHOTO с фактами вида (нужны для пункта STYLES) в положении `orientation`.
fn photo_rig(orientation: DeviceOrientation) -> Rig {
    let mut rig = Rig::with_zoom(LensRole::Wide, MotionPreference::Full);
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(look_facts())));
    rig.screen
        .handle_action(ScreenAction::DeviceOrientationChanged(orientation));
    rig.run(600);
    rig
}

#[test]
fn no_glass_in_photo_at_rest_with_the_sheet_with_every_card_and_with_zoom_in_every_orientation() {
    for orientation in ORIENTATIONS {
        let mut rig = photo_rig(orientation);
        let mut checked = check_frames(&mut rig, 2, &format!("{orientation:?}, покой"));

        // Шторка Controls: кадры раскрытия и покой.
        rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
        checked += check_frames(&mut rig, 40, &format!("{orientation:?}, шторка"));

        // Каждая карточка инструмента из меню: кадры морфа шторка → карточка и покой.
        for tool in [ToolId::Exposure, ToolId::Timer, ToolId::Aspect, ToolId::Styles] {
            assert!(
                rig.screen.state().menu_tools().contains(&tool),
                "{orientation:?}: пункт {tool:?} есть в меню"
            );
            rig.act(ScreenAction::ControlsItemSelected(tool));
            let expected = match tool {
                ToolId::Exposure => PanelState::ExposurePanel,
                other => PanelState::Submenu(other),
            };
            assert_eq!(
                rig.screen.state().panel(),
                expected,
                "{orientation:?}: открыта карточка {tool:?}"
            );
            checked += check_frames(&mut rig, 40, &format!("{orientation:?}, карточка {tool:?}"));
            rig.act(ScreenAction::SubmenuBackRequested);
            checked += check_frames(&mut rig, 40, &format!("{orientation:?}, назад из {tool:?}"));
        }
        rig.act(ScreenAction::ControlsMenuCloseRequested);
        checked += check_frames(&mut rig, 40, &format!("{orientation:?}, шторка закрывается"));

        // Карточка белого: у неё нет пункта меню, её открывает кнопка у затвора.
        rig.act(ScreenAction::WhiteButtonTapped);
        assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::White));
        checked += check_frames(&mut rig, 40, &format!("{orientation:?}, карточка белого"));
        // Левая кнопка «×» карточки, открытой не из меню, закрывает её.
        rig.act(ScreenAction::SubmenuBackRequested);
        checked += check_frames(&mut rig, 40, &format!("{orientation:?}, белое закрывается"));

        // Лента зума: смена объектива тапом по ступени.
        rig.act(ScreenAction::ZoomStopTapped {
            stop: 0,
            at: ms(rig.now_ms),
        });
        checked += check_frames(&mut rig, 30, &format!("{orientation:?}, лента зума"));
        assert!(checked > 400, "{orientation:?}: проверено кадров {checked}");
    }
}

#[test]
fn the_held_frame_veil_is_the_only_glass_and_only_while_the_frame_is_held() {
    let mut rig = photo_rig(DeviceOrientation::Portrait);
    assert_eq!(glass_groups(rig.frame()), 0, "в покое стекла нет");
    let frost_blur_px = UiKitTokens::default().glass.frost.blur_radius_pt * PX_PER_PT;
    let frost_only = |list: &DrawList| {
        list.leaf_items()
            .filter(
                |item| matches!(item, DrawItem::Glass(group) if near(group.params.blur_radius_px, frost_blur_px, 0.5)),
            )
            .count()
    };
    // Переворот камеры: рендер держит кадр — завеса рисует одно стекло Frost, остальное — поверхности.
    rig.backdrop = BackdropContinuity::HeldLastFrame { since: ms(rig.now_ms) };
    rig.act(ScreenAction::FlipCameraRequested { at: ms(rig.now_ms) });
    rig.run(64);
    let held = rig.last.clone();
    assert_eq!(glass_groups(&held), 1, "на удержанном кадре стекло одно");
    assert_eq!(frost_only(&held), 1, "и это завеса Frost (решение 299)");
    // Кадр вернулся: завеса растворяется и уходит, стекла в PHOTO больше нет.
    rig.backdrop = BackdropContinuity::Live;
    rig.run(1500);
    assert_photo_frame_has_no_glass(&rig.last.clone(), "после переворота");
}

/// Поверхность-лист шторки: широкий скруглённый прямоугольник PHOTO.
fn sheet_surface(list: &DrawList) -> Option<&ShapeItem> {
    surface_shapes(list).into_iter().find(|shape| {
        has_photo_edge(shape)
            && matches!(silhouette(shape), ShapeGeometry::RoundedRect { size, .. } if size.width > px(400.0))
    })
}

#[test]
fn the_sheet_grows_its_density_and_its_edge_together_with_the_old_morph() {
    let mut rig = photo_rig(DeviceOrientation::Portrait);
    let tokens = UiKitTokens::default();
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    let mut samples = Vec::new();
    for _ in 0..60 {
        rig.run(16);
        if let Some(sheet) = sheet_surface(&rig.last) {
            let edge = sheet.stroke.expect("у шторки PHOTO есть ободок");
            samples.push((sheet.fill.a, edge.color.a));
        }
    }
    assert!(
        samples.len() > 10,
        "шторка нарисована в кадрах раскрытия: {}",
        samples.len()
    );
    assert!(
        samples
            .windows(2)
            .all(|pair| pair[1].0 >= pair[0].0 - 1e-6 && pair[1].1 >= pair[0].1 - 1e-6),
        "плотность и ободок растут вместе с морфом: {samples:?}"
    );
    let (first_fill, first_edge) = samples[0];
    assert!(
        first_fill < tokens.surface.panel_density * 0.9 && first_edge < tokens.surface.edge.color.a * 0.9,
        "шторка появляется с малой плотности: {:?}",
        samples[0]
    );
    // Ободок идёт тем же прогрессом, что заливка: отношение альф постоянно (решение 270).
    let ratio = tokens.surface.edge.color.a / tokens.surface.panel_density;
    assert!(
        samples.iter().all(|(fill, edge)| near(*edge, fill * ratio, 1e-4)),
        "ободок нарастает одним прогрессом с заливкой: {samples:?}"
    );
    let (last_fill, last_edge) = *samples.last().unwrap();
    assert!(
        near(last_fill, tokens.surface.panel_density, 1e-4),
        "плотность доходит до 70 %: {last_fill}"
    );
    assert!(
        near(last_edge, tokens.surface.edge.color.a, 1e-4),
        "ободок доходит до токена: {last_edge}"
    );
}

/// Точка на подписи CALIBRATE в капсуле PHOTO: она стоит справа от выбранной PHOTO (≈ 80 pt от центра капсулы).
fn calibrate_label_point(rig: &Rig) -> ui_core::geometry::Point {
    let capsule = rig.center_with(|g| g.capsule());
    ui_core::geometry::Point::new(capsule.x + 80.0, capsule.y)
}

#[test]
fn under_the_open_sheet_the_bottom_row_dissolves_and_takes_no_touches() {
    // Контроль: без шторки тап по подписи CALIBRATE переключает режим — значит, точка живая.
    let mut control = photo_rig(DeviceOrientation::Portrait);
    let calibrate_label = calibrate_label_point(&control);
    control.tap(calibrate_label);
    control.run(600);
    assert_eq!(
        control.screen.state().mode(),
        CaptureMode::Calibrate,
        "контроль: тап по CALIBRATE работает"
    );

    let mut rig = photo_rig(DeviceOrientation::Portrait);
    let thumbnail = rig.center_with(|g| g.thumbnail());
    let capsule_calibrate = rig.center_with(|g| g.capsule());
    let visible_at = |list: &DrawList, point: ui_core::geometry::Point| {
        surface_shapes(list).into_iter().any(|shape| {
            shape.opacity.get() > 0.01
                && matches!(
                    shape.geometry,
                    ShapeGeometry::Circle { .. } | ShapeGeometry::Capsule { .. }
                )
                && near(shape.transform.tx, px(point.x), 3.0)
                && near(shape.transform.ty, px(point.y), 3.0)
        })
    };
    assert!(visible_at(&rig.last.clone(), thumbnail), "миниатюра видна без шторки");
    assert!(
        visible_at(&rig.last.clone(), capsule_calibrate),
        "капсула видна без шторки"
    );

    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(900);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    let list = rig.last.clone();
    assert!(!visible_at(&list, thumbnail), "миниатюра растворилась под шторкой");
    assert!(
        !visible_at(&list, capsule_calibrate),
        "капсула растворилась под шторкой"
    );
    // Тот же тап по подписи CALIBRATE под шторкой режим не меняет (ряд под шторкой не нажимается).
    rig.tap(calibrate_label);
    rig.run(600);
    assert_eq!(
        rig.screen.state().mode(),
        CaptureMode::Photo,
        "режим под шторкой не сменился"
    );

    rig.act(ScreenAction::ControlsMenuCloseRequested);
    rig.run(1000);
    let list = rig.last.clone();
    assert!(visible_at(&list, thumbnail), "миниатюра вернулась");
    assert!(visible_at(&list, capsule_calibrate), "капсула вернулась");
}

/// Прозрачность круга-поверхности миниатюры PHOTO в кадре (0 — её нет в кадре).
fn thumbnail_opacity(rig: &Rig) -> f32 {
    let thumbnail = rig.center_with(|g| g.thumbnail());
    surface_shapes(&rig.last)
        .into_iter()
        .filter(|shape| {
            matches!(shape.geometry, ShapeGeometry::Circle { .. })
                && near(shape.transform.tx, px(thumbnail.x), 3.0)
                && near(shape.transform.ty, px(thumbnail.y), 3.0)
        })
        .map(|shape| shape.opacity.get())
        .fold(0.0, f32::max)
}

/// Кадры возврата в PHOTO: прозрачность миниатюры в каждом из `frames` кадров по 16 мс.
fn thumbnail_track_after_switch_to_photo(rig: &mut Rig, frames: usize) -> Vec<f32> {
    rig.act(ScreenAction::ModeSelected(CaptureMode::Photo));
    (0..frames)
        .map(|_| {
            rig.run(16);
            thumbnail_opacity(rig)
        })
        .collect()
}

#[test]
fn the_section_presets_card_does_not_hide_the_photo_bottom_row_and_the_way_back_is_unchanged() {
    // Под шторкой PHOTO нижний ряд растворяется (14p), но меню раздела — это карточка пресетов, а не шторка
    // Controls: признак скрытия ряда в разделе не включается, и возврат в PHOTO идёт так же, как без карточки.
    let mut control = super::screen_calibration_tests::calibrating(look_facts(), DeviceOrientation::Portrait);
    let expected = thumbnail_track_after_switch_to_photo(&mut control, 30);

    let mut rig = super::screen_calibration_tests::calibrating(look_facts(), DeviceOrientation::Portrait);
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(900);
    assert!(
        !rig.screen.state().is_bottom_row_hidden(),
        "карточка пресетов раздела не прячет нижний ряд PHOTO"
    );
    rig.act(ScreenAction::ControlsMenuCloseRequested);
    rig.run(900);
    let after_card = thumbnail_track_after_switch_to_photo(&mut rig, 30);
    assert!(
        expected.iter().zip(&after_card).all(|(a, b)| near(*a, *b, 1e-4)),
        "миниатюра возвращается так же, как без карточки: {after_card:?} против {expected:?}"
    );
    assert!(near(*after_card.last().unwrap(), 1.0, 1e-3), "и в конце видна целиком");
}
