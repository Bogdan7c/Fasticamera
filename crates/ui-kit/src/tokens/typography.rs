//! Типографика: роли текста экрана (`docs/ios-camera/assets.md`, разделы 2–3; `layout.md` 2.1).
//!
//! Роль — это «кому нужен текст»: подпись режима, текст плашки, число на шкале. Экран и виды
//! `ui-kit` ссылаются на роль по имени ([`TextRoleKind`]); начертание, кегль, трекинг и вид цифр
//! живут только здесь. Шрифт — Inter (решение 81), поэтому кегли подобраны под его пропорции.
//!
//! # Происхождение чисел
//! * `[measured: R00 t=10.027]` подпись режима: высота заглавных 10,0–10,3 pt при доле заглавной
//!   Inter 0,728 → кегль ≈ 14,0 pt (полужирный). Остальные роли экрана iOS 27 считаются так же:
//!   кегль = высота заглавных / 0,728 (решение 104), метки замера — у каждого поля;
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
    /// Заголовок нижней карточки инструмента (ЗАГЛАВНЫМИ).
    CardTitle,
    /// Крупное значение в карточке («Off», «3s», «4:3», «0.0»): табличные цифры.
    CardValue,
    /// Подпись пункта шторки Controls под кругом (ЗАГЛАВНЫМИ).
    ToolCaption,
    /// Число в верхнем ряду («0.0» над «EV»): табличные цифры.
    StripValue,
    /// Единица под числом в верхнем ряду («EV»).
    StripUnit,
    /// Подпись варианта в ряду нижней карточки («4:3», «On», «10»): табличные цифры.
    CardOption,
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
    /// `[measured: R00 t=10.027]` заглавные 10,0–10,3 pt → кегль 14,0 pt для Inter (доля 0,728);
    /// полужирный.
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
    /// `[measured: C t=40.0]` заглавные 10,3 pt → кегль 14,1 pt; полужирный.
    pub card_title: TextRole,
    /// `[measured: C t=54.0]` высота цифр 20,7 pt → кегль 28,4 pt; обычный, табличные цифры.
    /// Атлас запечён под ~34 pt, 28,4 укладывается без увеличения.
    pub card_value: TextRole,
    /// `[measured: R00 t=22.512, C t=30.0]` заглавные 10,3 pt → кегль 14,1 pt; обычный.
    pub tool_caption: TextRole,
    /// `[measured: C t=18.0]` заглавные 10,3 pt → кегль 14,1 pt; полужирный, табличные цифры.
    pub strip_value: TextRole,
    /// `[measured: C t=18.0]` заглавные 6,6 pt → кегль 9,1 pt; полужирный.
    pub strip_unit: TextRole,
    /// `[estimate]` — подгонка, не Apple: размер вариантов карточки не снят, взят как у подписей верхнего
    /// ряда (`strip_value`, `[measured: C t=18.0]` заглавные 10,3 pt → 14,1 pt); полужирный, табличные цифры.
    pub card_option: TextRole,
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
            mode_label: role(TextWeight::Semibold, 14.0, DigitStyle::Proportional),
            banner_text: role(TextWeight::Semibold, 13.0, DigitStyle::Proportional),
            scale_value: role(TextWeight::Semibold, 15.0, DigitStyle::Tabular),
            menu_item: role(TextWeight::Regular, 13.0, DigitStyle::Proportional),
            caption: role(TextWeight::Regular, 12.0, DigitStyle::Proportional),
            countdown: role(TextWeight::Semibold, 105.0, DigitStyle::Tabular),
            card_title: role(TextWeight::Semibold, 14.1, DigitStyle::Proportional),
            card_value: role(TextWeight::Regular, 28.4, DigitStyle::Tabular),
            tool_caption: role(TextWeight::Regular, 14.1, DigitStyle::Proportional),
            strip_value: role(TextWeight::Semibold, 14.1, DigitStyle::Tabular),
            strip_unit: role(TextWeight::Semibold, 9.1, DigitStyle::Proportional),
            card_option: role(TextWeight::Semibold, 14.1, DigitStyle::Tabular),
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
            TextRoleKind::CardTitle => &self.card_title,
            TextRoleKind::CardValue => &self.card_value,
            TextRoleKind::ToolCaption => &self.tool_caption,
            TextRoleKind::StripValue => &self.strip_value,
            TextRoleKind::StripUnit => &self.strip_unit,
            TextRoleKind::CardOption => &self.card_option,
        }
    }

    /// Проверить все роли.
    pub fn validate(&self) -> Result<(), TokenError> {
        self.mode_label.validate("typography.mode_label")?;
        self.banner_text.validate("typography.banner_text")?;
        self.scale_value.validate("typography.scale_value")?;
        self.menu_item.validate("typography.menu_item")?;
        self.caption.validate("typography.caption")?;
        self.countdown.validate("typography.countdown")?;
        self.card_title.validate("typography.card_title")?;
        self.card_value.validate("typography.card_value")?;
        self.tool_caption.validate("typography.tool_caption")?;
        self.strip_value.validate("typography.strip_value")?;
        self.strip_unit.validate("typography.strip_unit")?;
        self.card_option.validate("typography.card_option")
    }
}
