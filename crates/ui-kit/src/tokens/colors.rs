//! Цвета элементов: токены, а не литералы в видах (`docs/ios-camera/glass.md`, `layout.md` 2.2).
//!
//! Все значения `[estimate]` — подгонка, не Apple: Apple точных цветов не публикует, а замеры
//! видео (сжатие YouTube) дают только «жёлтый, тёмный текст». Цвета лежат в закодированных
//! значениях экрана, как и весь `ui-draw` ([`ui_draw::color`]). Сверить со скринами iPhone в 12a.

use serde::{Deserialize, Serialize};
use ui_draw::color::DisplayColor;

use super::error::TokenError;

/// Цвета экрана камеры.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ColorTokens {
    /// Жёлтый фокуса, «солнца» и плашки. `[estimate]` — системный жёлтый iOS (#FFCC00).
    pub focus_yellow: DisplayColor,
    /// Тёмный текст на жёлтой плашке. `[estimate]` — почти чёрный.
    pub plate_text: DisplayColor,
    /// Основной текст и иконки на стекле. `[estimate]` — белый.
    pub text_primary: DisplayColor,
    /// Приглушённый текст (невыбранные подписи). `[estimate]` — белый, альфа 0,6.
    pub text_muted: DisplayColor,
    /// Подпись на стеклянной панели раздела калибровки (названия ручек, групп, прибора). Отдельно от `text_muted`,
    /// чтобы экран PHOTO не менялся. Выбор владельца по эскизам (14k, решение 228) — белый, альфа 0,92.
    pub panel_label: DisplayColor,
    /// Полупрозрачное затемнение (полосы, шторка под меню). `[estimate]` — чёрный, альфа 0,4.
    pub scrim: DisplayColor,
    /// Выбранная подпись карусели режимов (PHOTO). `[estimate]` — подгонка, не Apple: жёлтый, как
    /// у выбранного режима в камере iOS; сверить со скринами iPhone в 12a.
    pub selected_label: DisplayColor,
    /// Выбранная подпись и значение зума. `[measured: R00 t=88.5–100]` #FFCE00 (золотистый, не лимонный).
    pub zoom_selected: DisplayColor,
}

impl Default for ColorTokens {
    fn default() -> Self {
        Self {
            focus_yellow: DisplayColor::rgb(1.0, 0.8, 0.0),
            plate_text: DisplayColor::rgb(0.08, 0.08, 0.08),
            text_primary: DisplayColor::WHITE,
            text_muted: DisplayColor::WHITE.with_alpha(0.6),
            panel_label: DisplayColor::WHITE.with_alpha(0.92),
            scrim: DisplayColor::BLACK.with_alpha(0.4),
            selected_label: DisplayColor::rgb(1.0, 0.8, 0.0),
            zoom_selected: DisplayColor::rgb(1.0, 206.0 / 255.0, 0.0),
        }
    }
}

impl ColorTokens {
    /// Все цвета конечны и в 0..1.
    pub fn validate(&self) -> Result<(), TokenError> {
        let colors = [
            ("focus_yellow", self.focus_yellow),
            ("plate_text", self.plate_text),
            ("text_primary", self.text_primary),
            ("text_muted", self.text_muted),
            ("panel_label", self.panel_label),
            ("scrim", self.scrim),
            ("selected_label", self.selected_label),
            ("zoom_selected", self.zoom_selected),
        ];
        for (name, color) in colors {
            if !color.is_valid() {
                return Err(TokenError::InvalidColor { name });
            }
        }
        Ok(())
    }
}
