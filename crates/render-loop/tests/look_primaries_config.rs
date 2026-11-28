//! Секция `viewfinder.look_model.primaries` конфига рендера (сессия 14t, решения 339–342): из JSON до модели.
//! Годная секция принимается и доходит до модели значениями; каждая из четырёх ошибок проверки (не число, носок
//! не положителен, число вне диапазона, Lift ломает монотонность) и неизвестные поле и основа отвергаются РАЗНЫМИ
//! различимыми ошибками.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

use isp_model::look::model::InvalidLookModel;
use isp_model::look::primaries::{ChannelBasis, InvalidPrimariesModel, PrimariesModelSettings};
use isp_model::look::validation::ToneEdgeError;
use render_loop::config::{ConfigError, RenderLoopConfig, ViewfinderConfigError};

/// Конфиг рендера с заданной секцией `primaries` внутри `viewfinder.look_model`.
fn config_json(primaries: &str) -> String {
    format!(r#"{{"viewfinder":{{"look_model":{{"primaries":{primaries}}}}}}}"#)
}

fn parse(primaries: &str) -> RenderLoopConfig {
    serde_json::from_str(&config_json(primaries)).expect("секция читается")
}

/// Достаёт ошибку полосок из ошибки проверки конфига; любая другая форма — провал теста.
fn primaries_error(config: &RenderLoopConfig) -> InvalidPrimariesModel {
    match config.validate() {
        Err(ConfigError::Viewfinder(ViewfinderConfigError::LookModel(error))) => match error.source {
            ToneEdgeError::Model(InvalidLookModel::Primaries(primaries)) => primaries,
            other => panic!("ожидалась ошибка полосок, а пришло {other:?}"),
        },
        other => panic!("ожидалась ошибка модели вида, а пришло {other:?}"),
    }
}

#[test]
fn a_valid_custom_primaries_section_is_accepted_and_reaches_the_model() {
    let config = parse(
        r#"{"channel_basis":"working","encoding_toe":0.02,"lift_range":0.2,"gamma_slope_stops":1.0,"gain_stops_in_sqrt":0.5,"offset_range":0.1}"#,
    );
    config.validate().expect("годная секция проходит проверку конфига");
    assert_eq!(
        config.viewfinder.look_model.primaries,
        PrimariesModelSettings {
            channel_basis: ChannelBasis::Working,
            encoding_toe: 0.02,
            lift_range: 0.2,
            gamma_slope_stops: 1.0,
            gain_stops_in_sqrt: 0.5,
            offset_range: 0.1,
        },
        "значения секции дошли до модели"
    );
}

#[test]
fn an_omitted_primaries_section_keeps_the_default_model_with_the_display_p3_basis() {
    let config: RenderLoopConfig = serde_json::from_str(r#"{"viewfinder":{}}"#).expect("секция читается");
    config.validate().expect("умолчания годятся");
    let primaries = config.viewfinder.look_model.primaries;
    assert_eq!(primaries, PrimariesModelSettings::default());
    assert_eq!(primaries.channel_basis, ChannelBasis::DisplayP3);
}

#[test]
fn a_lift_range_at_or_above_the_monotonicity_limit_is_rejected_as_a_monotonicity_error() {
    // При gain_stops_in_sqrt = 1 предел Lift ровно 0,5: на краях ручек чёрное оказалось бы светлее белого.
    let config = parse(r#"{"gain_stops_in_sqrt":1.0,"lift_range":0.5}"#);
    assert!(
        matches!(
            primaries_error(&config),
            InvalidPrimariesModel::BreaksMonotonicity { lift_range, gain_floor, .. } if lift_range == 0.5 && gain_floor == 0.5
        ),
        "Lift на пределе — ошибка монотонности"
    );
    parse(r#"{"gain_stops_in_sqrt":1.0,"lift_range":0.49}"#)
        .validate()
        .expect("чуть ниже предела — годится");
}

#[test]
fn a_non_positive_encoding_toe_is_rejected_as_its_own_error() {
    let config = parse(r#"{"encoding_toe":0.0}"#);
    assert!(
        matches!(
            primaries_error(&config),
            InvalidPrimariesModel::EncodingToeNotPositive { value } if value == 0.0
        ),
        "нулевой носок — отдельная ошибка (наклон в нуле был бы бесконечным)"
    );
}

#[test]
fn a_number_outside_its_range_is_rejected_as_an_out_of_range_error_with_the_field_name() {
    let config = parse(r#"{"offset_range":0.9}"#);
    assert!(
        matches!(
            primaries_error(&config),
            InvalidPrimariesModel::OutOfRange { field: "offset_range", value, .. } if value == 0.9
        ),
        "сила Offset вне диапазона — ошибка диапазона с именем поля"
    );
    let negative = parse(r#"{"gamma_slope_stops":-1.0}"#);
    assert!(matches!(
        primaries_error(&negative),
        InvalidPrimariesModel::OutOfRange {
            field: "gamma_slope_stops",
            ..
        }
    ));
}

#[test]
fn an_unknown_primaries_field_or_basis_is_rejected_when_reading_the_config() {
    let unknown_field = serde_json::from_str::<RenderLoopConfig>(&config_json(r#"{"lift_strength":0.1}"#))
        .expect_err("неизвестное поле отвергается при чтении");
    assert!(unknown_field.to_string().contains("lift_strength"), "{unknown_field}");
    let unknown_basis = serde_json::from_str::<RenderLoopConfig>(&config_json(r#"{"channel_basis":"rec709"}"#))
        .expect_err("неизвестная основа отвергается при чтении");
    assert!(unknown_basis.to_string().contains("rec709"), "{unknown_basis}");
}
