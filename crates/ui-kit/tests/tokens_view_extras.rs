#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Токены, добавленные пакетом P1g: роль `Countdown`, размеры рисок шкалы, цвет выбранной подписи.
//! Главное — совместимость: старый конфиг без новых полей читается, неверные значения отвергаются.

use ui_draw::color::DisplayColor;
use ui_draw::text::{Figures, layout_line};
use ui_kit::tokens::{TextRoleKind, TokenError, UiKitTokens};

#[test]
fn countdown_role_is_a_large_tabular_figure_style_that_lays_out_digits() {
    let tokens = UiKitTokens::default();
    let style = tokens.typography.role(TextRoleKind::Countdown).style(3.0);
    // Прежний вид экрана: кегль шкалы 15 pt × 7 = 105 pt, 3 px на pt.
    assert!((style.size_px - 315.0).abs() < 1e-3, "{}", style.size_px);
    assert_eq!(style.figures, Figures::Tabular);
    // Все цифры одной ширины (табличные): отсчёт 3-2-1 не «дышит».
    let widths: Vec<f32> = ["1", "2", "3"]
        .iter()
        .map(|d| layout_line(d, &style).unwrap().advance_px)
        .collect();
    assert!(
        widths.windows(2).all(|pair| (pair[0] - pair[1]).abs() < 1e-3),
        "{widths:?}"
    );
    // И крупнее любой другой роли.
    for other in [
        TextRoleKind::ModeLabel,
        TextRoleKind::BannerText,
        TextRoleKind::ScaleValue,
        TextRoleKind::MenuItem,
        TextRoleKind::Caption,
    ] {
        assert!(tokens.typography.role(other).size_pt < tokens.typography.countdown.size_pt);
    }
}

#[test]
fn old_config_without_new_fields_still_loads_with_defaults() {
    // Конфиг, записанный до P1g, не знает новых полей: пустой и частичный объекты читаются умолчаниями.
    let from_empty: UiKitTokens = serde_json::from_str("{}").unwrap();
    assert_eq!(from_empty, UiKitTokens::default());
    let from_partial: UiKitTokens = serde_json::from_str(r#"{ "scale_ticks": { "thickness_pt": 2.0 } }"#).unwrap();
    assert_eq!(from_partial.scale_ticks.thickness_pt, 2.0);
    assert_eq!(
        from_partial.scale_ticks.tick_length_pt,
        UiKitTokens::default().scale_ticks.tick_length_pt
    );
    assert_eq!(from_partial.colors, UiKitTokens::default().colors);
    // Неизвестное поле по-прежнему ошибка (deny_unknown_fields).
    assert!(serde_json::from_str::<UiKitTokens>(r#"{ "scale_ticks": { "unknown": 1 } }"#).is_err());
}

#[test]
fn scale_tick_sizes_must_be_positive_and_finite() {
    let mut tokens = UiKitTokens::default();
    tokens.validate().unwrap();
    tokens.scale_ticks.thickness_pt = 0.0;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::NonPositiveSize {
            name: "scale_ticks.thickness_pt",
            ..
        })
    ));
    tokens.scale_ticks.thickness_pt = 1.5;
    tokens.scale_ticks.current_mark_length_pt = f64::NAN;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::NonPositiveSize {
            name: "scale_ticks.current_mark_length_pt",
            ..
        })
    ));
}

#[test]
fn selected_label_colour_and_countdown_role_are_validated() {
    let mut tokens = UiKitTokens::default();
    tokens.colors.selected_label = DisplayColor::rgb(2.0, 0.0, 0.0);
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::InvalidColor { name: "selected_label" })
    ));
    let mut tokens = UiKitTokens::default();
    tokens.typography.countdown.size_pt = -1.0;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::NonPositiveSize {
            name: "typography.countdown",
            ..
        })
    ));
}
