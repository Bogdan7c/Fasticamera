//! Раздел калибровки: геометрия, нижний ряд и поворот. Чистая зона превью пуста, капсула режимов остаётся измеренной (соседняя
//! подпись гаснет у края), а в альбоме весь раздел повёрнут вместе с телефоном и понимает жесты по глазным осям.
//! Общие помощники и тесты ручек, «чистяка», полос и приборов — в .

use look_contract::{BuiltinPreset, EditState, GlobalKnob, LookFacts, LookRecipe, PresetId, ToneBaseModel};
use scope_contract::ScopeKind;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_draw::draw_list::DrawItem;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{
    calibrating, center, drag_eye, edited, portrait, scope_images, set_looks, surface_vector, zone_px,
};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::look_test_support::{knob, look_facts};
use crate::modes::CaptureMode;

#[test]
fn nothing_is_drawn_over_the_clean_preview_zone() {
    for orientation in [
        DeviceOrientation::Portrait,
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::LandscapeRight,
    ] {
        let mut rig = calibrating(look_facts(), orientation);
        let list = rig.frame().clone();
        let layout = *rig.screen.state().calibration().layout().unwrap();
        let (left, top, right, bottom) = zone_px(&rig, layout.zones().clean_preview);
        // Охватывающий прямоугольник элемента чуть шире его самого (сглаживание краёв): касание границы зоны — не
        // перекрытие, поэтому считается только заход глубже `EDGE_SLACK_PX`.
        const EDGE_SLACK_PX: f32 = 3.0;
        let inside = |bounds: ui_draw::geometry::PxRect| {
            bounds.origin.x < right - EDGE_SLACK_PX
                && bounds.max_x() > left + EDGE_SLACK_PX
                && bounds.origin.y < bottom - EDGE_SLACK_PX
                && bounds.max_y() > top + EDGE_SLACK_PX
        };
        for item in list.leaf_items() {
            // Стекло проверяем по фигуре члена, а не по охватывающему прямоугольнику с запасом на линзу.
            let touches = match item {
                DrawItem::Glass(group) => group
                    .members
                    .iter()
                    .any(|member| member.shape_bounds_px().is_some_and(inside)),
                other => other.bounds_px().is_some_and(inside),
            };
            assert!(!touches, "{orientation:?}: над чистой зоной нарисовано {item:?}");
        }
    }
}

#[test]
fn mode_capsule_swipes_and_taps_between_calibrate_and_photo() {
    let mut rig = portrait(look_facts());
    assert_eq!(rig.screen.state().mode(), CaptureMode::Calibrate);
    let capsule = center(&rig, "mode_capsule", 0);
    // Выбран CALIBRATE (справа): палец вправо возвращает PHOTO.
    drag_eye(&mut rig, capsule, Vector::new(90.0, 0.0));
    assert_eq!(
        rig.screen.state().mode(),
        CaptureMode::Photo,
        "свайп по капсуле раздела вернул PHOTO"
    );
    // И обратно тапом по подписи CALIBRATE справа от центра.
    rig.run(600);
    let photo_capsule = center(&rig, "mode_capsule", 0);
    assert!(
        rig.screen.node_centers("mode_capsule").len() == 1,
        "в PHOTO одна видимая капсула"
    );
    rig.tap(Point::new(photo_capsule.x + 90.0, photo_capsule.y));
    rig.run(600);
    assert_eq!(
        rig.screen.state().mode(),
        CaptureMode::Calibrate,
        "тап по CALIBRATE в PHOTO открыл раздел"
    );
    // В разделе: тап по подписи PHOTO слева от центра возвращает PHOTO.
    let capsule = center(&rig, "mode_capsule", 0);
    rig.tap(Point::new(capsule.x - 80.0, capsule.y));
    rig.run(600);
    assert_eq!(
        rig.screen.state().mode(),
        CaptureMode::Photo,
        "тап по PHOTO в разделе вернул PHOTO"
    );
}

/// Подписи режимов в списке относительно центра капсулы режимов: (подпись, центр, ширина, прозрачность), px; и
/// полуширина капсулы-стекла.
fn mode_label_geometry(list: &ui_draw::draw_list::DrawList) -> (Vec<(&'static str, f32, f32, f32)>, f32) {
    let capsule = glass_members(list)
        .into_iter()
        .find(|(shape, ..)| {
            matches!(shape, ui_draw::shape::ShapeGeometry::Capsule { size } if size.height > px(40.0) && size.height < px(60.0))
        })
        .expect("капсула режимов в списке рисования");
    let ui_draw::shape::ShapeGeometry::Capsule { size } = capsule.0 else {
        unreachable!("капсулу выбрали выше")
    };
    let mut labels = Vec::new();
    for name in ["PHOTO", "CALIBRATE"] {
        for text in texts_of(list, name, TextRoleKind::ModeLabel) {
            // Подпись ставится от левого края: `tx` — начало строки.
            let width = text.run.advance_px;
            let center = text.transform.tx - capsule.1 + width / 2.0;
            labels.push((name, center, width, text.opacity.get()));
        }
    }
    (labels, size.width / 2.0)
}

/// Капсула сохраняет измеренный размер и промежуток (221,4 pt; 35,4 pt, `[measured: R00/C]`): выбранная подпись
/// стоит по центру, соседняя видна у края и гаснет, а не втискивается в расширенную рамку.
fn assert_measured_capsule_with_faded_neighbour(
    list: &ui_draw::draw_list::DrawList,
    (selected, neighbour): (&str, &str),
    context: &str,
) {
    let (labels, half) = mode_label_geometry(list);
    assert!(
        near(half, px(221.4) / 2.0, 1.5),
        "{context}: полуширина капсулы {half} px, измерено 221,4 pt"
    );
    let find = |name: &str| {
        *labels
            .iter()
            .find(|label| label.0 == name)
            .unwrap_or_else(|| panic!("{context}: подписи {name} нет в {labels:?}"))
    };
    let (_, selected_center, selected_width, selected_opacity) = find(selected);
    let (_, neighbour_center, neighbour_width, neighbour_opacity) = find(neighbour);
    assert!(
        near(selected_center, 0.0, 1.5),
        "{context}: выбранная {selected} не по центру: {selected_center}"
    );
    assert!(
        near(selected_opacity, 1.0, 0.01),
        "{context}: выбранная {selected} не в полную силу"
    );
    let gap = (neighbour_center - selected_center).abs() - (selected_width + neighbour_width) / 2.0;
    assert!(
        near(gap, px(35.4), 1.5),
        "{context}: промежуток между подписями {gap} px, измерено 35,4 pt"
    );
    // Соседка заходит за край стекла (видна лишь частично) и поэтому гаснет, но не исчезает.
    let neighbour_far_edge = neighbour_center.abs() + neighbour_width / 2.0;
    assert!(
        neighbour_far_edge > half,
        "{context}: {neighbour} целиком внутри капсулы: {neighbour_far_edge} <= {half}"
    );
    assert!(
        neighbour_opacity > 0.0 && neighbour_opacity < selected_opacity,
        "{context}: {neighbour} должна гаснуть у края, прозрачность {neighbour_opacity}"
    );
}

#[test]
fn the_capsule_keeps_the_measured_size_and_fades_the_neighbour_label_in_photo_and_in_the_section() {
    // Раздел: выбран CALIBRATE, PHOTO стоит слева.
    let mut rig = portrait(look_facts());
    assert_measured_capsule_with_faded_neighbour(&rig.frame().clone(), ("CALIBRATE", "PHOTO"), "раздел");
    // PHOTO: выбран PHOTO, CALIBRATE стоит справа (пружина карусели оседает дольше 600 мс — ждём с запасом).
    rig.act(ScreenAction::ModeSelected(CaptureMode::Photo));
    let photo_list = rig.run(3000).clone();
    assert_measured_capsule_with_faded_neighbour(&photo_list, ("PHOTO", "CALIBRATE"), "PHOTO");
    // Капсула и выбранная подпись стоят на измеренном x = 220 pt (середина экрана).
    let capsule_x = glass_members(&photo_list)
        .into_iter()
        .find(|(shape, ..)| matches!(shape, ui_draw::shape::ShapeGeometry::Capsule { size } if size.height > px(40.0) && size.height < px(60.0)))
        .map(|(_, x, _)| x)
        .expect("капсула режимов");
    assert!(
        near(capsule_x, px(220.0), 1.0),
        "центр капсулы {capsule_x} px, измерено 220 pt"
    );
}

#[test]
fn menu_button_in_the_section_requests_the_calibration_menu() {
    let mut rig = portrait(look_facts());
    let button = center(&rig, "menu_button", 0);
    rig.tap(button);
    rig.run(200);
    assert_eq!(
        rig.screen.state().calibration().menu(),
        crate::calibration::CalibrationMenu::Root,
        "кнопка меню внутри раздела открывает меню раздела"
    );
}

#[test]
fn in_landscape_the_whole_section_turns_with_the_phone_and_the_scope_follows_its_rotated_zone() {
    for (orientation, quarter) in [
        (DeviceOrientation::LandscapeLeft, 1.0_f32),
        (DeviceOrientation::LandscapeRight, -1.0_f32),
    ] {
        let mut rig = calibrating(look_facts(), orientation);
        let list = rig.frame().clone();
        let layout = *rig.screen.state().calibration().layout().unwrap();
        let images = scope_images(&list);
        assert_eq!(images.len(), 1);
        let image = images[0];
        // Поворот на ±90°: образ оси x картинки — это (a, c); у четверти оборота по часовой он смотрит вниз экрана.
        assert!(
            near(image.transform.a, 0.0, 1e-3) && near(image.transform.c.abs(), 1.0, 1e-3),
            "{orientation:?}: картинка повёрнута на четверть оборота: {:?}",
            image.transform
        );
        assert!(
            near(image.transform.c.signum(), quarter, 1e-3),
            "{orientation:?}: сторона поворота {:?}",
            image.transform
        );
        let zone = layout.eye_rect_on_surface(layout.zones().scope);
        assert!(
            near(image.transform.tx, px(zone.center().x), 0.6) && near(image.transform.ty, px(zone.center().y), 0.6),
            "{orientation:?}: центр картинки на центре повёрнутой зоны"
        );
        assert_eq!(
            (image.size_px.width as u32, image.size_px.height as u32),
            (layout.scope_image_size().width(), layout.scope_image_size().height())
        );
        // Заголовки строк повёрнуты вместе с разделом.
        let titles = texts_of(&list, "MIDTONES", TextRoleKind::CardTitle);
        assert_eq!(titles.len(), 1, "{orientation:?}: подпись строки на месте");
        assert!(
            near(titles[0].transform.c.abs(), 1.0, 1e-3),
            "{orientation:?}: подпись повёрнута: {:?}",
            titles[0].transform
        );
    }
}

#[test]
fn a_drag_along_the_eye_horizontal_changes_a_knob_by_the_same_amount_in_every_orientation() {
    let mut results = Vec::new();
    for orientation in [
        DeviceOrientation::Portrait,
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::LandscapeRight,
        DeviceOrientation::PortraitUpsideDown,
    ] {
        let mut rig = calibrating(look_facts(), orientation);
        // Глазная ось «вправо»: на поверхности в альбоме это вертикаль.
        let eye_right = surface_vector(&rig, Vector::new(1.0, 0.0));
        match orientation {
            DeviceOrientation::Portrait => assert!(eye_right.dx > 0.99 && eye_right.dy.abs() < 1e-9),
            DeviceOrientation::LandscapeLeft => {
                assert!(eye_right.dy > 0.99 && eye_right.dx.abs() < 1e-9, "{eye_right:?}")
            }
            DeviceOrientation::LandscapeRight => {
                assert!(eye_right.dy < -0.99 && eye_right.dx.abs() < 1e-9, "{eye_right:?}")
            }
            DeviceOrientation::PortraitUpsideDown => assert!(eye_right.dx < -0.99 && eye_right.dy.abs() < 1e-9),
        }
        let ruler = center(&rig, "calib_ruler", 0);
        drag_eye(&mut rig, ruler, Vector::new(-70.5, 0.0));
        let sent = set_looks(&rig.effects());
        let value = sent
            .last()
            .unwrap_or_else(|| panic!("{orientation:?}: ручка не сдвинулась"))
            .knob(GlobalKnob::Contrast)
            .get();
        assert!((value - 0.3).abs() < 1e-6, "{orientation:?}: значение {value}");
        results.push(value);
    }
    assert!(
        results.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 1e-6),
        "{results:?}"
    );
}

#[test]
fn in_landscape_taps_and_scope_swipes_use_eye_directions_too() {
    for orientation in [DeviceOrientation::LandscapeLeft, DeviceOrientation::LandscapeRight] {
        let mut rig = calibrating(look_facts(), orientation);
        // Свайп прибора глазами влево.
        let scope = center(&rig, "calib_scope", 0);
        drag_eye(&mut rig, scope, Vector::new(-80.0, 0.0));
        assert_eq!(
            rig.screen.state().calibration().scope_kind(),
            ScopeKind::RgbParade,
            "{orientation:?}"
        );
        // Капсула режимов в альбоме листается глазной горизонталью: палец глазами вправо возвращает PHOTO.
        let capsule = center(&rig, "mode_capsule", 0);
        drag_eye(&mut rig, capsule, Vector::new(90.0, 0.0));
        assert_eq!(
            rig.screen.state().mode(),
            CaptureMode::Photo,
            "{orientation:?}: свайп по капсуле раздела"
        );
        rig.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
        rig.run(600);
        // «Чистяк» в повёрнутом разделе нажимается там, где его рисует раздел.
        let clean = center(&rig, "calib_clean", 0);
        let t = rig.now_ms;
        rig.touch(TouchPhase::Began, clean, t);
        rig.run(150);
        assert!(
            rig.effects()
                .iter()
                .any(|effect| matches!(effect, Effect::CleanView(look_contract::CleanView::ShowBase))),
            "{orientation:?}: тап по повёрнутой кнопке сработал"
        );
        let t = rig.now_ms;
        rig.touch(TouchPhase::Ended, clean, t);
        rig.run(100);
        // Список в альбоме листается глазной вертикалью (на поверхности — горизонталью).
        let window = center(&rig, "calib_knobs_window", 0);
        let delta = surface_vector(&rig, Vector::new(0.0, -120.0));
        rig.drag(window, Point::new(window.x + delta.dx, window.y + delta.dy), 200);
        rig.run(600);
        assert!(
            rig.screen.calibration_scroll_offset().unwrap() > 60.0,
            "{orientation:?}: список прокрутился"
        );
    }
}

#[test]
fn turning_the_phone_rebuilds_the_section_and_keeps_every_value_from_the_facts() {
    let facts = edited(|s| s.with_knob(GlobalKnob::Contrast, knob(0.3)));
    let mut rig = portrait(facts);
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(600);
    let list = rig.frame().clone();
    assert_eq!(
        text_count_of(&list, "+0.30", TextRoleKind::ScaleValue),
        1,
        "значение пережило пересборку"
    );
    let position = rig.screen.calibration_row_position_steps(0).unwrap();
    assert!((position - 3.0).abs() < 0.02, "линейка стоит на 0,3: {position}");
    assert!(
        rig.effects().iter().all(|effect| !matches!(effect, Effect::Look(_))),
        "пересборка не шлёт действий"
    );
}

#[test]
fn tapping_the_clean_preview_zone_does_not_focus_or_lock() {
    let mut rig = portrait(look_facts());
    let layout = *rig.screen.state().calibration().layout().unwrap();
    let clean = layout.zones().clean_preview.center();
    rig.tap(clean);
    rig.run(200);
    rig.frame();
    assert!(
        rig.screen
            .focus_frame_phase()
            .is_none_or(|phase| phase == ui_kit::focus_frame::FocusPhase::Hidden),
        "рамки фокуса нет"
    );
    assert!(
        rig.effects().iter().all(|effect| !matches!(effect, Effect::Camera(_))),
        "тап по чистой зоне не наводит камеру"
    );
}

#[test]
fn calibrate_frames_are_valid_draw_lists_in_every_orientation_and_the_diagnostics_stay_clean() {
    for orientation in [
        DeviceOrientation::Portrait,
        DeviceOrientation::LandscapeLeft,
        DeviceOrientation::LandscapeRight,
        DeviceOrientation::PortraitUpsideDown,
    ] {
        let mut rig = calibrating(look_facts_with_edits(), orientation);
        rig.run(300);
        assert!(rig.last.validate().is_ok(), "{orientation:?}: словарь рисования");
        let diagnostics = rig.screen.diagnostics();
        assert_eq!(
            diagnostics.frame_failures, 0,
            "{orientation:?}: {:?}",
            diagnostics.last_failure
        );
        assert_eq!(diagnostics.unhandled_gestures, 0);
        assert_eq!(diagnostics.invalid_knob_values, 0);
    }
}

fn look_facts_with_edits() -> LookFacts {
    let mut facts = look_facts();
    facts.current = LookRecipe {
        base: ToneBaseModel::Aces,
        settings: facts.current.settings.with_knob(GlobalKnob::Warmth, knob(-0.4)),
    };
    facts.active = PresetId::Builtin(BuiltinPreset::AsEye);
    facts.edits = EditState::EditedOnTop;
    facts
}

/// Все четыре положения телефона.
const ALL_ORIENTATIONS: [DeviceOrientation; 4] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::PortraitUpsideDown,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
];

/// Центр чистой зоны превью на поверхности (pt).
fn clean_zone_center(rig: &Rig) -> Point {
    let layout = *rig.screen.state().calibration().layout().expect("раскладка раздела");
    layout.eye_point_on_surface(layout.zones().clean_preview.center())
}

#[test]
fn an_eye_horizontal_swipe_on_the_clean_zone_switches_the_mode_with_the_right_sign_in_every_orientation() {
    for orientation in ALL_ORIENTATIONS {
        // Вправо глазами = предыдущий режим: из CALIBRATE (последний в карусели) — PHOTO.
        let mut rig = calibrating(look_facts(), orientation);
        let zone = clean_zone_center(&rig);
        drag_eye(&mut rig, zone, Vector::new(90.0, 0.0));
        assert_eq!(
            rig.screen.state().mode(),
            CaptureMode::Photo,
            "{orientation:?}: свайп вправо по чистой зоне вернул PHOTO"
        );
        assert_eq!(
            rig.screen.state().calibration().menu(),
            crate::calibration::CalibrationMenu::Closed,
            "{orientation:?}: меню не открылось"
        );

        // Влево глазами = следующий режим; у CALIBRATE следующего нет, режим не меняется (знак не перевёрнут).
        let mut rig = calibrating(look_facts(), orientation);
        let zone = clean_zone_center(&rig);
        drag_eye(&mut rig, zone, Vector::new(-90.0, 0.0));
        assert_eq!(
            rig.screen.state().mode(),
            CaptureMode::Calibrate,
            "{orientation:?}: свайп влево по чистой зоне не должен менять режим"
        );
    }
}

#[test]
fn an_eye_vertical_swipe_on_the_clean_zone_neither_opens_the_menu_nor_changes_the_mode() {
    for orientation in ALL_ORIENTATIONS {
        for eye_dy in [-90.0, 90.0] {
            let mut rig = calibrating(look_facts(), orientation);
            let zone = clean_zone_center(&rig);
            drag_eye(&mut rig, zone, Vector::new(0.0, eye_dy));
            assert_eq!(
                rig.screen.state().calibration().menu(),
                crate::calibration::CalibrationMenu::Closed,
                "{orientation:?}, dy {eye_dy}: меню не должно открываться свайпом по чистой зоне"
            );
            assert_eq!(
                rig.screen.state().mode(),
                CaptureMode::Calibrate,
                "{orientation:?}, dy {eye_dy}"
            );
        }
    }
}
