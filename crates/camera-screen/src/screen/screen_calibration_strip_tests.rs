//! Полоска прибора, свайп страниц, CLEAN внизу, соседние основы, мёртвые края «↻», зазор над капсулой и модальность
//! карточки — пальцем, как в руках: касание в точках поверхности → состояние и эффекты (сессия 14n, решения 258, 259,
//! 277–281, 283).

use look_contract::{GlobalKnob, LookControl, LookIntent};
use scope_contract::{ScopeIntent, ScopeKind, VectorscopeAxes};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{calibrating, center, drag_eye, edited, portrait, surface_vector};
use super::test_rig::*;
use super::touch_map_test_support::TouchMap;
use crate::action::ScreenAction;
use crate::calibration::{CleanViewState, SectionCard};
use crate::effect::Effect;
use crate::look_test_support::{knob, look_facts, look_intents};
use crate::modes::CaptureMode;

const ORIENTATIONS: [DeviceOrientation; 3] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
];

fn scope_kind(rig: &Rig) -> ScopeKind {
    rig.screen.state().calibration().scope_kind()
}

/// Запросы прибора, ушедшие рендеру с прошлого вызова.
fn scope_requests(rig: &mut Rig) -> Vec<scope_contract::ScopeRequest> {
    rig.effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Scopes(ScopeIntent::Show(request)) => Some(request),
            _ => None,
        })
        .collect()
}

// ----- точки страниц ---------------------------------------------------------------------------------------------------

#[test]
fn tapping_a_page_dot_selects_that_scope_and_asks_the_renderer_for_it_in_every_orientation() {
    for orientation in ORIENTATIONS {
        let mut rig = calibrating(look_facts(), orientation);
        assert_eq!(scope_kind(&rig), ScopeKind::Waveform);
        for (index, expected) in ScopeKind::ALL.into_iter().enumerate().rev() {
            rig.effects();
            rig.tap(center(&rig, "calib_scope_page_dot", index));
            rig.run(200);
            assert_eq!(scope_kind(&rig), expected, "{orientation:?}: точка {index}");
            if index != 0 || scope_kind(&rig) != ScopeKind::Waveform {
                let requests = scope_requests(&mut rig);
                assert_eq!(requests.len(), 1, "{orientation:?}: рендеру ушёл один запрос");
                assert_eq!(requests[0].kind, expected);
            }
        }
        // Тап по уже выбранной точке ничего не просит заново.
        rig.effects();
        rig.tap(center(&rig, "calib_scope_page_dot", 0));
        rig.run(200);
        assert!(rig.effects().is_empty(), "{orientation:?}: повтор выбора молчит");
    }
}

#[test]
fn page_dot_cells_are_44_pt_tall_and_never_share_a_point() {
    let rig = portrait(look_facts());
    let scene = rig.screen.scene_for_tests().expect("сцена");
    let map = TouchMap::of_subtree(scene, "calibration_root");
    let cells: Vec<_> = map
        .live()
        .iter()
        .filter(|area| area.label == "calib_scope_page_dot")
        .collect();
    assert_eq!(cells.len(), ScopeKind::ALL.len());
    for cell in &cells {
        assert!(
            (cell.declared.size.height - 44.0).abs() < 1e-6,
            "ячейка точки 44 pt по высоте"
        );
    }
    for (index, first) in cells.iter().enumerate() {
        for second in &cells[index + 1..] {
            assert!(
                first.declared.max_x() <= second.declared.min_x() + 1e-9,
                "ячейки встык без перекрытия"
            );
        }
    }
}

// ----- оси вектороскопа ------------------------------------------------------------------------------------------------

#[test]
fn the_axes_toggle_switches_the_vectorscope_standard_with_the_menu_closed_and_resends_the_request() {
    for orientation in ORIENTATIONS {
        let mut rig = calibrating(look_facts(), orientation);
        rig.act(ScreenAction::ScopeKindSelected(ScopeKind::Vectorscope));
        rig.run(300);
        rig.effects();
        assert_eq!(rig.screen.state().calibration().card(), SectionCard::Closed);
        assert_eq!(
            rig.screen.state().calibration().vectorscope_axes(),
            VectorscopeAxes::YCbCrBt709
        );

        rig.tap(center(&rig, "calib_scope_axes", 1));
        rig.run(200);
        assert_eq!(
            rig.screen.state().calibration().vectorscope_axes(),
            VectorscopeAxes::OklabAb,
            "{orientation:?}: правый сегмент — OKLab"
        );
        assert_eq!(
            rig.screen.state().calibration().card(),
            SectionCard::Closed,
            "меню не открывалось"
        );
        let requests = scope_requests(&mut rig);
        assert_eq!(requests.len(), 1, "{orientation:?}");
        assert_eq!(requests[0].vectorscope_axes, VectorscopeAxes::OklabAb);
        assert_eq!(requests[0].kind, ScopeKind::Vectorscope);

        rig.tap(center(&rig, "calib_scope_axes", 0));
        rig.run(200);
        assert_eq!(
            rig.screen.state().calibration().vectorscope_axes(),
            VectorscopeAxes::YCbCrBt709
        );
    }
}

#[test]
fn the_axes_toggle_is_drawn_and_touchable_only_on_the_vectorscope_page() {
    let mut rig = portrait(look_facts());
    let list = rig.frame().clone();
    assert_eq!(
        text_count_of(&list, "OKLab", TextRoleKind::SegmentLabel),
        0,
        "на waveform переключателя нет"
    );
    // Место, где на странице вектороскопа стоит правый сегмент: палец там на waveform ничего не меняет.
    rig.act(ScreenAction::ScopeKindSelected(ScopeKind::Vectorscope));
    rig.run(300);
    let list = rig.frame().clone();
    assert_eq!(text_count_of(&list, "OKLab", TextRoleKind::SegmentLabel), 1);
    assert_eq!(text_count_of(&list, "Y'CbCr", TextRoleKind::SegmentLabel), 1);
    let toggle_spot = center(&rig, "calib_scope_axes", 1);
    rig.act(ScreenAction::ScopeKindSelected(ScopeKind::RgbParade));
    rig.run(300);
    rig.effects();
    rig.tap(toggle_spot);
    rig.run(200);
    assert_eq!(
        rig.screen.state().calibration().vectorscope_axes(),
        VectorscopeAxes::YCbCrBt709,
        "на другой странице тап по месту переключателя оси не меняет"
    );
    assert_eq!(scope_kind(&rig), ScopeKind::RgbParade);
}

#[test]
fn the_waveform_and_parade_pages_label_the_levels_and_the_vectorscope_does_not() {
    let mut rig = portrait(look_facts());
    for kind in [ScopeKind::Waveform, ScopeKind::RgbParade] {
        rig.act(ScreenAction::ScopeKindSelected(kind));
        rig.run(300);
        let list = rig.frame().clone();
        for label in ["0", "25", "50", "75", "100"] {
            assert_eq!(
                text_count_of(&list, label, TextRoleKind::Caption),
                1,
                "{kind:?}: число уровня {label} нарисовано один раз"
            );
        }
    }
    rig.act(ScreenAction::ScopeKindSelected(ScopeKind::Vectorscope));
    rig.run(300);
    let list = rig.frame().clone();
    for label in ["25", "50", "75", "100"] {
        assert_eq!(
            text_count_of(&list, label, TextRoleKind::Caption),
            0,
            "у вектороскопа чисел уровней нет"
        );
    }
}

#[test]
fn level_numbers_stand_left_of_the_picture_each_on_the_height_of_its_own_level_line() {
    for orientation in [DeviceOrientation::Portrait, DeviceOrientation::LandscapeLeft] {
        let mut rig = calibrating(look_facts(), orientation);
        let layout = *rig.screen.state().calibration().layout().expect("раскладка");
        let list = rig.frame().clone();
        let picture = layout.scope_image_rect_for(ScopeKind::Waveform);
        for line in ScopeKind::Waveform.level_lines() {
            let runs = texts_of(&list, line.label, TextRoleKind::Caption);
            assert_eq!(runs.len(), 1, "{orientation:?}: число {}", line.label);
            let item = &runs[0];
            // Базовая линия подписи: поверхность → глаз (в альбоме оси повёрнуты); центр букв выше неё на половину
            // их высоты.
            let baseline = layout.surface_point_in_eye(Point::new(
                f64::from(item.transform.tx) / f64::from(px(1.0)),
                f64::from(item.transform.ty) / f64::from(px(1.0)),
            ));
            // Номинальная высота линии в картинке; крайние подписи прижаты внутрь на половину высоты букв.
            let half_height = f64::from(item.run.cap_height_px / px(1.0)) / 2.0;
            let centre_y = baseline.y - half_height;
            let nominal_y = picture.min_y() + f64::from(line.nominal_fraction_from_top()) * picture.size.height;
            let expected_y = nominal_y.clamp(picture.min_y() + half_height, picture.max_y() - half_height);
            assert!(
                (centre_y - expected_y).abs() < 0.5,
                "{orientation:?}: число {} на высоте {} вместо {expected_y} (линия {nominal_y})",
                line.label,
                centre_y
            );
            let right_edge = baseline.x + f64::from(item.run.advance_px / px(1.0)) / 2.0;
            assert!(
                right_edge <= picture.min_x() + 1e-3,
                "{orientation:?}: число {} не лежит в поле слева от картинки: правый край {right_edge}",
                line.label
            );
        }
    }
}

// ----- CLEAN внизу ---------------------------------------------------------------------------------------------------------

#[test]
fn clean_sits_on_the_capsule_line_left_of_the_capsule_and_a_held_finger_shows_the_base() {
    for orientation in ORIENTATIONS {
        let mut rig = calibrating(look_facts(), orientation);
        let layout = *rig.screen.state().calibration().layout().expect("раскладка");
        let clean = center(&rig, "calib_clean", 0);
        let expected = layout.eye_point_on_surface(layout.clean_button_center());
        assert!(
            (clean.x - expected.x).abs() < 1e-6 && (clean.y - expected.y).abs() < 1e-6,
            "{orientation:?}: CLEAN там, где раскладка"
        );
        assert!(
            layout.clean_button_center().x < layout.carousel_center().x,
            "{orientation:?}: слева от капсулы"
        );
        let started = rig.now_ms;
        rig.touch(TouchPhase::Began, clean, started);
        rig.run(200);
        assert_eq!(
            rig.screen.state().calibration().clean_view(),
            CleanViewState::HoldingBase
        );
        assert!(
            rig.effects()
                .iter()
                .any(|effect| matches!(effect, Effect::CleanView(look_contract::CleanView::ShowBase)))
        );
        let ended = rig.now_ms;
        rig.touch(TouchPhase::Ended, clean, ended);
        rig.run(100);
        assert_eq!(
            rig.screen.state().calibration().clean_view(),
            CleanViewState::ShowingEdited
        );
        assert!(
            rig.effects()
                .iter()
                .any(|effect| matches!(effect, Effect::CleanView(look_contract::CleanView::ShowEdited)))
        );
    }
}

#[test]
fn a_hold_that_outlives_a_rebuilt_section_is_released_exactly_once() {
    // Удержание пришло действием (не пальцем этой кнопки) и пережило пересборку раздела поворотом: палец прежней
    // кнопки потерян, вид должен вернуться сам — один раз.
    let mut rig = portrait(look_facts());
    rig.act(ScreenAction::CleanViewPressed);
    rig.run(300);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        CleanViewState::HoldingBase
    );
    rig.effects();
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(400);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        CleanViewState::ShowingEdited
    );
    let released = rig
        .effects()
        .into_iter()
        .filter(|effect| matches!(effect, Effect::CleanView(look_contract::CleanView::ShowEdited)))
        .count();
    assert_eq!(released, 1, "ShowEdited ровно один");
}

#[test]
fn clean_button_holds_the_base_while_pressed_and_releases_on_lift_or_cancel() {
    let mut rig = portrait(look_facts());
    let clean = center(&rig, "calib_clean", 0);
    let shown = |rig: &mut Rig| -> Vec<look_contract::CleanView> {
        rig.effects()
            .into_iter()
            .filter_map(|effect| match effect {
                Effect::CleanView(view) => Some(view),
                _ => None,
            })
            .collect()
    };
    // Нажатие и удержание.
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, clean, t);
    rig.run(200);
    assert_eq!(
        shown(&mut rig),
        vec![look_contract::CleanView::ShowBase],
        "нажали — показана основа"
    );
    // Отпускание.
    let t = rig.now_ms;
    rig.touch(TouchPhase::Ended, clean, t);
    rig.run(100);
    assert_eq!(
        shown(&mut rig),
        vec![look_contract::CleanView::ShowEdited],
        "отпустили — вид вернулся"
    );
    // Отмена жеста системой.
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, clean, t);
    rig.run(100);
    assert_eq!(shown(&mut rig), vec![look_contract::CleanView::ShowBase]);
    let t = rig.now_ms;
    rig.touch(TouchPhase::Cancelled, clean, t);
    rig.run(100);
    assert_eq!(
        shown(&mut rig),
        vec![look_contract::CleanView::ShowEdited],
        "отмена тоже отпускает"
    );
}

#[test]
fn clean_view_never_sticks_when_the_touch_stream_breaks_or_the_phone_turns() {
    let mut rig = portrait(look_facts());
    let clean = center(&rig, "calib_clean", 0);
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, clean, t);
    rig.run(100);
    rig.effects();
    // Разрыв потока касаний посреди удержания.
    rig.screen.handle_touch(super::ScreenTouch::Gap);
    rig.run(200);
    assert!(
        rig.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::CleanView(look_contract::CleanView::ShowEdited))),
        "после разрыва потока вид вернулся"
    );
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::ShowingEdited
    );
    // Поворот телефона посреди удержания: раздел пересобирается, кнопка не залипает.
    let clean = center(&rig, "calib_clean", 0);
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, clean, t);
    rig.run(100);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::HoldingBase
    );
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(400);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::ShowingEdited,
        "пересборка раздела не оставила «чистяк» нажатым"
    );
}

#[test]
fn a_clean_view_held_by_an_action_stays_held_across_frames() {
    // Удержание пришло действием, а не пальцем кнопки (сценарий, сквозной тест, другой хост): страховка от залипания
    // срабатывает только после пересборки шапки и не должна отпускать такое удержание на следующем кадре (дефект,
    // найденный сквозным тестом `calibration_end_to_end`: «чистяк» отпускался раньше, чем рендер успевал его показать).
    let mut rig = portrait(look_facts());
    rig.act(ScreenAction::CleanViewPressed);
    rig.run(500);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::HoldingBase,
        "полсекунды кадров не отпустили удержание без пальца"
    );
    assert!(
        !rig.effects()
            .iter()
            .any(|effect| matches!(effect, Effect::CleanView(look_contract::CleanView::ShowEdited))),
        "экран сам не просил вернуть правки"
    );
    rig.act(ScreenAction::CleanViewReleased);
    assert_eq!(
        rig.screen.state().calibration().clean_view(),
        crate::calibration::CleanViewState::ShowingEdited
    );
}

#[test]
fn clean_is_gone_from_the_tone_header() {
    let rig = portrait(look_facts());
    let nodes = rig.screen.node_centers("calib_clean");
    assert_eq!(nodes.len(), 1, "кнопка CLEAN одна");
    let carousel = center(&rig, "calib_base_carousel", 0);
    assert!(
        nodes[0].y > carousel.y + 300.0,
        "CLEAN не в шапке колонки, а внизу экрана"
    );
}

// ----- мёртвые края «↻» ------------------------------------------------------------------------------------------------------

#[test]
fn the_formerly_dead_strips_of_a_row_reset_above_the_row_and_at_the_right_edge_reset_the_knob() {
    let facts = edited(|settings| {
        GlobalKnob::ALL
            .iter()
            .fold(settings, |acc, knob_id| acc.with_knob(*knob_id, knob(0.3)))
    });
    let mut rig = portrait(facts);
    let widgets = crate::config::ScreenConfig::default().calibration.widgets;
    // Точки считаются от центра рисунка «↻» и чисел конфига, а не из карты касаний: проверка не пересказывает
    // раскладку, а спрашивает палец. Полоса над строкой — выше верха заголовка строки на пол-полосы расширения;
    // зона 44 pt растёт от края строки внутрь: раньше родитель-строка резал верхнюю полосу.
    let mut centres = rig.screen.node_centers("calib_reset");
    centres.sort_by(|first, second| first.y.total_cmp(&second.y));
    assert!(
        centres.len() >= 3,
        "в окне видны «↻» нескольких строк: {}",
        centres.len()
    );
    let half_frame = widgets.reset_button_diameter_pt / 2.0;
    let above_title_y = widgets.row_title_height_pt / 2.0 + widgets.row_reset_reach_above_pt / 2.0;
    for (index, centre) in centres.iter().take(3).enumerate() {
        for (name, probe) in [
            ("полоса над строкой", Point::new(centre.x, centre.y - above_title_y)),
            // Крайний правый столбец строки: «↻» стоит у её правого края, и зона 44 pt обязана доходить до него.
            ("правый край строки", Point::new(centre.x + half_frame - 1.0, centre.y)),
            // Расширение зоны слева от рисунка кнопки (рисунок 26 pt, зона 44 pt): только оно ловит этот палец.
            (
                "расширение слева",
                Point::new(centre.x - half_frame - 6.0, centre.y + 6.0),
            ),
        ] {
            rig.effects();
            rig.tap(probe);
            rig.run(100);
            let resets: Vec<LookIntent> = look_intents(&rig.effects()).into_iter().cloned().collect();
            assert!(
                matches!(resets.as_slice(), [LookIntent::ResetControl(LookControl::Knob(_))]),
                "«↻» №{index}, {name}: ждали один сброс ручки, получили {resets:?}"
            );
        }
    }
}

// ----- свайп страниц: один порог -----------------------------------------------------------------------------------------

#[test]
fn a_25_pt_swipe_flips_one_page_in_its_direction_and_a_15_pt_one_does_nothing() {
    for orientation in ORIENTATIONS {
        let mut rig = calibrating(look_facts(), orientation);
        let layout = *rig.screen.state().calibration().layout().expect("раскладка");
        let area = layout.scope_picture_area();
        let start = layout.eye_point_on_surface(area.center());

        drag_eye(&mut rig, start, Vector::new(-15.0, 0.0));
        assert_eq!(
            scope_kind(&rig),
            ScopeKind::Waveform,
            "{orientation:?}: 15 pt — не свайп"
        );
        drag_eye(&mut rig, start, Vector::new(-25.0, 0.0));
        assert_eq!(
            scope_kind(&rig),
            ScopeKind::RgbParade,
            "{orientation:?}: 25 pt влево — следующая страница"
        );
        drag_eye(&mut rig, start, Vector::new(-25.0, 0.0));
        assert_eq!(scope_kind(&rig), ScopeKind::Vectorscope);
        drag_eye(&mut rig, start, Vector::new(-60.0, 0.0));
        assert_eq!(
            scope_kind(&rig),
            ScopeKind::Vectorscope,
            "{orientation:?}: за последней страницей листать некуда"
        );
        drag_eye(&mut rig, start, Vector::new(25.0, 0.0));
        assert_eq!(
            scope_kind(&rig),
            ScopeKind::RgbParade,
            "{orientation:?}: вправо — назад"
        );
    }
}

#[test]
fn a_swipe_started_on_the_strip_flips_pages_too() {
    let mut rig = portrait(look_facts());
    let layout = *rig.screen.state().calibration().layout().expect("раскладка");
    // Пустое место полоски между именем и точками: не цель, свайп отсюда листает так же, как по картинке.
    let strip = layout.scope_strip();
    let start = layout.eye_point_on_surface(Point::new(strip.min_x() + 110.0, strip.center().y));
    drag_eye(&mut rig, start, Vector::new(-40.0, 0.0));
    assert_eq!(scope_kind(&rig), ScopeKind::RgbParade);
}

/// Быстрый мазок глазами на `eye` от `from`: палец отпускается на ходу, со скоростью (в отличие от `drag_eye`, который
/// перед отпусканием замирает). Так листают режимы капсулой, поэтому им проверяется «ничего не листнулось».
fn flick_eye(rig: &mut Rig, from: Point, eye: Vector) {
    let delta = surface_vector(rig, eye);
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, from, start);
    for step in 1..=4 {
        let t = f64::from(step) / 4.0;
        rig.touch(
            TouchPhase::Moved,
            Point::new(from.x + delta.dx * t, from.y + delta.dy * t),
            start + i64::from(step) * 8,
        );
    }
    rig.touch(
        TouchPhase::Ended,
        Point::new(from.x + delta.dx, from.y + delta.dy),
        start + 36,
    );
    rig.now_ms = start + 46;
    rig.frame();
    rig.run(600);
}

// ----- зазор между прибором и капсулой (решение 277) ----------------------------------------------------------------------

#[test]
fn a_touch_in_the_gap_between_the_scope_and_the_capsule_flips_neither_the_scope_nor_the_mode() {
    for orientation in ORIENTATIONS {
        let mut rig = calibrating(look_facts(), orientation);
        let layout = *rig.screen.state().calibration().layout().expect("раскладка");
        let scope_bottom = layout.zones().scope.max_y();
        let capsule = layout.carousel_center();
        let capsule_top = capsule.y - crate::config::ScreenConfig::default().widgets.bottom.capsule_height_pt / 2.0;
        assert!(
            capsule_top > scope_bottom,
            "{orientation:?}: между прибором и капсулой есть зазор"
        );
        // Опора проверки: мазок по САМОЙ капсуле режим меняет — значит, стенд умеет показать листание режимов.
        let on_capsule = layout.eye_point_on_surface(capsule);
        flick_eye(&mut rig, on_capsule, Vector::new(120.0, 0.0));
        assert_ne!(
            rig.screen.state().mode(),
            CaptureMode::Calibrate,
            "{orientation:?}: контроль — капсула листает режимы"
        );
        rig.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
        rig.run(400);

        let gap_y = (scope_bottom + capsule_top) / 2.0;
        for x in [layout.eye_size().width / 2.0, 150.0] {
            let start = layout.eye_point_on_surface(Point::new(x, gap_y));
            rig.effects();
            for (flick, dx) in [(false, -60.0), (false, 60.0), (true, -60.0), (true, 60.0)] {
                if flick {
                    flick_eye(&mut rig, start, Vector::new(dx, 0.0));
                } else {
                    drag_eye(&mut rig, start, Vector::new(dx, 0.0));
                }
                assert_eq!(
                    scope_kind(&rig),
                    ScopeKind::Waveform,
                    "{orientation:?} x={x}: прибор не листнулся"
                );
                assert_eq!(
                    rig.screen.state().mode(),
                    CaptureMode::Calibrate,
                    "{orientation:?} x={x}: режим не сменился"
                );
            }
        }
    }
}

// ----- модальность карточки (решение 281) -----------------------------------------------------------------------------------

#[test]
fn with_the_card_open_the_capsule_never_changes_the_mode_and_a_tap_outside_the_card_only_closes_it() {
    for orientation in ORIENTATIONS {
        let mut rig = calibrating(look_facts(), orientation);
        let capsule = center(&rig, "mode_capsule", 0);
        let layout = *rig.screen.state().calibration().layout().expect("раскладка");
        rig.act(ScreenAction::ControlsMenuOpenRequested);
        rig.run(700);
        assert_eq!(rig.screen.state().calibration().card(), SectionCard::Presets);
        rig.effects();
        // Карточка лежит над нижним рядом и накрывает капсулу: палец по ней режим не меняет.
        rig.tap(capsule);
        rig.run(500);
        assert_eq!(
            rig.screen.state().mode(),
            CaptureMode::Calibrate,
            "{orientation:?}: режим остался"
        );
        // Тап мимо карточки (по затемнению над ней) только закрывает меню, и режим снова не меняется.
        let above_card = layout.eye_point_on_surface(Point::new(layout.eye_size().width / 2.0, 20.0));
        rig.tap(above_card);
        rig.run(500);
        assert_eq!(
            rig.screen.state().calibration().card(),
            SectionCard::Closed,
            "{orientation:?}"
        );
        assert_eq!(rig.screen.state().mode(), CaptureMode::Calibrate, "{orientation:?}");
    }
}

#[test]
fn a_swipe_on_the_capsule_with_the_card_open_does_not_change_the_mode_either() {
    let mut rig = portrait(look_facts());
    let capsule = center(&rig, "mode_capsule", 0);
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.run(700);
    drag_eye(&mut rig, capsule, Vector::new(120.0, 0.0));
    assert_eq!(
        rig.screen.state().mode(),
        CaptureMode::Calibrate,
        "открытое меню модальное"
    );
}
