#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Защита от огромных пределов: число рисок ограничено явной ошибкой, ничего не аллоцируется
//! миллиардами и не насыщается молча при переводе в целые.

use ui_core::time::MotionPreference;
use ui_kit::drive::{DragOrigin, GrowthDirection, ScrollTransition};
use ui_kit::scale::{ScaleError, ScaleSpec, SnapScale};
use ui_kit::tokens::UiKitTokens;
use ui_kit::value::{
    DEFAULT_MAX_MARKS, LinearLimits, LogLimits, Logarithmic, LogarithmicSpec, QuantizedLinear, ReferenceValue,
    ValueError, ValueModel,
};

mod common;
use common::*;

fn linear(step: f64, min: f64, max: f64, max_marks: u32) -> Result<QuantizedLinear, ValueError> {
    QuantizedLinear::with_max_marks(step, ReferenceValue::None, LinearLimits { min, max }, max_marks)
}

#[test]
fn linear_budget_is_exact_at_the_boundary() {
    // 0…9 с шагом 1 — ровно 10 шагов.
    let model = linear(1.0, 0.0, 9.0, 10).unwrap();
    assert_eq!(model.marks().len(), 10);
    assert_eq!(
        linear(1.0, 0.0, 10.0, 10),
        Err(ValueError::TooManyMarks { count: 11.0, max: 10 }),
        "на шаг больше бюджета — ошибка с числом шагов"
    );
}

#[test]
fn absurd_linear_limits_are_an_error_not_billions_of_marks() {
    for (step, min, max) in [
        (1.0 / 3.0, -1e12, 1e12),
        (1e-12, 0.0, 1.0),
        (1.0, -f64::MAX, f64::MAX),
        (1.0, 1e300, 1e300),
    ] {
        let result = QuantizedLinear::new(step, ReferenceValue::None, LinearLimits { min, max });
        assert!(
            matches!(
                result,
                Err(ValueError::TooManyMarks { .. }) | Err(ValueError::InvalidLimits { .. })
            ),
            "step = {step}, [{min}, {max}]: {result:?}"
        );
    }
    // Один шаг далеко от нуля: бюджет не нарушен, но индекс не представим точно.
    assert!(matches!(
        QuantizedLinear::new(1.0, ReferenceValue::None, LinearLimits { min: 1e300, max: 1e300 }),
        Err(ValueError::InvalidLimits { .. })
    ));
    assert!(matches!(
        QuantizedLinear::new(
            1.0 / 3.0,
            ReferenceValue::At(1e300),
            LinearLimits { min: 0.0, max: 1.0 }
        ),
        Err(ValueError::InvalidSpec { name: "reference", .. })
    ));
}

#[test]
fn default_budget_allows_realistic_scales() {
    // ISO-подобная фокусная шкала: десятки тысяч шагов ещё допустимы.
    let model = QuantizedLinear::new(0.01, ReferenceValue::None, LinearLimits { min: 0.0, max: 500.0 }).unwrap();
    assert_eq!(model.marks().len(), 50_001);
    let budget = DEFAULT_MAX_MARKS;
    assert!(model.marks().len() < budget as usize);
}

#[test]
fn set_limits_over_budget_keeps_the_model() {
    let mut model = linear(1.0, 0.0, 9.0, 10).unwrap();
    let before = model.clone();
    assert!(matches!(
        model.set_limits(LinearLimits { min: 0.0, max: 1e9 }),
        Err(ValueError::TooManyMarks { .. })
    ));
    assert_eq!(model, before);
}

#[test]
fn logarithmic_tick_spacing_is_bounded_too() {
    let spec = |tick_spacing_units: f64| LogarithmicSpec {
        units_per_octave: 12.0,
        tick_spacing_units,
        anchors: vec![1.0],
    };
    let limits = LogLimits { min: 0.5, max: 2.0 };
    // 24 единицы по 3: 9 рисок — норма.
    assert_eq!(Logarithmic::new(spec(3.0), limits).unwrap().marks().len(), 9);
    assert!(matches!(
        Logarithmic::new(spec(1e-9), limits),
        Err(ValueError::TooManyMarks { .. })
    ));
    assert!(matches!(
        Logarithmic::with_max_marks(spec(3.0), limits, 5),
        Err(ValueError::TooManyMarks { max: 5, .. })
    ));
    // Огромные пределы тоже режутся на set_limits, модель остаётся прежней.
    let mut model = Logarithmic::new(spec(0.001), LogLimits { min: 1.0, max: 2.0 }).unwrap();
    let before = model.clone();
    assert!(matches!(
        model.set_limits(LogLimits {
            min: 1e-300,
            max: 1e300
        }),
        Err(ValueError::TooManyMarks { .. })
    ));
    assert_eq!(model, before);
}

/// На уровне шкалы: огромные пределы — явная ошибка модели, шкала не меняется.
#[test]
fn scale_rejects_absurd_limits_and_stays_usable() {
    let tokens = UiKitTokens::default();
    let spec = ScaleSpec {
        direction: GrowthDirection::FingerRight,
        drag_origin: DragOrigin::FromRecognition,
        ..ScaleSpec::default()
    };
    let mut scale = SnapScale::new(
        exposure_model(),
        1.0,
        spec,
        &env(&tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    let before = scale.snapshot(ms(0));
    let result = scale.set_limits(
        LinearLimits { min: -1e12, max: 1e12 },
        ScrollTransition::Immediate,
        ms(1),
    );
    assert!(matches!(
        result,
        Err(ScaleError::Value(ValueError::TooManyMarks { .. }))
    ));
    assert_eq!(scale.snapshot(ms(0)), before);
    assert_eq!(scale.marks().len(), 13);
}
