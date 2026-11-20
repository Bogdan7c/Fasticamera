//! Типографика: роли текста экрана (`docs/ios-camera/assets.md`, разделы 2–3; `layout.md` 2.1).
//!
//! Роль — это «кому нужен текст»: подпись режима, текст плашки, число на шкале. Экран и виды
//! `ui-kit` ссылаются на роль по имени ([`TextRoleKind`]); начертание, кегль, трекинг и вид цифр
//! живут только здесь. Шрифт — Inter (решение 81), поэтому кегли подобраны под его пропорции.
//!
//! # Происхождение чисел
//! * `[derived]` подпись режима: заглавные 14,2 pt (`layout.md` 2.1, замер) при высоте заглавных
//!   Inter 0,7275 em (1490/2048) → кегль 14,2 / 0,7275 ≈ 19,5 pt;
//! * `[estimate]` — подгонка, не Apple: остальные роли (их размеров в замерах нет).
//!
//! Dynamic Type экран камеры не следует (`assets.md` раздел 3): кегли фиксированные.

use serde::{Deserialize, Serialize};
use ui_draw::text::tracking::sf_pro_tracking_em;
use ui_draw::text::{Figures, FontFace, TextStyle};

use super::error::TokenError;

/// Начертание из запечённого набора (`ui-draw/assets/bake.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextWeight {
    Regular,
    Semibold,
}

impl TextWeight {
    fn face(self) -> FontFace {
        match self {
            TextWeight::Regular => FontFace::Regular,
            TextWeight::Semibold => FontFace::Semibold,
        }
    }
}

/// Какие цифры нужны роли.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DigitStyle {
    /// Обычные цифры шрифта.
    Proportional,
    /// Табличные (`tnum`): подпись не дёргается при смене цифры (EV, зум).
    Tabular,
}

impl DigitStyle {
    fn figures(self) -> Figures {
        match self {
            DigitStyle::Proportional => Figures::Proportional,
            DigitStyle::Tabular => Figures::Tabular,
        }
    }
}

/// Откуда берётся трекинг роли.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tracking {
    /// Таблица SF Pro из HIG по кеглю (`assets.md` раздел 3) — стартовая точка подгонки.
    SfProTable,
    /// Фиксированный трекинг, доли em.
    FixedEm { em: f32 },
}

/// Какая роль текста нужна.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextRoleKind {
    /// Подпись режима в карусели (PHOTO).
    ModeLabel,
    /// Текст плашки (AE/AF LOCK).
    BannerText,
    /// Числовое значение шкалы (EV, зум): табличные цифры.
    ScaleValue,
    /// Подпись пункта меню.
    MenuItem,
    /// Мелкая подпись.
    Caption,
    /// Крупные цифры отсчёта таймера по центру кадра.
    Countdown,
}

/// Одна роль текста.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRole {
    pub weight: TextWeight,
    /// Кегль, pt (1 pt = ширина экрана / 440 px, решение 82).
    pub size_pt: f32,
    pub tracking: Tracking,
    pub digits: DigitStyle,
}

impl TextRole {
    /// Стиль вёрстки `ui-draw` при масштабе `px_per_pt`.
    pub fn style(&self, px_per_pt: f32) -> TextStyle {
        TextStyle {
            face: self.weight.face(),
            size_px: self.size_pt * px_per_pt,
            tracking_em: match self.tracking {
                Tracking::SfProTable => sf_pro_tracking_em(self.size_pt),
                Tracking::FixedEm { em } => em,
            },
            figures: self.digits.figures(),
        }
    }

    fn validate(&self, name: &'static str) -> Result<(), TokenError> {
        if !self.size_pt.is_finite() || self.size_pt <= 0.0 {
            return Err(TokenError::NonPositiveSize {
                name,
                value: f64::from(self.size_pt),
            });
        }
        if let Tracking::FixedEm { em } = self.tracking
            && !em.is_finite()
        {
            return Err(TokenError::OutOfRange {
                name,
                value: f64::from(em),
            });
        }
        Ok(())
    }
}

/// Все роли текста.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TypographyTokens {
    /// `[derived]` заглавные 14,2 pt → кегль ≈ 19,5 pt для Inter; полужирный (`layout.md` 2.1).
    pub mode_label: TextRole,
    /// `[estimate]` — подгонка, не Apple: плашка 30 pt высотой, текст ≈ 13 pt полужирный.
    pub banner_text: TextRole,
    /// `[estimate]` — подгонка, не Apple: число на шкале, табличные цифры.
    pub scale_value: TextRole,
    /// `[estimate]` — подгонка, не Apple: подпись пункта меню.
    pub menu_item: TextRole,
    /// `[estimate]` — подгонка, не Apple: мелкая подпись (Caption 1 из HIG = 12 pt).
    pub caption: TextRole,
    /// `[estimate]` — подгонка, не Apple: цифры отсчёта таймера, 15 pt шкалы × 7 (прежний вид экрана).
    /// Ограничение: атлас запечён под кегль до ~34 pt (`em_texels = 64`), здесь увеличение ~4×, углы
    /// цифр скруглены SDF — сверить со скринами iPhone в 12a.
    pub countdown: TextRole,
}

impl Default for TypographyTokens {
    fn default() -> Self {
        let role = |weight, size_pt, digits| TextRole {
            weight,
            size_pt,
            tracking: Tracking::SfProTable,
            digits,
        };
        Self {
            mode_label: role(TextWeight::Semibold, 19.5, DigitStyle::Proportional),
            banner_text: role(TextWeight::Semibold, 13.0, DigitStyle::Proportional),
            scale_value: role(TextWeight::Semibold, 15.0, DigitStyle::Tabular),
            menu_item: role(TextWeight::Regular, 13.0, DigitStyle::Proportional),
            caption: role(TextWeight::Regular, 12.0, DigitStyle::Proportional),
            countdown: role(TextWeight::Semibold, 105.0, DigitStyle::Tabular),
        }
    }
}

impl TypographyTokens {
    /// Роль по имени.
    pub fn role(&self, kind: TextRoleKind) -> &TextRole {
        match kind {
            TextRoleKind::ModeLabel => &self.mode_label,
            TextRoleKind::BannerText => &self.banner_text,
            TextRoleKind::ScaleValue => &self.scale_value,
            TextRoleKind::MenuItem => &self.menu_item,
            TextRoleKind::Caption => &self.caption,
            TextRoleKind::Countdown => &self.countdown,
        }
    }

    /// Проверить все роли.
    pub fn validate(&self) -> Result<(), TokenError> {
        self.mode_label.validate("typography.mode_label")?;
        self.banner_text.validate("typography.banner_text")?;
        self.scale_value.validate("typography.scale_value")?;
        self.menu_item.validate("typography.menu_item")?;
        self.caption.validate("typography.caption")?;
        self.countdown.validate("typography.countdown")
    }
}
