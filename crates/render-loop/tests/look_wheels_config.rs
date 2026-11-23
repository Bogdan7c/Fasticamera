//! Секция `viewfinder.look_model.wheels` конфига рендера (сессия 14i, решения 186, 191–193): из JSON до модели.
//! Годная своя секция принимается и доходит до модели значениями; кольцо за пределом монотонности, число вне
//! диапазона и неизвестное поле отвергаются РАЗНЫМИ различимыми ошибками.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

use isp_model::look::model::InvalidLookModel;
use isp_model::look::validation::ToneEdgeError;
use isp_model::look::wheels::InvalidWheelModel;
use render_loop::config::{ConfigError, RenderLoopConfig, ViewfinderConfigError};

/// Конфиг рендера с заданной секцией `wheels` внутри `viewfinder.look_model`.
fn config_json(wheels: &str) -> String {
    format!(r#"{{"viewfinder":{{"look_model":{{"wheels":{wheels}}}}}}}"#)
}

/// Достаёт ошибку колёс из ошибки проверки конфига; любая другая форма — провал теста.
fn wheel_error(config: &RenderLoopConfig) -> InvalidWheelModel {
    match config.validate() {
        Err(ConfigError::Viewfinder(ViewfinderConfigError::LookModel(error))) => match error.source {
            ToneEdgeError::Model(InvalidLookModel::Wheels(wheels)) => wheels,
            other => panic!("ожидалась ошибка колёс, а пришло {other:?}"),
        },
        other => panic!("ожидалась ошибка модели вида, а пришло {other:?}"),
    }
}

#[test]
fn a_valid_custom_wheels_section_is_accepted_and_reaches_the_model() {
    let config: RenderLoopConfig = serde_json::from_str(&config_json(
        r#"{"push_chroma_range":0.08,"ring_lightness_range":0.12,"zone_pivot_display_gray":0.18}"#,
    ))
    .expect("секция читается");

    config.validate().expect("годная секция проходит проверку конфига");
    let wheels = config.viewfinder.look_model.wheels;
    assert_eq!(
        (
            wheels.push_chroma_range,
            wheels.ring_lightness_range,
            wheels.zone_pivot_display_gray
        ),
        (0.08, 0.12, 0.18),
        "значения секции дошли до модели"
    );
}

#[test]
fn an_omitted_wheels_section_keeps_the_default_model() {
    let config: RenderLoopConfig = serde_json::from_str(r#"{"viewfinder":{}}"#).expect("секция читается");
    config.validate().expect("умолчания годятся");
    assert_eq!(
        config.viewfinder.look_model.wheels,
        isp_model::look::wheels::WheelModelSettings::default()
    );
}

#[test]
fn a_ring_range_above_the_monotonicity_limit_is_rejected_as_a_monotonicity_error() {
    // При сером 0,20 предел кольца ≈ 0,138: 0,25 переворачивает тон на краю кольца.
    let config: RenderLoopConfig =
        serde_json::from_str(&config_json(r#"{"ring_lightness_range":0.25}"#)).expect("JSON читается");
    assert!(
        matches!(
            wheel_error(&config),
            InvalidWheelModel::RingBreaksMonotonicity { value, .. } if value == 0.25
        ),
        "кольцо сверх предела — ошибка монотонности"
    );
}

#[test]
fn a_number_outside_its_range_is_rejected_as_an_out_of_range_error() {
    let config: RenderLoopConfig =
        serde_json::from_str(&config_json(r#"{"push_chroma_range":0.9}"#)).expect("JSON читается");
    assert!(
        matches!(
            wheel_error(&config),
            InvalidWheelModel::OutOfRange { field: "push_chroma_range", value, .. } if value == 0.9
        ),
        "сила толчка вне диапазона — ошибка диапазона с именем поля"
    );
}

#[test]
fn an_unknown_wheels_field_is_rejected_when_reading_the_config() {
    let error = serde_json::from_str::<RenderLoopConfig>(&config_json(r#"{"ring_lightness":0.1}"#))
        .expect_err("неизвестное поле отвергается при чтении");
    assert!(error.to_string().contains("ring_lightness"), "{error}");
}
