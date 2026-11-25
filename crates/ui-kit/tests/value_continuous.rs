#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Непрерывные оси: Кельвины по майредам и линейная со знаком. Значение ↔ положение без округления, больше
//! Кельвинов — дальше по шкале, риски на абсолютной сетке, ошибки не меняют модель.

use ui_kit::value::{
    ContinuousLinear, Emphasis, LinearLimits, MiredKelvin, Quantization, RulerAxis, ValueError, ValueModel,
};

const KELVIN: LinearLimits = LinearLimits {
    min: 2000.0,
    max: 10000.0,
};

fn mired() -> MiredKelvin {
    MiredKelvin::new(5.0, KELVIN).unwrap()
}

#[test]
fn mired_axis_is_uniform_in_mired_and_grows_with_kelvin() {
    let model = mired();
    assert_eq!(model.quantization(), Quantization::Continuous);
    // Положение = −майреды: 3125 К = 320 майред, 5000 К = 200 майред.
    assert!((model.position_of(3125.0).unwrap() + 320.0).abs() < 1e-9);
    assert!((model.position_of(5000.0).unwrap() + 200.0).abs() < 1e-9);
    // Больше Кельвинов — больше положение (вправо по шкале).
    let mut previous = f64::NEG_INFINITY;
    for kelvin in [2000.0, 2700.0, 3200.0, 5600.0, 6500.0, 10_000.0] {
        let position = model.position_of(kelvin).unwrap();
        assert!(position > previous, "{kelvin} К: {position} не больше {previous}");
        previous = position;
    }
    // Равные сдвиги по шкале — равные сдвиги в майредах, а не в Кельвинах.
    let low = model.value_at(-400.0);
    let step_low = model.value_at(-390.0) - low;
    let high = model.value_at(-150.0);
    let step_high = model.value_at(-140.0) - high;
    assert!(
        step_high > step_low * 5.0,
        "в холодной части 10 майред — много Кельвинов: {step_high} против {step_low}"
    );
    assert_eq!(model.position_range(), (-500.0, -100.0));
}

#[test]
fn mired_value_and_position_round_trip_without_rounding() {
    let model = mired();
    for kelvin in [2000.0, 2437.3, 3140.0, 3141.7, 5600.5, 9999.0, 10_000.0] {
        let position = model.position_of(kelvin).unwrap();
        let back = model.value_at(position);
        assert!((back - kelvin).abs() < 1e-9 * kelvin, "{kelvin} → {position} → {back}");
    }
}

#[test]
fn mired_clamps_to_limits_and_rejects_values_without_mired() {
    let model = mired();
    assert_eq!(model.clamp_value(500.0).unwrap(), 2000.0);
    assert_eq!(model.clamp_value(50_000.0).unwrap(), 10_000.0);
    assert_eq!(model.value_at(-10_000.0), 2000.0, "положение за краем — предел");
    assert_eq!(model.value_at(-1.0), 10_000.0);
    assert_eq!(model.value_at(f64::NAN), 2000.0, "не число — нижняя граница");
    assert!(matches!(model.clamp_value(f64::NAN), Err(ValueError::NonFinite(_))));
    assert!(matches!(
        model.clamp_value(0.0),
        Err(ValueError::InvalidSpec { name: "value", .. })
    ));
    assert!(matches!(
        model.clamp_value(-100.0),
        Err(ValueError::InvalidSpec { name: "value", .. })
    ));
}

#[test]
fn mired_marks_sit_on_the_absolute_grid_and_survive_a_limits_change() {
    let mut model = mired();
    let marks = model.marks();
    // От 100 до 500 майред с шагом 5: 81 риска, по возрастанию положения, все обычные.
    assert_eq!(marks.len(), 81);
    assert_eq!(marks.first().unwrap().position, -500.0);
    assert_eq!(marks.last().unwrap().position, -100.0);
    assert!(
        marks
            .windows(2)
            .all(|pair| (pair[1].position - pair[0].position - 5.0).abs() < 1e-9)
    );
    assert!(
        marks.iter().all(|mark| mark.emphasis == Emphasis::Tick),
        "опорных рисок (магнитов) нет"
    );
    // Другие пределы: риски остаются на тех же местах сетки, а не едут за краем.
    model
        .set_limits(LinearLimits {
            min: 2200.0,
            max: 6000.0,
        })
        .unwrap();
    let narrowed = model.marks();
    assert!(
        narrowed
            .iter()
            .all(|mark| (mark.position / 5.0 - (mark.position / 5.0).round()).abs() < 1e-9)
    );
    assert_eq!(
        narrowed.first().unwrap().position,
        -450.0,
        "1e6/2200 = 454,5 → первая риска у −450"
    );
    assert_eq!(
        narrowed.last().unwrap().position,
        -170.0,
        "1e6/6000 = 166,7 → последняя риска не выше −166,7: −170"
    );
    assert_eq!(model.tick_step_units(), 5.0);
}

#[test]
fn mired_rejects_bad_specs_and_failed_limits_change_keeps_the_model() {
    assert!(matches!(
        MiredKelvin::new(0.0, KELVIN),
        Err(ValueError::InvalidSpec {
            name: "tick_spacing",
            ..
        })
    ));
    assert!(matches!(
        MiredKelvin::new(5.0, LinearLimits { min: 0.0, max: 100.0 }),
        Err(ValueError::InvalidLimits { .. })
    ));
    assert!(matches!(
        MiredKelvin::new(
            5.0,
            LinearLimits {
                min: 5000.0,
                max: 5000.0
            }
        ),
        Err(ValueError::InvalidLimits { .. })
    ));
    assert!(matches!(
        MiredKelvin::with_max_marks(5.0, KELVIN, 10),
        Err(ValueError::TooManyMarks { .. })
    ));
    let mut model = mired();
    assert!(
        model
            .set_limits(LinearLimits {
                min: 9000.0,
                max: 3000.0
            })
            .is_err()
    );
    assert_eq!(model.limits(), &KELVIN, "пределы прежние после отказа");
    assert_eq!(model.marks().len(), 81);
}

#[test]
fn tint_axis_is_linear_signed_and_unrounded() {
    let model = ContinuousLinear::new(5.0, LinearLimits { min: -50.0, max: 50.0 }).unwrap();
    assert_eq!(model.quantization(), Quantization::Continuous);
    assert_eq!(model.position_range(), (-50.0, 50.0));
    assert_eq!(model.position_of(-12.3).unwrap(), -12.3, "без округления к шагу");
    assert_eq!(model.value_at(3.5), 3.5);
    assert_eq!(model.value_at(80.0), 50.0);
    assert_eq!(model.value_at(-80.0), -50.0);
    assert_eq!(model.clamp_value(-70.0).unwrap(), -50.0);
    assert!(matches!(
        model.clamp_value(f64::INFINITY),
        Err(ValueError::NonFinite(_))
    ));
    let marks = model.marks();
    assert_eq!(marks.len(), 21, "от −50 до +50 через 5");
    assert!(
        marks.iter().all(|mark| mark.emphasis == Emphasis::Tick),
        "нуль тоже обычная риска — магнита нет"
    );
    assert!(marks.iter().any(|mark| mark.position == 0.0));
    assert_eq!(model.tick_step_units(), 5.0);
}

#[test]
fn tint_axis_rejects_bad_specs_and_keeps_state_on_failure() {
    assert!(matches!(
        ContinuousLinear::new(-1.0, LinearLimits { min: -1.0, max: 1.0 }),
        Err(ValueError::InvalidSpec { .. })
    ));
    assert!(matches!(
        ContinuousLinear::new(5.0, LinearLimits { min: 1.0, max: 1.0 }),
        Err(ValueError::InvalidLimits { .. })
    ));
    assert!(matches!(
        ContinuousLinear::with_max_marks(1.0, LinearLimits { min: 0.0, max: 1000.0 }, 100),
        Err(ValueError::TooManyMarks { .. })
    ));
    let mut model = ContinuousLinear::new(5.0, LinearLimits { min: -50.0, max: 50.0 }).unwrap();
    assert!(
        model
            .set_limits(LinearLimits {
                min: f64::NAN,
                max: 1.0
            })
            .is_err()
    );
    assert_eq!(model.limits(), &LinearLimits { min: -50.0, max: 50.0 });
}

#[test]
fn the_public_mired_formula_is_the_same_one_the_axis_uses() {
    use ui_kit::value::mired_of_kelvin;
    assert!((mired_of_kelvin(3125.0) - 320.0).abs() < 1e-9);
    for kelvin in [2000.0, 3200.0, 6500.0, 10_000.0] {
        // Положение оси — те же майреды со знаком минус.
        assert!((mired().position_of(kelvin).unwrap() + mired_of_kelvin(kelvin)).abs() < 1e-9);
    }
}
