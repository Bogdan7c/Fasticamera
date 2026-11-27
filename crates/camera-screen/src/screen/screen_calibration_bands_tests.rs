//! Раздел калибровки: полосы оттенка, прокрутка списка ручек и приборы. Общие помощники и тесты ручек, основы,
//! «чистяка» — в `screen_calibration_tests`.

use look_contract::{BandAxis, EditState, GlobalKnob, HueBandId, LookControl, LookIntent};
use scope_contract::{ScopeIntent, ScopeKind};
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_draw::draw_list::DrawItem;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{center, drag_eye, edited, portrait, set_looks, surface_vector};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::effect::Effect;
use crate::look_test_support::{band, knob, look_facts, look_intents};

const FRAME: i64 = 16;

#[test]
fn tapping_a_band_dot_selects_that_band_and_its_rulers_drive_that_band() {
    let mut rig = portrait(look_facts());
    assert_eq!(
        rig.screen.state().calibration().selected_band(),
        Some(&band("red")),
        "по умолчанию выбрана первая полоса"
    );
    // Точки полос лежат в конце списка: прокручиваем колонку вверх пальцем.
    scroll_list_to_end(&mut rig);
    let sky = center(&rig, "calib_band_dot", 1);
    rig.tap(sky);
    rig.run(100);
    assert_eq!(rig.screen.state().calibration().selected_band(), Some(&band("sky")));
    // Линейка HUE (первая ось полосы) двигает ось выбранной полосы, а не другой.
    let hue = center(&rig, "calib_ruler", 8);
    drag_eye(&mut rig, hue, Vector::new(-knob_finger_pt(60.0), 0.0));
    let sent = set_looks(&rig.effects());
    let last = sent.last().expect("SetLook с полосой");
    let sky_adjustment = last.bands.for_band(&band("sky"));
    assert!(
        (sky_adjustment.axis(BandAxis::Hue).get() - 0.3).abs() < 1e-6,
        "{last:?}"
    );
    assert!(
        last.bands.for_band(&band("red")).is_neutral(),
        "соседняя полоса не тронута"
    );
    assert_eq!(
        sky_adjustment.axis(BandAxis::Saturation).get(),
        0.0,
        "соседние оси не тронуты"
    );
}

#[test]
fn band_reset_arrow_resets_the_axis_of_the_selected_band() {
    let mut facts = look_facts();
    let adjusted = facts
        .current
        .settings
        .bands
        .for_band(&band("red"))
        .with_axis(BandAxis::Luminance, knob(0.4));
    facts.current.settings.bands = facts.current.settings.bands.with_adjustment(adjusted);
    facts.edits = EditState::EditedOnTop;
    let mut rig = portrait(facts);
    scroll_list_to_end(&mut rig);
    // Третья ось (LUM) правлена, первые две равны пресету.
    let lum_reset = center(&rig, "calib_reset", 10);
    rig.tap(lum_reset);
    rig.run(100);
    let intents = look_intents(&rig.effects()).into_iter().cloned().collect::<Vec<_>>();
    assert_eq!(
        intents,
        vec![LookIntent::ResetControl(LookControl::Band {
            band: band("red"),
            axis: BandAxis::Luminance
        })]
    );
}

#[test]
fn a_band_that_differs_from_the_preset_is_marked_under_its_dot() {
    let mut facts = look_facts();
    let adjusted = facts
        .current
        .settings
        .bands
        .for_band(&band("sky"))
        .with_axis(BandAxis::Saturation, knob(-0.2));
    facts.current.settings.bands = facts.current.settings.bands.with_adjustment(adjusted);
    facts.edits = EditState::EditedOnTop;
    let mut plain = portrait(look_facts());
    let mut marked = portrait(facts);
    scroll_list_to_end(&mut plain);
    scroll_list_to_end(&mut marked);
    let shapes = |rig: &mut Rig| {
        rig.frame()
            .leaf_items()
            .filter(|item| matches!(item, DrawItem::Shape(_)))
            .count()
    };
    assert_eq!(
        shapes(&mut marked),
        shapes(&mut plain) + 2,
        "у полосы с правками поверх пресета две лишние фигуры: кольцо «есть правки» и метка «отличается от пресета» под точкой"
    );
}

/// Листает список ручек пальцем до конца (глазами — вверх).
pub(super) fn scroll_list_to_end(rig: &mut Rig) {
    let list_point = center(rig, "calib_knobs_window", 0);
    for _ in 0..4 {
        let delta = surface_vector(rig, Vector::new(0.0, -300.0));
        rig.drag(
            list_point,
            Point::new(list_point.x + delta.dx, list_point.y + delta.dy),
            200,
        );
        rig.run(800);
    }
    rig.effects();
}

#[test]
fn dragging_the_list_scrolls_the_rows_and_header_stays_put() {
    let mut rig = portrait(look_facts());
    let before = texts_of(&rig.frame().clone(), "CONTRAST", TextRoleKind::CardTitle)[0]
        .transform
        .ty;
    let base_before = rig.screen.node_centers("calib_base_carousel")[0];
    let list_point = center(&rig, "calib_knobs_window", 0);
    // Тянем по свободному месту окна списка, не по линейке: заголовок группы лежит над первой строкой.
    rig.drag(
        Point::new(list_point.x, list_point.y + 5.0),
        Point::new(list_point.x, list_point.y - 175.0),
        200,
    );
    rig.run(800);
    let offset = rig.screen.calibration_scroll_offset().unwrap();
    assert!(offset > 100.0, "список прокрутился: {offset}");
    let after_text = texts_of(&rig.frame().clone(), "CONTRAST", TextRoleKind::CardTitle);
    // Строка уехала вверх на смещение (в px) либо погасла у края.
    assert!(
        after_text.is_empty() || after_text[0].transform.ty < before - px(offset - 2.0) + 3.0,
        "строка уехала вверх вместе со списком"
    );
    assert_eq!(
        rig.screen.node_centers("calib_base_carousel")[0],
        base_before,
        "шапка не двигается"
    );
    assert!(
        rig.effects().iter().all(|effect| !matches!(effect, Effect::Look(_))),
        "прокрутка не двигает ручки"
    );
}

#[test]
fn rows_scrolled_out_of_the_window_are_neither_drawn_nor_touchable() {
    let mut rig = portrait(look_facts());
    scroll_list_to_end(&mut rig);
    let list = rig.frame().clone();
    assert_eq!(
        text_count_of(&list, "CONTRAST", TextRoleKind::CardTitle),
        0,
        "верхняя строка уехала из окна и не рисуется"
    );
    // Строка Contrast уехала вверх под шапку: касание в её прежней линии (под шапкой) — не по ней.
    let window = center(&rig, "calib_knobs_window", 0);
    let contrast_ruler = center(&rig, "calib_ruler", 0);
    assert!(
        contrast_ruler.y < window.y - 100.0,
        "узел строки действительно уехал выше окна"
    );
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, contrast_ruler, t);
    rig.touch(
        TouchPhase::Moved,
        Point::new(contrast_ruler.x - 60.0, contrast_ruler.y),
        t + FRAME,
    );
    rig.touch(
        TouchPhase::Moved,
        Point::new(contrast_ruler.x - 80.0, contrast_ruler.y),
        t + 2 * FRAME,
    );
    rig.touch(
        TouchPhase::Ended,
        Point::new(contrast_ruler.x - 80.0, contrast_ruler.y),
        t + 3 * FRAME,
    );
    rig.now_ms = t + 4 * FRAME;
    rig.run(300);
    assert!(
        set_looks(&rig.effects()).is_empty(),
        "уехавшая из окна строка пальцем не берётся"
    );
}

#[test]
fn swiping_the_scope_zone_switches_the_instrument_in_a_line_without_wrapping() {
    let mut rig = portrait(look_facts());
    let scope = center(&rig, "calib_scope", 0);
    assert_eq!(rig.screen.state().calibration().scope_kind(), ScopeKind::Waveform);
    drag_eye(&mut rig, scope, Vector::new(-80.0, 0.0));
    // Следом за RGB WAVEFORM — страница LUMA WAVEFORM (решение 321).
    assert_eq!(rig.screen.state().calibration().scope_kind(), ScopeKind::LumaWaveform);
    let asked: Vec<_> = rig
        .effects()
        .into_iter()
        .filter_map(|effect| match effect {
            Effect::Scopes(ScopeIntent::Show(request)) => Some(request.kind),
            _ => None,
        })
        .collect();
    assert_eq!(
        asked,
        vec![ScopeKind::LumaWaveform],
        "рендеру ушёл запрос нового прибора"
    );
    drag_eye(&mut rig, scope, Vector::new(-80.0, 0.0));
    assert_eq!(rig.screen.state().calibration().scope_kind(), ScopeKind::RgbParade);
    drag_eye(&mut rig, scope, Vector::new(-80.0, 0.0));
    assert_eq!(rig.screen.state().calibration().scope_kind(), ScopeKind::Vectorscope);
    drag_eye(&mut rig, scope, Vector::new(-80.0, 0.0));
    assert_eq!(
        rig.screen.state().calibration().scope_kind(),
        ScopeKind::Vectorscope,
        "за краем ряда — стоит"
    );
    drag_eye(&mut rig, scope, Vector::new(80.0, 0.0));
    assert_eq!(
        rig.screen.state().calibration().scope_kind(),
        ScopeKind::RgbParade,
        "вправо — назад"
    );
    // Короткий сдвиг прибор не меняет.
    drag_eye(&mut rig, scope, Vector::new(-15.0, 0.0));
    assert_eq!(rig.screen.state().calibration().scope_kind(), ScopeKind::RgbParade);
}

#[test]
fn a_changed_band_set_rebuilds_the_dots() {
    let mut rig = portrait(look_facts());
    scroll_list_to_end(&mut rig);
    assert_eq!(rig.screen.node_centers("calib_band_dot").len(), 3);
    let mut more = look_facts();
    more.bands.push(look_contract::BandFacts {
        id: HueBandId::new("violet").unwrap(),
        swatch: look_contract::BandSwatch { srgb: [120, 40, 200] },
    });
    rig.screen.handle_action(ScreenAction::LookFactsChanged(Box::new(more)));
    rig.run(300);
    assert_eq!(
        rig.screen.node_centers("calib_band_dot").len(),
        4,
        "точка новой полосы появилась"
    );
}

#[test]
fn a_band_ruler_keeps_the_band_it_took_at_the_start_of_the_drag() {
    let mut rig = portrait(look_facts());
    scroll_list_to_end(&mut rig);
    let hue = center(&rig, "calib_ruler", 8);
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, hue, start);
    for step in 1..=10 {
        rig.touch(
            TouchPhase::Moved,
            Point::new(hue.x - 8.0 * f64::from(step), hue.y),
            start + i64::from(step) * FRAME,
        );
    }
    rig.now_ms = start + 11 * FRAME;
    rig.frame();
    // Посреди жеста выбрана другая полоса (другим пальцем или программно): жест остаётся за прежней.
    rig.screen.handle_action(ScreenAction::BandSelected(band("green")));
    for step in 11..=24 {
        rig.touch(
            TouchPhase::Moved,
            Point::new(hue.x - 8.0 * f64::from(step), hue.y),
            start + i64::from(step) * FRAME,
        );
    }
    rig.touch(
        TouchPhase::Ended,
        Point::new(hue.x - 8.0 * 24.0, hue.y),
        start + 26 * FRAME,
    );
    rig.now_ms = start + 27 * FRAME;
    rig.run(600);
    let sent = set_looks(&rig.effects());
    assert!(!sent.is_empty(), "палец тянул линейку полосы");
    let touched: Vec<_> = sent
        .iter()
        .map(|s| !s.bands.for_band(&band("red")).is_neutral())
        .collect();
    assert!(
        touched.iter().any(|t| *t),
        "значения ушли полосе red, которую взял палец: {sent:?}"
    );
    assert!(
        sent.iter().all(|s| s.bands.for_band(&band("green")).is_neutral()),
        "новая выбранная полоса green не получила чужих правок"
    );
}

#[test]
fn reset_and_dot_taps_leave_no_unhandled_gestures_behind() {
    let facts = edited(|s| s.with_knob(GlobalKnob::Contrast, knob(0.3)));
    let mut rig = portrait(facts);
    let reset = center(&rig, "calib_reset", 0);
    rig.tap(reset);
    rig.run(100);
    let base_reset = center(&rig, "calib_base_reset", 0);
    rig.tap(base_reset);
    rig.run(100);
    assert_eq!(
        rig.screen.diagnostics().unhandled_gestures,
        0,
        "стражи тапа кнопок раздела гасятся арбитражем, а не считаются ничьими"
    );
}
