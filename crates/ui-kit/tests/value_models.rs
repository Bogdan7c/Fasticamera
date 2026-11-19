#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Модели значений: отображение значение ↔ положение, квантование, упоры, пределы снаружи.

use ui_kit::value::{
    AxisPair, Emphasis, LinearLimits, LogLimits, Logarithmic, LogarithmicSpec, PairValue, Quantization,
    QuantizedLinear, ReferenceValue, SeriesLimits, ValueError, ValueModel, ValueSeries,
};

fn exposure_model() -> QuantizedLinear {
    QuantizedLinear::new(1.0 / 3.0, ReferenceValue::At(0.0), LinearLimits { min: -2.0, max: 2.0 }).unwrap()
}

fn zoom_model() -> Logarithmic {
    Logarithmic::new(
        LogarithmicSpec {
            units_per_octave: 12.0,
            tick_spacing_units: 3.0,
            anchors: vec![0.5, 1.0, 2.0],
        },
        LogLimits { min: 0.5, max: 2.0 },
    )
    .unwrap()
}

#[test]
fn linear_limits_give_thirteen_steps_with_reference_at_zero() {
    let model = exposure_model();
    assert_eq!(model.position_range(), (-6.0, 6.0));
    let marks = model.marks();
    assert_eq!(marks.len(), 13, "от -2 до +2 по 1/3 — 13 шагов");
    let references: Vec<f64> = marks
        .iter()
        .filter(|mark| mark.emphasis == Emphasis::Reference)
        .map(|mark| mark.position)
        .collect();
    assert_eq!(references, vec![0.0], "опорный упор один — ноль");
    assert_eq!(model.quantization(), Quantization::Stepped);
}

#[test]
fn linear_position_and_value_are_inverse_for_every_step() {
    let model = exposure_model();
    for index in -6_i64..=6 {
        // Независимый эталон значения шага: деление, а не умножение, как в модели.
        let expected_value = index as f64 / 3.0;
        let value = model.value_at(index as f64);
        assert!((value - expected_value).abs() < 1e-15, "шаг {index}");
        assert_eq!(model.position_of(value).unwrap(), index as f64);
    }
}

#[test]
fn linear_quantizes_between_steps_to_the_nearest_one() {
    let model = exposure_model();
    assert!((model.value_at(1.49) - 1.0 / 3.0).abs() < 1e-15);
    assert!((model.value_at(1.51) - 2.0 / 3.0).abs() < 1e-15);
    // Значение между шагами округляется к ближайшему.
    assert!((model.clamp_value(0.30).unwrap() - 1.0 / 3.0).abs() < 1e-15);
}

#[test]
fn thousand_steps_back_and_forth_do_not_accumulate_error() {
    let model = QuantizedLinear::new(
        1.0 / 3.0,
        ReferenceValue::At(0.0),
        LinearLimits {
            min: -400.0,
            max: 400.0,
        },
    )
    .unwrap();
    // Идём по индексам вверх на 1000 шагов и обратно, как это делала бы шкала.
    let mut index = 0_i64;
    for _ in 0..1000 {
        index += 1;
    }
    let top = model.value_at(index as f64);
    assert!((top - 1000.0 / 3.0).abs() < 1e-12, "верх: {top}");
    for _ in 0..1000 {
        index -= 1;
    }
    assert_eq!(
        model.value_at(index as f64),
        0.0,
        "после возврата ровно ноль, без остатка"
    );
    // Для сравнения: суммирование дробей остаток даёт — модель так не считает.
    let mut accumulated = 0.0_f64;
    for _ in 0..1000 {
        accumulated += 1.0 / 3.0;
    }
    for _ in 0..1000 {
        accumulated -= 1.0 / 3.0;
    }
    assert_ne!(accumulated, 0.0, "эталон: накопление ошибки реально существует");
}

#[test]
fn linear_limits_come_from_outside_and_out_of_range_values_are_clamped() {
    let mut model = exposure_model();
    assert!((model.clamp_value(5.0).unwrap() - 2.0).abs() < 1e-15);
    assert!((model.clamp_value(-7.0).unwrap() + 2.0).abs() < 1e-15);
    // Другая камера: пределы сузились до ±1.
    model.set_limits(LinearLimits { min: -1.0, max: 1.0 }).unwrap();
    assert_eq!(model.position_range(), (-3.0, 3.0));
    // Выбранное ранее значение +2 теперь вне пределов: явное правило — ближайшая граница.
    assert!((model.clamp_value(2.0).unwrap() - 1.0).abs() < 1e-15);
    assert_eq!(model.position_of(2.0).unwrap(), 3.0);
    assert!(
        (model.value_at(100.0) - 1.0).abs() < 1e-15,
        "положение за краем даёт крайнее значение"
    );
    // Положение значения от пределов не зависит (абсолютное).
    assert_eq!(model.position_of(1.0 / 3.0).unwrap(), 1.0);
}

#[test]
fn linear_rejects_bad_input_and_keeps_old_limits_on_error() {
    assert!(matches!(
        QuantizedLinear::new(0.0, ReferenceValue::None, LinearLimits { min: 0.0, max: 1.0 }),
        Err(ValueError::InvalidSpec { name: "step", .. })
    ));
    assert!(matches!(
        QuantizedLinear::new(f64::NAN, ReferenceValue::None, LinearLimits { min: 0.0, max: 1.0 }),
        Err(ValueError::NonFinite(_))
    ));
    assert!(matches!(
        QuantizedLinear::new(1.0 / 3.0, ReferenceValue::At(0.1), LinearLimits { min: 0.0, max: 1.0 }),
        Err(ValueError::InvalidSpec { name: "reference", .. })
    ));
    // Между 0,1 и 0,2 нет ни одного шага 1/3.
    assert!(matches!(
        QuantizedLinear::new(1.0 / 3.0, ReferenceValue::None, LinearLimits { min: 0.1, max: 0.2 }),
        Err(ValueError::InvalidLimits { .. })
    ));
    let mut model = exposure_model();
    let before = model.clone();
    assert!(model.set_limits(LinearLimits { min: 0.1, max: 0.2 }).is_err());
    assert_eq!(model, before, "при ошибке модель не меняется");
    assert!(matches!(
        model.clamp_value(f64::INFINITY),
        Err(ValueError::NonFinite(_))
    ));
}

#[test]
fn log_one_x_sits_exactly_between_half_and_two() {
    let model = zoom_model();
    let low = model.position_of(0.5).unwrap();
    let mid = model.position_of(1.0).unwrap();
    let high = model.position_of(2.0).unwrap();
    assert_eq!(mid, 0.0);
    assert_eq!(mid - low, high - mid, "1× ровно посередине");
    assert_eq!(high - low, 24.0, "две октавы по 12 единиц");
    // Обратное отображение в опорных точках точное.
    assert_eq!(model.value_at(low), 0.5);
    assert_eq!(model.value_at(mid), 1.0);
    assert_eq!(model.value_at(high), 2.0);
    assert_eq!(model.quantization(), Quantization::Continuous);
}

#[test]
fn log_mapping_is_continuous_and_matches_independent_power_formula() {
    let model = zoom_model();
    for step in 0..=240 {
        let position = -12.0 + step as f64 * 0.1;
        let expected = 2.0_f64.powf(position / 12.0);
        assert!(
            (model.value_at(position) - expected).abs() < 1e-12,
            "положение {position}"
        );
    }
    // Прямое отображение обратно значению.
    for value in [0.5, 0.7, 1.0, 1.3, 2.0] {
        let round_trip = model.value_at(model.position_of(value).unwrap());
        assert!((round_trip - value).abs() < 1e-12, "{value}");
    }
}

#[test]
fn log_marks_merge_ticks_into_anchors_and_follow_limits() {
    let mut model = zoom_model();
    let marks = model.marks();
    let references: Vec<f64> = marks
        .iter()
        .filter(|mark| mark.emphasis == Emphasis::Reference)
        .map(|mark| mark.position)
        .collect();
    assert_eq!(references, vec![-12.0, 0.0, 12.0]);
    // Тик на месте якоря не дублируется (в -12, 0, 12 стоит ровно одна риска).
    for anchor in [-12.0, 0.0, 12.0] {
        assert_eq!(
            marks
                .iter()
                .filter(|mark| (mark.position - anchor).abs() < 1e-6)
                .count(),
            1
        );
    }
    assert!(
        marks.windows(2).all(|pair| pair[0].position < pair[1].position),
        "по возрастанию"
    );
    // Другая камера: нижний предел 1×. Якорь 0,5 пропадает с шкалы, но вернётся с пределами.
    model.set_limits(LogLimits { min: 1.0, max: 2.0 }).unwrap();
    let narrowed: Vec<f64> = model
        .marks()
        .iter()
        .filter(|mark| mark.emphasis == Emphasis::Reference)
        .map(|mark| mark.position)
        .collect();
    assert_eq!(narrowed, vec![0.0, 12.0]);
    model.set_limits(LogLimits { min: 0.5, max: 2.0 }).unwrap();
    assert_eq!(model.marks().len(), marks.len());
}

#[test]
fn log_clamps_out_of_range_and_rejects_non_positive() {
    let model = zoom_model();
    assert_eq!(model.clamp_value(10.0).unwrap(), 2.0);
    assert_eq!(model.clamp_value(0.1).unwrap(), 0.5);
    assert!(matches!(
        model.clamp_value(0.0),
        Err(ValueError::InvalidSpec { name: "value", .. })
    ));
    assert!(matches!(model.clamp_value(-1.0), Err(ValueError::InvalidSpec { .. })));
    assert!(matches!(model.clamp_value(f64::NAN), Err(ValueError::NonFinite(_))));
    assert_eq!(model.value_at(1000.0), 2.0);
    assert_eq!(model.value_at(-1000.0), 0.5);
    assert!(
        Logarithmic::new(
            LogarithmicSpec {
                units_per_octave: 12.0,
                tick_spacing_units: 3.0,
                anchors: vec![0.0],
            },
            LogLimits { min: 0.5, max: 2.0 }
        )
        .is_err()
    );
    let mut model = zoom_model();
    assert!(model.set_limits(LogLimits { min: 2.0, max: 0.5 }).is_err());
    assert!(model.set_limits(LogLimits { min: 0.0, max: 1.0 }).is_err());
    assert_eq!(model.limits(), &LogLimits { min: 0.5, max: 2.0 });
}

#[test]
fn series_maps_values_to_indices_and_clamps_to_external_limits() {
    // Выдержки в знаменателях секунды: 1/8000 … 1/1000.
    let denominators = vec![8000_u32, 4000, 2000, 1000];
    let mut series = ValueSeries::new(denominators, vec![], SeriesLimits::whole(4).unwrap()).unwrap();
    assert_eq!(series.position_of(2000).unwrap(), 2.0);
    assert_eq!(series.value_at(2.4), 2000);
    assert_eq!(series.value_at(2.6), 1000);
    assert_eq!(series.value_at(99.0), 1000);
    assert_eq!(series.value_at(-5.0), 8000);
    assert_eq!(series.quantization(), Quantization::Stepped);
    // Камера умеет только 4000…2000.
    let limits = series.limits_between(4000, 2000).unwrap();
    series.set_limits(limits).unwrap();
    assert_eq!(series.position_range(), (1.0, 2.0));
    assert_eq!(
        series.clamp_value(8000).unwrap(),
        4000,
        "значение ряда вне пределов — ближайшая граница"
    );
    assert_eq!(series.clamp_value(1000).unwrap(), 2000);
    assert_eq!(series.marks().len(), 2);
    // Значения не из ряда — явная ошибка, а не подмена.
    assert_eq!(series.clamp_value(3000), Err(ValueError::NotInSeries));
    assert_eq!(series.position_of(3000), Err(ValueError::NotInSeries));
}

#[test]
fn series_validates_construction_and_marks_references() {
    assert!(
        ValueSeries::<u32>::new(
            vec![],
            vec![],
            SeriesLimits {
                first_index: 0,
                last_index: 0
            }
        )
        .is_err()
    );
    assert!(matches!(
        ValueSeries::new(vec![1_u32, 1], vec![], SeriesLimits::whole(2).unwrap()),
        Err(ValueError::InvalidSpec { name: "values", .. })
    ));
    assert!(ValueSeries::new(vec![1_u32, 2], vec![5], SeriesLimits::whole(2).unwrap()).is_err());
    assert!(
        ValueSeries::new(
            vec![1_u32, 2],
            vec![],
            SeriesLimits {
                first_index: 1,
                last_index: 0
            }
        )
        .is_err()
    );
    assert!(
        ValueSeries::new(
            vec![1_u32, 2],
            vec![],
            SeriesLimits {
                first_index: 0,
                last_index: 2
            }
        )
        .is_err()
    );
    assert!(SeriesLimits::whole(0).is_err());
    // ISO: референс — второй элемент.
    let series = ValueSeries::new(vec![50_u32, 100, 200], vec![1], SeriesLimits::whole(3).unwrap()).unwrap();
    let emphasis: Vec<Emphasis> = series.marks().iter().map(|mark| mark.emphasis).collect();
    assert_eq!(emphasis, vec![Emphasis::Tick, Emphasis::Reference, Emphasis::Tick]);
    // Ряд чисел с плавающей точкой: NaN не находится.
    let floats = ValueSeries::new(vec![1.0_f64, 2.0], vec![], SeriesLimits::whole(2).unwrap()).unwrap();
    assert_eq!(floats.clamp_value(f64::NAN), Err(ValueError::NotInSeries));
}

#[test]
fn axis_pair_composes_two_independent_models_and_changes_limits_atomically() {
    // Кельвины 2500…8000 по 100 и оттенок -10…10 по 1.
    let kelvin = QuantizedLinear::new(
        100.0,
        ReferenceValue::At(5500.0),
        LinearLimits {
            min: 2500.0,
            max: 8000.0,
        },
    )
    .unwrap();
    let tint = QuantizedLinear::new(1.0, ReferenceValue::At(0.0), LinearLimits { min: -10.0, max: 10.0 }).unwrap();
    let mut pair = AxisPair::new(kelvin, tint);
    let clamped = pair
        .clamp_pair(PairValue {
            primary: 9000.0,
            secondary: -3.4,
        })
        .unwrap();
    assert_eq!(clamped.primary, 8000.0);
    assert_eq!(clamped.secondary, -3.0);
    // Успешная смена обеих осей.
    pair.set_limits(
        LinearLimits {
            min: 3000.0,
            max: 7000.0,
        },
        LinearLimits { min: -5.0, max: 5.0 },
    )
    .unwrap();
    assert_eq!(pair.primary().position_range(), (30.0, 70.0));
    assert_eq!(pair.secondary().position_range(), (-5.0, 5.0));
    // Вторая ось отвергает пределы → первая откатывается.
    let result = pair.set_limits(
        LinearLimits {
            min: 2500.0,
            max: 8000.0,
        },
        LinearLimits { min: 0.2, max: 0.4 },
    );
    assert!(result.is_err());
    assert_eq!(
        pair.primary().position_range(),
        (30.0, 70.0),
        "первая ось осталась прежней"
    );
    assert_eq!(pair.secondary().position_range(), (-5.0, 5.0));
    let (primary, secondary) = pair.into_axes();
    assert_eq!(primary.limits().min, 3000.0);
    assert_eq!(secondary.limits().max, 5.0);
}
