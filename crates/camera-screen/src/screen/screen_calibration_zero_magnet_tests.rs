//! Магнит нуля на линейках ручек раздела калибровки (решение 284), «палец в пикселях → значение и щелчки»:
//! медленная протяжка ручки через ноль держит значение нулём, потом ведёт его дальше без скачка и щёлкает нулём один раз.

use look_contract::{GlobalKnob, LookKnob};
use shell_contract::haptics::HapticPattern;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;
use ui_kit::tokens::{HapticEvent, UiKitTokens};

use super::screen_calibration_tests::{calibrating_with, center, set_looks};
use super::test_rig::*;
use crate::config::ScreenConfig;
use crate::effect::Effect;
use crate::look_test_support::{knob, look_facts};

/// Единиц шкалы на значение 1,0.
const UNITS_PER_ONE: f32 = 200.0;
/// Один кадр медленной протяжки идёт дольше интервала ограничителя щелчков, чтобы он не гасил щелчки теста.
/// Где ручка стоит до протяжки, единицы шкалы.
const START_UNITS: i32 = 10;
const SLOW_FRAME: i64 = 100;

fn units(value: LookKnob) -> i32 {
    (value.get() * UNITS_PER_ONE).round() as i32
}

/// Протяжка ручки CONTRAST пальцем вправо (значение уменьшается) по 1 pt за кадр, `steps` кадров, потом отпускание
/// без скорости. Возвращает значение пальца в каждом кадре (единицы шкалы; последнее известное, если в кадре
/// намерения не было) и все эффекты щелчков.
fn slide_right(rig: &mut Rig, start: Point, steps: i64) -> (Vec<i32>, Vec<HapticPattern>) {
    let mut values = Vec::new();
    let mut clicks = Vec::new();
    // Пока пан не распознан (порог 5 pt), намерений нет: значение — стартовое.
    let mut last = START_UNITS;
    let begin = rig.now_ms;
    rig.touch(TouchPhase::Began, start, begin);
    for frame in 1..=steps {
        let at = begin + frame * SLOW_FRAME;
        rig.touch(
            TouchPhase::Moved,
            Point::new(start.x + knob_finger_pt(1.0) * frame as f64, start.y),
            at,
        );
        rig.now_ms = at;
        rig.frame();
        let effects = rig.effects();
        if let Some(settings) = set_looks(&effects).last() {
            last = units(settings.knob(GlobalKnob::Contrast));
        }
        clicks.extend(effects.iter().filter_map(|effect| match effect {
            Effect::Haptic(pattern) => Some(*pattern),
            _ => None,
        }));
        values.push(last);
    }
    let end = Point::new(start.x + knob_finger_pt(1.0) * steps as f64, start.y);
    let release = begin + (steps + 1) * SLOW_FRAME;
    rig.touch(TouchPhase::Ended, end, release);
    rig.now_ms = release + 10;
    rig.run(600);
    (values, clicks)
}

#[test]
fn a_slow_drag_across_zero_holds_zero_then_continues_without_a_jump_and_clicks_once() {
    let mut facts = look_facts();
    facts.current.settings = facts
        .current
        .settings
        .with_knob(GlobalKnob::Contrast, knob(START_UNITS as f32 / UNITS_PER_ONE));
    facts.preset = facts.current.clone();
    let mut rig = calibrating_with(ScreenConfig::default(), facts, DeviceOrientation::Portrait);
    let ruler = center(&rig, "calib_ruler", 0);
    let zone = zero_magnet_half_width_pt() as i64;
    // 10 единиц до нуля, 2 × зона держания, ещё 8 единиц после.
    let steps = i64::from(START_UNITS) + 2 * zone + 8;
    let (values, clicks) = slide_right(&mut rig, Point::new(ruler.x - 40.0, ruler.y), steps);

    // Кадр k — индекс k − 1: ноль достигнут на кадре 10 (индекс 9) и держится 2 × зона кадров дальше.
    let held_from = START_UNITS as usize - 1;
    let held_until = held_from + 2 * zone as usize + 1;
    assert!(
        values[held_from..held_until].iter().all(|value| *value == 0),
        "на пути зоны значение ровно ноль: {values:?}"
    );
    assert_eq!(
        values[held_from - 2],
        2,
        "до нуля значение идёт за пальцем 1:1: {values:?}"
    );
    assert_eq!(
        *values.last().unwrap(),
        -8,
        "после зоны значение дальше идёт за пальцем: {values:?}"
    );
    // Кадры до распознавания пана (первые пять) не считаем: там значение один раз догоняет палец от точки касания.
    let largest_jump = values[5..]
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .max()
        .unwrap();
    assert!(
        largest_jump <= 1,
        "ни скачка при входе в ноль, ни при выходе: {values:?}"
    );

    let reference = UiKitTokens::default()
        .haptics
        .pattern_for(HapticEvent::ScaleReference)
        .unwrap();
    assert_eq!(
        clicks.iter().filter(|pattern| **pattern == reference).count(),
        1,
        "сильный щелчок нуля — один за проход: {clicks:?}"
    );
}

#[test]
fn releasing_the_finger_inside_the_zone_leaves_the_knob_exactly_on_zero() {
    let mut facts = look_facts();
    facts.current.settings = facts
        .current
        .settings
        .with_knob(GlobalKnob::Contrast, knob(START_UNITS as f32 / UNITS_PER_ONE));
    facts.preset = facts.current.clone();
    let mut rig = calibrating_with(ScreenConfig::default(), facts, DeviceOrientation::Portrait);
    let ruler = center(&rig, "calib_ruler", 0);
    // Путь 10 + 6: палец остановился посреди зоны.
    let (values, _) = slide_right(&mut rig, Point::new(ruler.x - 40.0, ruler.y), 16);
    assert_eq!(*values.last().unwrap(), 0);
    let settings = set_looks(&rig.effects());
    assert!(
        settings.iter().all(|s| units(s.knob(GlobalKnob::Contrast)) == 0),
        "после отпускания значение не уходит с нуля"
    );
}

#[test]
fn with_the_magnet_switched_off_in_config_the_value_follows_the_finger_through_zero() {
    let mut config = ScreenConfig::default();
    config.calibration.widgets.knob_zero_magnet_pt = 0.0;
    let mut facts = look_facts();
    facts.current.settings = facts
        .current
        .settings
        .with_knob(GlobalKnob::Contrast, knob(START_UNITS as f32 / UNITS_PER_ONE));
    facts.preset = facts.current.clone();
    let mut rig = calibrating_with(config, facts, DeviceOrientation::Portrait);
    let ruler = center(&rig, "calib_ruler", 0);
    let (values, _) = slide_right(&mut rig, Point::new(ruler.x - 40.0, ruler.y), 20);
    assert_eq!(values[9], 0);
    assert_eq!(values[10], -1, "магнит выключен: ноль не держится: {values:?}");
    assert_eq!(*values.last().unwrap(), -10);
}
