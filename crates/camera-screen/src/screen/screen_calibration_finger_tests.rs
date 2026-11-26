//! Палец на линейках ручек раздела калибровки (решения 234, 251): шкала −200…+200 с шагом 1; путь пальца на единицу —
//! из конфига экрана (`knob_full_travel_pt` / (2·`knob_units_per_full_scale`), по умолчанию 1 pt; тесты берут
//! число оттуда, а не из литерала).
//!
//! Проверяется то, что делает владелец: палец ставит любое целое ровно; значение идёт от точки касания, а не от
//! точки распознавания пана; быстрый взмах едет дальше медленного ведения; вертикальная прокрутка списка не
//! срывается в ручку; значение пресета отмечено на линейке; кольцо у точки полосы с правками; позиция списка
//! шкал переживает переключение SCALES ↔ WHEELS.

use look_contract::{BandAxis, ColorWheel, GlobalKnob, LookControl, LookFacts, LookKnob, WheelZone};
use shell_contract::haptics::HapticPattern;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::shape::ShapeGeometry;

use super::screen_calibration_bands_tests::scroll_list_to_end;
use super::screen_calibration_tests::{center, drag_eye, edited, portrait, set_looks};
use super::screen_calibration_wheels_tests::{intents, wheels};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::calibration::ControlViewKind;
use crate::effect::Effect;
use crate::look_test_support::{band, knob, look_facts};

/// Единиц шкалы на значение 1,0.
const UNITS_PER_ONE: f32 = 200.0;
/// Наибольший путь пальца за одну протяжку, pt: палец не уходит за край экрана (в колонке ≈ 200 pt).
const LONGEST_SWIPE_PT: f64 = 120.0;
const FRAME: i64 = 16;

/// Значение, которое ровно равно `units / 200` в `f32` (так же, как литерал `0.15` в файле пресета).
fn exact(units: i32) -> LookKnob {
    knob(units as f32 / UNITS_PER_ONE)
}

/// pt пальца на единицу шкалы (из конфига экрана, решение 251).
fn points_per_unit() -> f64 {
    knob_finger_pt(1.0)
}

/// Наибольшая протяжка за раз, единиц: `LONGEST_SWIPE_PT` пальца.
fn longest_swipe_units() -> i32 {
    (LONGEST_SWIPE_PT / points_per_unit()) as i32
}

fn units_of(value: LookKnob) -> i32 {
    (value.get() * UNITS_PER_ONE).round() as i32
}

/// Последнее значение ручки `knob_id` среди ушедших намерений; `None` — палец ручку не двигал.
fn last_sent(effects: &[Effect], knob_id: GlobalKnob) -> Option<LookKnob> {
    set_looks(effects).last().map(|settings| settings.knob(knob_id))
}

/// Присылает экрану факты, где ручка стоит на `value` (так делает библиотека пресетов после намерения).
fn library_applies(rig: &mut Rig, facts: &LookFacts, knob_id: GlobalKnob, value: LookKnob) -> LookFacts {
    let mut next = facts.clone();
    next.current.settings = next.current.settings.with_knob(knob_id, value);
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(next.clone())));
    rig.run(300);
    next
}

/// Протяжка на `finger_dx` pt по горизонтали, которая обязательно распознаётся как пан: если путь короче 12 pt
/// (порог распознавания 5 pt), палец сначала уходит за порог в ту же сторону и возвращается к цели, как на телефоне,
/// когда нужна единица-две.
fn drag_via_recognition(rig: &mut Rig, start: Point, finger_dx: f64) {
    const MIN_OUTBOUND_PT: f64 = 12.0;
    if finger_dx.abs() >= MIN_OUTBOUND_PT {
        drag_eye(rig, start, Vector::new(finger_dx, 0.0));
        return;
    }
    let outbound = MIN_OUTBOUND_PT * if finger_dx == 0.0 { 1.0 } else { finger_dx.signum() };
    let t = rig.now_ms;
    rig.touch(TouchPhase::Began, start, t);
    for (index, dx) in [outbound * 0.5, outbound, outbound * 0.5, finger_dx]
        .into_iter()
        .enumerate()
    {
        rig.touch(
            TouchPhase::Moved,
            Point::new(start.x + dx, start.y),
            t + (index as i64 + 1) * FRAME,
        );
    }
    for hold in 5..=16 {
        rig.touch(
            TouchPhase::Moved,
            Point::new(start.x + finger_dx, start.y),
            t + hold * FRAME,
        );
    }
    rig.touch(
        TouchPhase::Ended,
        Point::new(start.x + finger_dx, start.y),
        t + 17 * FRAME,
    );
    rig.now_ms = t + 17 * FRAME + 10;
    rig.frame();
    rig.run(600);
}

/// Ставит ручку строки `slot` пальцем на `target_units`: несколькими протяжками, как на телефоне (палец не уходит за
/// край экрана); после каждой «библиотека» присылает факты с новым значением. Возвращает последнее значение пальца.
fn set_knob_by_finger(
    rig: &mut Rig,
    facts: &LookFacts,
    (slot, knob_id): (usize, GlobalKnob),
    target_units: i32,
) -> LookKnob {
    let mut facts = facts.clone();
    let mut current = units_of(facts.current.settings.knob(knob_id));
    let mut last = facts.current.settings.knob(knob_id);
    for _ in 0..6 {
        if current == target_units {
            return last;
        }
        let segment = (target_units - current).clamp(-longest_swipe_units(), longest_swipe_units());
        // Палец влево — значение растёт. Начинаем с той стороны, куда палец идёт, чтобы хватило места на экране.
        let finger_dx = -knob_finger_pt(f64::from(segment));
        let ruler = center(rig, "calib_ruler", slot);
        let start = Point::new(ruler.x - finger_dx.signum() * 70.0, ruler.y);
        drag_via_recognition(rig, start, finger_dx);
        let Some(value) = last_sent(&rig.effects(), knob_id) else {
            panic!("палец тянул ручку {knob_id:?} на {segment}, но намерения не ушло");
        };
        last = value;
        current = units_of(value);
        facts = library_applies(rig, &facts, knob_id, value);
    }
    assert_eq!(current, target_units, "за шесть протяжек не дошли до цели");
    last
}

// ----- любое целое ставится пальцем ---------------------------------------------------------------------------------

#[test]
fn the_finger_sets_any_integer_exactly_n_over_200() {
    for target in [-200, -74, -1, 1, 30, 200] {
        let facts = look_facts();
        let mut rig = portrait(facts.clone());
        let value = set_knob_by_finger(&mut rig, &facts, (0, GlobalKnob::Contrast), target);
        assert_eq!(
            value.get().to_bits(),
            exact(target).get().to_bits(),
            "{target}: палец поставил {} вместо ровно {target}/200",
            value.get()
        );
    }
}

#[test]
fn the_finger_sets_plus_30_equal_to_a_preset_value_of_015_and_one_step_around_it() {
    // Пресет хранит 0.15 литералом (= +30); палец обязан поставить то же значение бит в бит, иначе «↻» горела бы
    // вечно.
    let mut facts = look_facts();
    facts.preset.settings = facts.preset.settings.with_knob(GlobalKnob::Contrast, knob(0.15));
    let mut rig = portrait(facts.clone());
    for target in [29, 31, 30] {
        let value = set_knob_by_finger(&mut rig, &facts, (0, GlobalKnob::Contrast), target);
        assert_eq!(value.get().to_bits(), exact(target).get().to_bits(), "{target}");
        facts = library_applies(&mut rig, &facts, GlobalKnob::Contrast, value);
        let reset_wanted = facts.control_differs(&LookControl::Knob(GlobalKnob::Contrast));
        assert_eq!(
            reset_wanted,
            target != 30,
            "{target}: «отличается от пресета» верно только вне +30 (метка пресета не магнитит и не тянет значение)"
        );
    }
}

#[test]
fn the_wheel_ring_ruler_is_as_exact_as_the_knob_rulers() {
    for target in [-200, -74, 1, 30, 200] {
        let facts = look_facts();
        let mut rig = wheels(facts.clone(), DeviceOrientation::Portrait);
        let mut current = 0;
        let mut facts = facts;
        let mut last = None;
        for _ in 0..6 {
            if current == target {
                break;
            }
            let segment = (target - current).clamp(-longest_swipe_units(), longest_swipe_units());
            let finger_dx = -knob_finger_pt(f64::from(segment));
            let ruler = center(&rig, "calib_ruler", 0);
            let start = Point::new(ruler.x - finger_dx.signum() * 70.0, ruler.y);
            drag_via_recognition(&mut rig, start, finger_dx);
            let ring = intents(&rig.effects())
                .into_iter()
                .filter_map(|intent| match intent {
                    look_contract::LookIntent::SetWheelRing {
                        zone: WheelZone::Lift,
                        ring,
                    } => Some(ring),
                    _ => None,
                })
                .next_back()
                .expect("палец тянул кольцо: SetWheelRing ушёл");
            current = units_of(ring);
            last = Some(ring);
            let wheel = facts.current.settings.wheels.wheel(WheelZone::Lift);
            facts.current.settings.wheels = facts
                .current
                .settings
                .wheels
                .with_wheel(WheelZone::Lift, ColorWheel { ring, ..wheel });
            rig.screen
                .handle_action(ScreenAction::LookFactsChanged(Box::new(facts.clone())));
            rig.run(300);
        }
        let ring = last.expect("кольцо двигалось");
        assert_eq!(
            ring.get().to_bits(),
            exact(target).get().to_bits(),
            "кольцо {target}: {}",
            ring.get()
        );
    }
}

// ----- начало движения и инерция ----------------------------------------------------------------------------------

#[test]
fn the_value_follows_the_finger_from_the_touch_point_with_no_dead_start() {
    // Ручка стоит на +30 (не на нуле: у нуля палец держит магнит, решение 284) и идёт к +50, не пересекая ноль.
    let mut start = look_facts();
    start.current.settings = start.current.settings.with_knob(GlobalKnob::Contrast, exact(30));
    start.preset = start.current.clone();
    let mut rig = portrait(start);
    let ruler = center(&rig, "calib_ruler", 0);
    let finger_dx = knob_finger_pt(20.0);
    drag_eye(
        &mut rig,
        Point::new(ruler.x + finger_dx, ruler.y),
        Vector::new(-finger_dx, 0.0),
    );
    let sent = set_looks(&rig.effects());
    let values: Vec<f32> = sent
        .iter()
        .map(|settings| settings.knob(GlobalKnob::Contrast).get())
        .collect();
    assert_eq!(
        values.last().map(|value| value.to_bits()),
        Some(exact(50).get().to_bits()),
        "путь на 20 единиц от касания — ровно +50: ни одна единица не потеряна на пороге пана: {values:?}"
    );
    let first = values.first().copied().expect("палец двигал ручку");
    assert!(
        (0.17..=0.19).contains(&first),
        "в момент распознавания (порог 5 pt) значение уже догнало палец (30 + 4…8 единиц): {first}"
    );
}

/// Взмах: палец идёт влево по `per_frame_pt` за кадр `frames` кадров и отрывается без остановки (скорость
/// отпускания ≈ `per_frame_pt / 0,016` pt/с). Возвращает последнее значение ручки после того, как всё успокоилось.
fn swipe_left(rig: &mut Rig, from: Point, (per_frame_pt, frames): (f64, i64)) -> Option<LookKnob> {
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, from, start);
    let mut point = from;
    for frame in 1..=frames {
        point = Point::new(from.x - per_frame_pt * frame as f64, from.y);
        rig.touch(TouchPhase::Moved, point, start + frame * FRAME);
    }
    rig.touch(TouchPhase::Ended, point, start + frames * FRAME + 1);
    rig.now_ms = start + frames * FRAME + 10;
    rig.frame();
    rig.run(1500);
    last_sent(&rig.effects(), GlobalKnob::Contrast)
}

#[test]
fn the_faster_the_swipe_the_further_the_knob_travels_beyond_the_finger() {
    // Взмахи 125, 250 и 375 pt/с по 12 кадров: палец проходит 24, 48, 72 pt, остальное доезжает инерция; чем
    // быстрее отпущен палец, тем дальше значение, и оно всегда целое.
    let mut results = Vec::new();
    for per_frame_pt in [2.0, 4.0, 6.0] {
        let mut rig = portrait(look_facts());
        let ruler = center(&rig, "calib_ruler", 0);
        let finger_units = (per_frame_pt * 12.0 / points_per_unit()) as i32;
        let value =
            swipe_left(&mut rig, Point::new(ruler.x + 80.0, ruler.y), (per_frame_pt, 12)).expect("взмах двигал ручку");
        assert_eq!(
            value.get().to_bits(),
            exact(units_of(value)).get().to_bits(),
            "остановилась на целой единице"
        );
        assert!(
            units_of(value) > finger_units + 10,
            "{per_frame_pt} pt за кадр: после отрыва ручка поехала дальше пальца ({finger_units}): {}",
            units_of(value)
        );
        results.push(units_of(value));
    }
    assert!(
        results.windows(2).all(|pair| pair[0] < pair[1]),
        "быстрее — дальше: {results:?}"
    );
}

#[test]
fn one_or_two_fast_swipes_carry_the_knob_from_one_edge_to_the_other() {
    // Взмахи ≈ 1000 и ≈ 1500 pt/с от −200: до +200 за один-два взмаха (весь ход — 400 единиц, 400 pt пальца).
    for per_frame_pt in [16.0, 24.0] {
        let mut start = look_facts();
        start.current.settings = start.current.settings.with_knob(GlobalKnob::Contrast, knob(-1.0));
        start.preset = start.current.clone();
        let mut rig = portrait(start.clone());
        let mut facts = start;
        let mut value = knob(-1.0);
        let mut swipes = 0;
        while units_of(value) < 200 && swipes < 2 {
            let ruler = center(&rig, "calib_ruler", 0);
            if let Some(next) = swipe_left(&mut rig, Point::new(ruler.x + 80.0, ruler.y), (per_frame_pt, 6)) {
                value = next;
                facts = library_applies(&mut rig, &facts, GlobalKnob::Contrast, value);
            }
            swipes += 1;
        }
        assert_eq!(
            units_of(value),
            200,
            "{} pt/с: от −200 до +200 за {swipes} взмаха(ов) не дошли: {}",
            per_frame_pt / 0.016,
            units_of(value)
        );
    }
}

#[test]
fn a_slow_drag_with_a_stop_leaves_the_value_exactly_under_the_finger() {
    let mut rig = portrait(look_facts());
    let ruler = center(&rig, "calib_ruler", 2);
    // drag_eye ведёт палец плавно и задерживается перед отрывом: скорость отпускания нулевая.
    let finger_dx = knob_finger_pt_from_zero(99.0);
    drag_eye(
        &mut rig,
        Point::new(ruler.x + 60.0, ruler.y),
        Vector::new(-finger_dx, 0.0),
    );
    let value = last_sent(&rig.effects(), GlobalKnob::Shadows).expect("палец двигал ручку");
    assert_eq!(
        value.get().to_bits(),
        exact(99).get().to_bits(),
        "путь на 99 единиц = +99 ровно, без доезда"
    );
    // Библиотека применила значение и прислала факты: линейка (она уже ехала назад к старым фактам) возвращается.
    library_applies(&mut rig, &look_facts(), GlobalKnob::Shadows, value);
    rig.run(1500);
    let position = rig.screen.calibration_row_position_steps(2).expect("строка Shadows");
    assert!((position - 99.0).abs() < 0.02, "и линейка стоит на 99: {position}");
}

// ----- арбитраж: список и ручка -----------------------------------------------------------------------------------

#[test]
fn vertical_and_steep_drags_that_start_on_a_ruler_scroll_the_list_not_the_knob() {
    // 90° (строго вертикально) и 60° от горизонтали: оба обязаны листать список; ручка не двигается.
    for (label, eye) in [
        ("вертикаль", Vector::new(0.0, -110.0)),
        ("60° от горизонтали", Vector::new(-40.0, -69.3)),
        ("60° вниз-вправо после прокрутки", Vector::new(40.0, -69.3)),
    ] {
        let mut rig = portrait(look_facts());
        let ruler = center(&rig, "calib_ruler", 1);
        drag_eye(&mut rig, ruler, eye);
        let offset = rig.screen.calibration_scroll_offset().expect("список есть");
        assert!(offset > 40.0, "{label}: список прокрутился ({offset})");
        assert!(set_looks(&rig.effects()).is_empty(), "{label}: ручка не сорвалась");
    }
}

#[test]
fn a_horizontal_and_a_shallow_drag_on_a_ruler_turn_the_knob_not_the_list() {
    // Строго горизонтально и 20° от горизонтали: крутят ручку, список стоит.
    let run_pt = knob_finger_pt(120.0);
    let shallow_rise_pt = run_pt * 20.0_f64.to_radians().tan();
    for (label, eye) in [
        ("горизонталь", Vector::new(-run_pt, 0.0)),
        ("20° от горизонтали", Vector::new(-run_pt, shallow_rise_pt)),
    ] {
        let mut rig = portrait(look_facts());
        let ruler = center(&rig, "calib_ruler", 1);
        drag_eye(&mut rig, Point::new(ruler.x + 80.0, ruler.y), eye);
        assert!(
            last_sent(&rig.effects(), GlobalKnob::Midtones).is_some_and(|value| units_of(value) >= 110),
            "{label}: ручка повернулась"
        );
        assert_eq!(
            rig.screen.calibration_scroll_offset(),
            Some(0.0),
            "{label}: список стоит"
        );
    }
}

// ----- метка значения пресета на линейке -------------------------------------------------------------------------

fn shape_items(rig: &mut Rig) -> Vec<ShapeItem> {
    rig.frame()
        .leaf_items()
        .filter_map(|item| match item {
            DrawItem::Shape(shape) => Some(shape.clone()),
            _ => None,
        })
        .collect()
}

/// Фигуры-круги без обводки диаметра `diameter_pt` (точки и метки).
fn discs_of(rig: &mut Rig, diameter_pt: f64) -> Vec<ShapeItem> {
    shape_items(rig)
        .into_iter()
        .filter(|shape| {
            shape.stroke.is_none()
                && matches!(shape.geometry, ShapeGeometry::Circle { diameter } if near(diameter, px(diameter_pt), 0.05))
        })
        .collect()
}

#[test]
fn the_preset_value_is_marked_on_the_ruler_only_while_the_knob_differs_from_it() {
    let marker_diameter = crate::calibration::CalibrationWidgetsConfig::default().preset_marker_diameter_pt;
    let mut same = portrait(look_facts());
    assert!(
        discs_of(&mut same, marker_diameter).is_empty(),
        "значение равно пресету: метки нет"
    );

    // Контраст +30 (0,15), пресет в нуле: метка в нуле, то есть на 30 единиц левее жёлтой отметки (дальше метка
    // уходит за видимое окно линейки).
    let mut rig = portrait(edited(|settings| settings.with_knob(GlobalKnob::Contrast, knob(0.15))));
    rig.run(1500);
    let markers = discs_of(&mut rig, marker_diameter);
    assert_eq!(markers.len(), 1, "у правленой ручки ровно одна метка пресета");
    let ruler = center(&rig, "calib_ruler", 0);
    let marker = &markers[0];
    assert!(
        near(marker.transform.tx, px(ruler.x - knob_finger_pt(30.0)), 1.5),
        "метка стоит в позиции значения пресета (0), на 30 единиц левее текущего +30: {} против {}",
        marker.transform.tx,
        px(ruler.x - knob_finger_pt(30.0))
    );
    assert!(
        marker.transform.ty > px(ruler.y),
        "метка под рядом рисок: {} ниже {}",
        marker.transform.ty,
        px(ruler.y)
    );
    assert!(set_looks(&rig.effects()).is_empty(), "метка не даёт действий");
}

// ----- BANDS: кольцо «есть правки» -------------------------------------------------------------------------------

/// Фигуры-кольца (обводка, без заливки) диаметра `diameter_pt`.
fn rings_of(rig: &mut Rig, diameter_pt: f64) -> Vec<ShapeItem> {
    shape_items(rig)
        .into_iter()
        .filter(|shape| {
            shape.stroke.is_some()
                && matches!(shape.geometry, ShapeGeometry::Circle { diameter } if near(diameter, px(diameter_pt), 0.05))
        })
        .collect()
}

fn with_band_edit(mut facts: LookFacts, band_id: &str, edited_on_top_of_preset: bool) -> LookFacts {
    let adjusted = facts
        .current
        .settings
        .bands
        .for_band(&band(band_id))
        .with_axis(BandAxis::Saturation, knob(-0.2));
    facts.current.settings.bands = facts.current.settings.bands.with_adjustment(adjusted);
    if !edited_on_top_of_preset {
        // Рабочий вид владельца: правки сохранены в пресете, относительно него различий нет.
        facts.preset = facts.current.clone();
    }
    facts
}

#[test]
fn a_band_with_edits_has_a_ring_even_when_the_edits_are_saved_in_the_preset() {
    let config = crate::calibration::CalibrationWidgetsConfig::default();
    let ring_diameter =
        config.band_dot_diameter_pt + 2.0 * config.band_edited_ring_gap_pt + config.band_edited_ring_width_pt;
    let marker_diameter = config.band_marker_diameter_pt;
    let count = |facts: LookFacts| {
        let mut rig = portrait(facts);
        scroll_list_to_end(&mut rig);
        (
            rings_of(&mut rig, ring_diameter).len(),
            discs_of(&mut rig, marker_diameter).len(),
        )
    };
    assert_eq!(count(look_facts()), (0, 0), "нейтральные полосы: ни кольца, ни метки");
    assert_eq!(
        count(with_band_edit(look_facts(), "sky", false)),
        (1, 0),
        "правки лежат в пресете: у точки кольцо «есть правки», метки «отличается от пресета» нет"
    );
    assert_eq!(
        count(with_band_edit(look_facts(), "sky", true)),
        (1, 1),
        "правки поверх пресета: и кольцо, и метка отличия"
    );
}

#[test]
fn the_edited_ring_sits_around_the_dot_of_the_edited_band_only() {
    let config = crate::calibration::CalibrationWidgetsConfig::default();
    let ring_diameter =
        config.band_dot_diameter_pt + 2.0 * config.band_edited_ring_gap_pt + config.band_edited_ring_width_pt;
    let mut rig = portrait(with_band_edit(look_facts(), "sky", false));
    scroll_list_to_end(&mut rig);
    let rings = rings_of(&mut rig, ring_diameter);
    let sky_dot = center(&rig, "calib_band_dot", 1);
    assert_eq!(rings.len(), 1);
    assert!(
        near(rings[0].transform.tx, px(sky_dot.x), 1.0) && near(rings[0].transform.ty, px(sky_dot.y), 1.0),
        "кольцо вокруг точки полосы sky (вторая): ({}, {}) против ({}, {})",
        rings[0].transform.tx,
        rings[0].transform.ty,
        px(sky_dot.x),
        px(sky_dot.y)
    );
}

// ----- прокрутка списка не сбрасывается при SCALES ↔ WHEELS -------------------------------------------------------

#[test]
fn the_scales_scroll_position_survives_switching_to_wheels_and_back() {
    let mut rig = portrait(look_facts());
    let ruler = center(&rig, "calib_ruler", 1);
    drag_eye(&mut rig, ruler, Vector::new(0.0, -150.0));
    let before = rig.screen.calibration_scroll_offset().expect("список есть");
    assert!(before > 100.0, "список прокручен: {before}");
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.run(400);
    assert_eq!(
        rig.screen.calibration_scroll_offset(),
        Some(0.0),
        "у колёс прокрутки нет"
    );
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Scales));
    rig.run(400);
    let after = rig.screen.calibration_scroll_offset().expect("список есть");
    assert!(
        (after - before).abs() < 0.5,
        "после WHEELS → SCALES список стоит на прежнем месте: было {before}, стало {after}"
    );
}

#[test]
fn a_list_that_was_never_scrolled_still_comes_back_at_the_top() {
    let mut rig = portrait(look_facts());
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.run(400);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Scales));
    rig.run(400);
    assert_eq!(rig.screen.calibration_scroll_offset(), Some(0.0));
}

// ----- щелчки отдачи ------------------------------------------------------------------------------------------------

/// Щелчки с метками времени (мс), если рисовать кадр после каждого шага пальца, как на телефоне (кадры по 16 мс).
fn clicks_of_a_frame_by_frame_swipe(
    rig: &mut Rig,
    from: Point,
    (per_frame_pt, frames): (f64, i64),
) -> Vec<(i64, HapticPattern)> {
    let mut clicks = Vec::new();
    let mut collect = |rig: &mut Rig, at_ms: i64| {
        for effect in rig.effects() {
            if let Effect::Haptic(pattern) = effect {
                clicks.push((at_ms, pattern));
            }
        }
    };
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, from, start);
    let mut point = from;
    for frame in 1..=frames {
        point = Point::new(from.x - per_frame_pt * frame as f64, from.y);
        let at = start + frame * FRAME;
        rig.touch(TouchPhase::Moved, point, at);
        rig.now_ms = at;
        rig.frame();
        collect(rig, at);
    }
    let release = start + frames * FRAME + 1;
    rig.touch(TouchPhase::Ended, point, release);
    for frame in 0..90 {
        let at = release + frame * FRAME;
        rig.now_ms = at;
        rig.frame();
        collect(rig, at);
    }
    clicks
}

#[test]
fn a_fast_swipe_clicks_at_most_every_30_ms_and_the_slow_drag_clicks_on_ticks_only() {
    // Взмах по риске за кадр (20 единиц за 16 мс — резкие ≈ 1250 pt/с при 1 pt на единицу): каждый кадр проходит
    // риску, но щелчки идут не чаще раза в 30 мс. Медленнее риска реже, чем раз в 30 мс, и дроссель не работает.
    let swipe_per_frame_pt = knob_finger_pt(20.0);
    let mut rig = portrait(look_facts());
    let ruler = center(&rig, "calib_ruler", 0);
    let clicks =
        clicks_of_a_frame_by_frame_swipe(&mut rig, Point::new(ruler.x + 80.0, ruler.y), (swipe_per_frame_pt, 6));
    assert!(clicks.len() >= 4, "щелчки есть: {clicks:?}");
    assert!(
        clicks.windows(2).all(|pair| pair[1].0 - pair[0].0 >= 30),
        "быстрый взмах не сыплет щелчки каждый кадр: {clicks:?}"
    );
    assert!(clicks.len() <= 8, "6 рисок на пути, а щелчков меньше: {}", clicks.len());

    // Медленное ведение (3 единицы за кадр): по щелчку на каждой риске (через 20 единиц), а не на каждой единице.
    let mut slow = portrait(look_facts());
    let ruler = center(&slow, "calib_ruler", 0);
    let from = Point::new(ruler.x + 80.0, ruler.y);
    let start = slow.now_ms;
    slow.touch(TouchPhase::Began, from, start);
    let mut played = 0;
    for frame in 1..=36_i64 {
        let at = start + frame * FRAME;
        slow.touch(
            TouchPhase::Moved,
            Point::new(from.x - knob_finger_pt(3.0) * frame as f64, from.y),
            at,
        );
        slow.now_ms = at;
        slow.frame();
        played += slow
            .effects()
            .iter()
            .filter(|effect| matches!(effect, Effect::Haptic(_)))
            .count();
    }
    // 108 pt пути за 36 кадров по 3 = 102 единицы (первые 6 pt держит магнит нуля): риски +20, +40, +60, +80, +100.
    assert_eq!(played, 5, "щелчок на каждой из пяти рисок, а не на 102 единицах");
}
