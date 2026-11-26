//! Упоры на шкалах теплоты и оттенка (решения 288, 293): рендер сообщает, где картинка перестаёт реагировать на
//! ручку, экран рисует на линейке риску-упор на `±доля · N` единиц, приглушает ход за упором и число, пока ручка
//! стоит за упором. Проверяется нарисованное: положение и яркость фигур и текста в кадре.

use look_contract::{GlobalKnob, KnobSideStops, KnobStop, KnobTravelStops, LookFacts, LookWhiteStops, StopFraction};
use ui_core::geometry::Point;
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::TextRoleKind;

use super::{center, edited, portrait, scroll_row_into_view};
use crate::action::ScreenAction;
use crate::calibration::CalibrationWidgetsConfig;
use crate::look_test_support::{knob, look_facts};
use crate::screen::test_rig::*;

/// Слот строки WARMTH среди глобальных ручек.
const WARMTH_SLOT: usize = 6;
/// Слот строки TINT среди глобальных ручек.
const TINT_SLOT: usize = 7;
/// Единиц шкалы на значение 1,0 (по умолчанию конфига).
const UNITS_PER_ONE: f64 = 200.0;

fn at(fraction: f32) -> KnobStop {
    KnobStop::At(StopFraction::new(fraction).expect("доля упора"))
}

/// Упоры только у оттенка; теплота без упоров.
fn tint_stops(minus: KnobStop, plus: KnobStop) -> LookWhiteStops {
    LookWhiteStops::Known(KnobTravelStops {
        warmth: KnobSideStops {
            minus: KnobStop::None,
            plus: KnobStop::None,
        },
        tint: KnobSideStops { minus, plus },
    })
}

/// Экран калибровки с фактами `facts`; строка `slot` прокручена в видимую часть, упоры `stops` сообщены, кадры прошли.
fn rig_with(facts: LookFacts, slot: usize, stops: LookWhiteStops) -> (Rig, Point) {
    let mut rig = portrait(facts);
    let ruler = scroll_row_into_view(&mut rig, slot);
    rig.screen.handle_action(ScreenAction::LookWhiteStopsChanged(stops));
    rig.run(1500);
    // Строка могла сдвинуться при пересборке: центр линейки берётся заново.
    let ruler_after = center(&rig, "calib_ruler", slot);
    assert!((ruler_after.y - ruler.y).abs() < 1.0, "строка стоит там же");
    (rig, ruler_after)
}

/// Фигуры-полоски на линии линейки `ruler` (центр по высоте), по возрастанию x.
fn bars_on_ruler(rig: &mut Rig, ruler: Point) -> Vec<ShapeItem> {
    let mut bars: Vec<ShapeItem> = rig
        .frame()
        .leaf_items()
        .filter_map(|item| match item {
            DrawItem::Shape(shape)
                if matches!(shape.geometry, ShapeGeometry::RoundedRect { .. })
                    && near(shape.transform.ty, px(ruler.y), px(2.0)) =>
            {
                Some(shape.clone())
            }
            _ => None,
        })
        .collect();
    bars.sort_by(|first, second| first.transform.tx.total_cmp(&second.transform.tx));
    bars
}

fn bar_size(shape: &ShapeItem) -> (f32, f32) {
    match shape.geometry {
        ShapeGeometry::RoundedRect { size, .. } => (size.width, size.height),
        _ => panic!("полоска — прямоугольник"),
    }
}

/// Риски-упоры линейки: толще и длиннее обычных (размеры из конфига).
fn stop_ticks(rig: &mut Rig, ruler: Point) -> Vec<ShapeItem> {
    let look = &CalibrationWidgetsConfig::default().knob_stops;
    bars_on_ruler(rig, ruler)
        .into_iter()
        .filter(|bar| {
            let (width, height) = bar_size(bar);
            near(width, px(look.tick_thickness_pt), 0.05) && near(height, px(look.tick_length_pt), 0.05)
        })
        .collect()
}

/// Яркость обычной риски линейки на `units` единиц от центра (`None` — такой риски нет).
fn tick_alpha_at(rig: &mut Rig, ruler: Point, units: f64) -> Option<f32> {
    let expected_x = px(ruler.x + units * knob_finger_pt(1.0));
    bars_on_ruler(rig, ruler)
        .into_iter()
        .filter(|bar| near(bar_size(bar).1, px(13.0), 0.05))
        .find(|bar| near(bar.transform.tx, expected_x, 1.0))
        .map(|bar| bar.fill.a)
}

fn value_alpha(rig: &mut Rig, label: &str) -> f32 {
    let list = rig.frame().clone();
    let runs = texts_of(&list, label, TextRoleKind::ScaleValue);
    assert_eq!(runs.len(), 1, "число «{label}» нарисовано один раз");
    runs[0].color.a
}

#[test]
fn a_known_tint_stop_draws_a_tick_at_fraction_times_units_and_dims_the_travel_beyond_it() {
    let (mut rig, ruler) = rig_with(look_facts(), TINT_SLOT, tint_stops(KnobStop::None, at(0.46)));
    let ticks = stop_ticks(&mut rig, ruler);
    assert_eq!(ticks.len(), 1, "риска-упор одна: у минусовой стороны упора нет");
    assert!(
        near(ticks[0].transform.tx, px(ruler.x + 92.0 * knob_finger_pt(1.0)), 1.0),
        "упор 0,46 стоит на +92 единицах (N = 200): {} против {}",
        ticks[0].transform.tx,
        px(ruler.x + 92.0 * knob_finger_pt(1.0))
    );
    let before_stop = tick_alpha_at(&mut rig, ruler, 80.0).expect("риска +80");
    let mirrored_before = tick_alpha_at(&mut rig, ruler, -80.0).expect("риска −80");
    assert_eq!(before_stop, mirrored_before, "до упора риски обычные");
}

#[test]
fn ticks_beyond_a_stop_are_dimmed_and_ticks_before_it_are_not() {
    // Упор на +60 (0,3): риска +80 уже за ним, +40 — до него.
    let (mut rig, ruler) = rig_with(look_facts(), TINT_SLOT, tint_stops(KnobStop::None, at(0.3)));
    let dim = CalibrationWidgetsConfig::default().knob_stops.beyond_tick_dim_factor;
    let beyond = tick_alpha_at(&mut rig, ruler, 80.0).expect("риска +80");
    let mirrored = tick_alpha_at(&mut rig, ruler, -80.0).expect("риска −80");
    assert!(
        (beyond - mirrored * dim).abs() < 1e-4,
        "риска за упором тусклее зеркальной в {dim} раза: {beyond} против {mirrored}"
    );
    assert_eq!(
        tick_alpha_at(&mut rig, ruler, 40.0),
        tick_alpha_at(&mut rig, ruler, -40.0),
        "до упора риски обычные"
    );
    assert_eq!(
        tick_alpha_at(&mut rig, ruler, 60.0),
        tick_alpha_at(&mut rig, ruler, -60.0),
        "риска на самом упоре не тускнеет"
    );
}

#[test]
fn a_minus_side_stop_mirrors_to_the_left_of_zero() {
    let (mut rig, ruler) = rig_with(look_facts(), TINT_SLOT, tint_stops(at(0.3), KnobStop::None));
    let ticks = stop_ticks(&mut rig, ruler);
    assert_eq!(ticks.len(), 1);
    assert!(
        near(ticks[0].transform.tx, px(ruler.x - 60.0 * knob_finger_pt(1.0)), 1.0),
        "упор минусовой стороны на −60 единицах"
    );
}

#[test]
fn a_dead_from_centre_side_dims_the_whole_side_and_draws_no_stop_tick() {
    let (mut rig, ruler) = rig_with(
        look_facts(),
        TINT_SLOT,
        tint_stops(KnobStop::None, KnobStop::DeadFromCentre),
    );
    assert!(
        stop_ticks(&mut rig, ruler).is_empty(),
        "у мёртвой с нуля стороны точки-упора нет"
    );
    let dim = CalibrationWidgetsConfig::default().knob_stops.beyond_tick_dim_factor;
    for units in [20.0, 60.0] {
        let plus = tick_alpha_at(&mut rig, ruler, units).expect("риска справа");
        let minus = tick_alpha_at(&mut rig, ruler, -units).expect("риска слева");
        assert!(
            (plus - minus * dim).abs() < 1e-4,
            "{units}: вся плюсовая сторона тусклая: {plus} против {minus}"
        );
    }
}

#[test]
fn unknown_and_not_applicable_stops_draw_nothing_and_leave_the_ruler_symmetric() {
    for stops in [LookWhiteStops::Unknown, LookWhiteStops::NotApplicable] {
        let (mut rig, ruler) = rig_with(look_facts(), TINT_SLOT, stops);
        assert!(stop_ticks(&mut rig, ruler).is_empty(), "{stops:?}: упоров нет");
        assert_eq!(
            tick_alpha_at(&mut rig, ruler, 100.0),
            tick_alpha_at(&mut rig, ruler, -100.0),
            "{stops:?}: тусклого хода нет"
        );
    }
}

#[test]
fn tint_stops_do_not_touch_the_warmth_row() {
    let (mut rig, ruler) = rig_with(look_facts(), WARMTH_SLOT, tint_stops(at(0.2), at(0.46)));
    assert!(
        stop_ticks(&mut rig, ruler).is_empty(),
        "у теплоты упоров нет, у оттенка они свои"
    );
    assert_eq!(
        tick_alpha_at(&mut rig, ruler, 100.0),
        tick_alpha_at(&mut rig, ruler, -100.0)
    );
}

#[test]
fn the_value_is_dim_only_while_the_knob_stands_beyond_the_stop() {
    let normal = 1.0;
    let dim = CalibrationWidgetsConfig::default().knob_stops.beyond_value_alpha;
    let stops = tint_stops(KnobStop::None, at(0.46));
    for (units, label, expected) in [
        (120, "+120", dim),
        (50, "+50", normal),
        (92, "+92", normal),
        (-120, "\u{2212}120", normal),
    ] {
        let facts = edited(|settings| settings.with_knob(GlobalKnob::Tint, knob(units as f32 / UNITS_PER_ONE as f32)));
        let (mut rig, _) = rig_with(facts, TINT_SLOT, stops);
        assert!(
            (value_alpha(&mut rig, label) - expected).abs() < 1e-4,
            "{label}: число должно быть с яркостью {expected}"
        );
    }
}

#[test]
fn a_dead_side_makes_any_nonzero_value_on_it_dim() {
    let facts = edited(|settings| settings.with_knob(GlobalKnob::Tint, knob(0.05)));
    let dead_plus = tint_stops(KnobStop::None, KnobStop::DeadFromCentre);
    let (mut rig, _) = rig_with(facts, TINT_SLOT, dead_plus);
    assert!(
        (value_alpha(&mut rig, "+10") - CalibrationWidgetsConfig::default().knob_stops.beyond_value_alpha).abs() < 1e-4
    );
}

#[test]
fn changing_stops_updates_the_picture_at_once() {
    let (mut rig, ruler) = rig_with(look_facts(), TINT_SLOT, LookWhiteStops::Unknown);
    assert!(stop_ticks(&mut rig, ruler).is_empty());
    let stops = tint_stops(KnobStop::None, at(0.46));
    rig.screen.handle_action(ScreenAction::LookWhiteStopsChanged(stops));
    rig.run(300);
    assert_eq!(stop_ticks(&mut rig, ruler).len(), 1, "новые упоры сразу на шкале");
    rig.screen
        .handle_action(ScreenAction::LookWhiteStopsChanged(LookWhiteStops::NotApplicable));
    rig.run(300);
    assert!(stop_ticks(&mut rig, ruler).is_empty(), "упоры пропали");
}

#[test]
fn a_stop_exactly_on_the_edge_of_the_scale_draws_without_error_and_dims_nothing() {
    let (mut rig, ruler) = rig_with(look_facts(), TINT_SLOT, tint_stops(at(1.0), at(1.0)));
    // Упор на самом краю шкалы: дальше него хода нет, затемнять нечего, обычные риски не тускнеют.
    assert_eq!(
        rig.screen.diagnostics().frame_failures,
        0,
        "кадры с упором 1,0 строятся"
    );
    let plus = tick_alpha_at(&mut rig, ruler, 80.0).expect("риска +80");
    let minus = tick_alpha_at(&mut rig, ruler, -80.0).expect("риска −80");
    assert_eq!(plus, minus);
    let normal = tick_alpha_at(&mut rig, ruler, 40.0).expect("риска +40");
    assert_eq!(plus, normal, "ход до упора на краю не затемнён");
}
