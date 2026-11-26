//! Раздел калибровки без стекла (14m, решения 263, 269, 270): панели закрашены тёмной полупрозрачной поверхностью,
//! а не стеклом. Проверяется то, что видит рендер: в кадрах раздела нет групп стекла (поэтому пирамида размытия
//! не строится), поверхность появляется нарастанием плотности, нажатие масштабирует, но не светит, а при возврате
//! в PHOTO стекло на месте.

use look_contract::{LookFacts, LookProblem, LookProblemKind, PresetError};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_draw::draw_list::{DrawItem, DrawList, ShapeItem};
use ui_kit::tokens::UiKitTokens;

use super::screen_calibration_tests::{calibrating, center};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::calibration::ControlViewKind;
use crate::look_test_support::look_facts;
use crate::modes::CaptureMode;

const ORIENTATIONS: [DeviceOrientation; 3] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
];

/// Фигуры-поверхности кадра: закрашены цветом поверхности из токенов (так панели раздела отличаются от прочих фигур).
fn surfaces(list: &DrawList) -> Vec<&ShapeItem> {
    let color = UiKitTokens::default().surface.fill_color;
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Shape(shape) if (shape.fill.r, shape.fill.g, shape.fill.b) == (color.r, color.g, color.b) => {
                Some(shape)
            }
            _ => None,
        })
        .collect()
}

/// Группы стекла кадра (включая вложенные в обрезку). В кадре раздела их нет ни одной: панель прибора тоже
/// поверхность (подложка, решения 257, 263).
fn glass_groups(list: &DrawList) -> usize {
    list.leaf_items()
        .filter(|item| matches!(item, DrawItem::Glass(_)))
        .count()
}

fn open_presets_card(rig: &mut Rig) {
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(900);
}

#[test]
fn no_glass_in_any_section_frame_state_and_orientation() {
    for orientation in ORIENTATIONS {
        for view in [ControlViewKind::Scales, ControlViewKind::Wheels] {
            let mut rig = calibrating(look_facts(), orientation);
            rig.act(ScreenAction::ControlViewSelected(view));
            // Покой, затем карточка пресетов (в том числе кадры раскрытия).
            let checked_frames = std::cell::Cell::new(0);
            let check = |rig: &mut Rig, context: &str| {
                let list = rig.frame().clone();
                let stray = glass_groups(&list);
                assert_eq!(stray, 0, "{orientation:?} {view:?} {context}: в кадре раздела стекло");
                checked_frames.set(checked_frames.get() + 1);
            };
            rig.run(400);
            check(&mut rig, "покой");
            rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
            for _ in 0..50 {
                rig.run(16);
                check(&mut rig, "карточка пресетов раскрывается");
            }
            for _ in 0..50 {
                rig.run(16);
                check(&mut rig, "карточка пресетов открыта");
            }
            assert!(checked_frames.get() >= 100, "проверены кадры анимации, а не один покой");
            // Закрытое меню и плашка сбоя — тоже без стекла.
            let problem = LookFacts {
                problem: Some(LookProblem {
                    serial: 1,
                    kind: LookProblemKind::Rejected(PresetError::NameTaken),
                }),
                ..look_facts()
            };
            rig.screen
                .handle_action(ScreenAction::LookFactsChanged(Box::new(problem)));
            for _ in 0..30 {
                rig.run(16);
                check(&mut rig, "плашка сбоя");
            }
        }
    }
}

#[test]
fn no_glass_while_the_phone_turns_inside_the_section() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let mut frames = 0;
    for target in [
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::LandscapeRight,
        DeviceOrientation::Portrait,
    ] {
        rig.screen.handle_action(ScreenAction::DeviceOrientationChanged(target));
        for _ in 0..60 {
            rig.run(16);
            let list = rig.last.clone();
            assert_eq!(glass_groups(&list), 0, "поворот к {target:?}: в кадре раздела стекло");
            frames += 1;
        }
    }
    assert_eq!(frames, 180);
}

/// Известное ограничение (14m, сообщено главному агенту): завеса перехода между камерами (`transition_veil.rs`)
/// рисует стекло роли Frost поверх КАДРА, и в разделе калибровки тоже, когда рендер удерживает последний кадр
/// (`BackdropContinuity::HeldLastFrame`: смена объектива, перезапуск камеры). Завесу этот пакет не менял — на таких
/// кадрах раздела пирамида размытия строится. Тест фиксирует факт, чтобы изменение завесы было осознанным решением.
#[test]
fn the_held_frame_veil_still_draws_frost_glass_in_the_section() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    rig.run(100);
    let calm = rig.frame().clone();
    assert_eq!(glass_groups(&calm), 0, "в покое стекла нет");
    rig.backdrop = ui_draw::frame_source::BackdropContinuity::HeldLastFrame { since: ms(rig.now_ms) };
    rig.run(32);
    let held = rig.last.clone();
    let frost_blur_px = UiKitTokens::default().glass.frost.blur_radius_pt * PX_PER_PT;
    let frost_groups = held
        .leaf_items()
        .filter(|item| matches!(item, DrawItem::Glass(group) if near(group.params.blur_radius_px, frost_blur_px, 0.5)))
        .count();
    assert_eq!(frost_groups, 1, "на удержанном кадре завеса рисует одно стекло Frost");
}

#[test]
fn photo_keeps_its_glass_and_the_section_returns_to_surface_without_a_mixed_frame() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    assert!(!surfaces(rig.frame()).is_empty(), "в разделе панели — поверхности");
    let photo_glass = |list: &DrawList| {
        list.leaf_items()
            .filter(|item| matches!(item, DrawItem::Glass(_)))
            .count()
    };
    rig.act(ScreenAction::ModeSelected(CaptureMode::Photo));
    // Каждый кадр перехода: либо раздел (поверхности, без стекла вне прибора), либо PHOTO (стекло, без поверхностей) —
    // смешанного кадра нет (решение 269: материал меняется в том же кадре, что и режим).
    let mut photo_frames = 0;
    for _ in 0..120 {
        rig.run(16);
        let list = rig.last.clone();
        let (glass, surface) = (photo_glass(&list), surfaces(&list).len());
        let in_photo = glass_groups(&list) > 0;
        assert!(
            !(in_photo && surface > 0),
            "смешанный кадр: стекло PHOTO ({glass}) вместе с поверхностями раздела ({surface})"
        );
        if in_photo {
            photo_frames += 1;
        }
    }
    assert!(photo_frames > 0, "после возврата в PHOTO стекло снова рисуется");
    let list = rig.last.clone();
    assert!(photo_glass(&list) >= 4, "в PHOTO стекло затвора, кнопок и капсулы");
    assert!(surfaces(&list).is_empty(), "в PHOTO поверхностей раздела нет");
}

#[test]
fn knobs_column_and_capsule_are_surfaces_with_configured_density() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let list = rig.frame().clone();
    let tokens = UiKitTokens::default();
    let found = surfaces(&list);
    // Колонка ручек, капсула режимов и кнопка меню — поверхности плотности панели; панель прибора — подложка.
    let (backings, panels): (Vec<&ShapeItem>, Vec<&ShapeItem>) = found
        .iter()
        .partition(|surface| surface.fill.a == tokens.surface.backing_density);
    assert!(panels.len() >= 3, "колонка, капсула и кнопка меню: {}", panels.len());
    assert_eq!(backings.len(), 1, "подложка под следом прибора — ровно одна");
    for surface in &panels {
        assert_eq!(
            surface.fill.a, tokens.surface.panel_density,
            "плотность панели из конфига"
        );
    }
    for surface in &found {
        assert!(surface.shadow.is_none(), "тени у поверхности нет");
    }
}

#[test]
fn the_sheet_surface_grows_its_density_with_the_old_morph_progress() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let tokens = UiKitTokens::default();
    let before: Vec<_> = surfaces(rig.frame())
        .iter()
        .map(|s| (s.geometry, s.transform))
        .collect();
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    let mut alphas = Vec::new();
    for _ in 0..60 {
        rig.run(16);
        let list = rig.last.clone();
        // Лист — фигура, которой не было в покое.
        if let Some(sheet) = surfaces(&list)
            .into_iter()
            .find(|s| !before.contains(&(s.geometry, s.transform)))
        {
            alphas.push(sheet.fill.a);
        }
    }
    assert!(
        alphas.len() > 10,
        "лист меню нарисован в кадрах раскрытия: {}",
        alphas.len()
    );
    assert!(
        alphas.windows(2).all(|pair| pair[1] >= pair[0] - 1e-6),
        "плотность листа растёт вместе с морфом: {alphas:?}"
    );
    assert!(
        alphas[0] < tokens.surface.panel_density * 0.9,
        "лист появляется с малой плотности: {}",
        alphas[0]
    );
    assert!(
        near(*alphas.last().unwrap(), tokens.surface.panel_density, 1e-4),
        "и доходит до плотности панели: {alphas:?}"
    );
}

#[test]
fn pressing_the_menu_button_scales_the_surface_without_a_glow() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let button = center(&rig, "menu_button", 0);
    let size_of = |list: &DrawList| {
        surfaces(list)
            .into_iter()
            .find(|s| matches!(s.geometry, ui_draw::shape::ShapeGeometry::Circle { .. }))
            .map(|s| (s.transform.a.hypot(s.transform.c), s.fill, s.shadow.is_some()))
            .expect("кнопка меню — круг-поверхность")
    };
    let (rest_scale, rest_fill, _) = size_of(rig.frame());
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, button, start);
    // Держим палец: нажатие нарастает пружиной и оседает на полном значении.
    rig.now_ms = start;
    rig.run(500);
    let (pressed_scale, pressed_fill, has_shadow) = size_of(&rig.last.clone());
    let gain = UiKitTokens::default().glass.button.press.scale_gain;
    assert!(
        near(pressed_scale, rest_scale * (1.0 + gain), 0.01),
        "нажатая кнопка крупнее ровно на прирост стеклянной кнопки: {pressed_scale} против {rest_scale} × (1 + {gain})"
    );
    assert_eq!(pressed_fill, rest_fill, "свечения нет: заливка при нажатии та же");
    assert!(!has_shadow, "тени и свечения нет");
    rig.touch(TouchPhase::Cancelled, button, rig.now_ms);
}

#[test]
fn presets_card_is_a_surface_too_and_never_glass_and_hides_the_bottom_row_under_it() {
    let mut rig = calibrating(look_facts(), DeviceOrientation::Portrait);
    let is_circle = |shape: &ShapeItem| matches!(shape.geometry, ui_draw::shape::ShapeGeometry::Circle { .. });
    let is_card = |shape: &ShapeItem| matches!(shape.geometry, ui_draw::shape::ShapeGeometry::RoundedRect { .. });
    let before = rig.frame().clone();
    assert!(
        surfaces(&before).into_iter().any(is_circle),
        "кнопка меню — круг-поверхность"
    );
    let panels_at_rest = surfaces(&before).into_iter().filter(|shape| is_card(shape)).count();
    open_presets_card(&mut rig);
    let list = rig.frame().clone();
    assert_eq!(
        surfaces(&list).into_iter().filter(|shape| is_card(shape)).count(),
        panels_at_rest + 1,
        "карточка добавила свою поверхность"
    );
    // Решение 292: нижний ряд (кнопка меню — круг, CLEAN, капсула режимов) под карточкой не просвечивает.
    assert_eq!(
        surfaces(&list).into_iter().filter(|shape| is_circle(shape)).count(),
        0,
        "кнопки нижнего ряда не рисуются под карточкой"
    );
    assert_eq!(glass_groups(&list), 0);
}
