//! Пресеты материала стекла: «кнопка» и «панель/меню» (`docs/ios-camera/glass.md`, раздел 6).
//!
//! Сами формулы и тип [`GlassStyle`] живут в `ui-draw` (решение 86); здесь — числа дизайн-системы
//! экрана: какие значения берёт кнопка и какие панель. Все числа — `[estimate]` на основе `[reverse]`
//! (Kyant0/AndroidLiquidGlass): подгонка, не Apple. Проверять глазами на 11a.

use serde::{Deserialize, Serialize};
use ui_draw::glass::{DarkEdgeStyle, DimmingStyle, Extent, GlassStyle, PressStyle, ShadowStyle, SpecularStyle};

use super::TokenError;

/// Роль стекла: от неё зависит, какой пресет брать.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlassRole {
    /// Кнопки, круглые значки, затвор, сегменты (≈ 44–48 pt).
    Button,
    /// Панели, меню, листы (крупные формы).
    Panel,
    /// Матовое размытие без линзы: «замёрзший» кадр под интерфейсом на смене объектива (13).
    Frost,
}

/// Пресеты стекла экрана.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GlassTokens {
    /// Кнопка: зона линзы 25 % высоты, сила 50 % высоты, размытие 2 pt (Kyant: 12/24/2 dp на 48 dp).
    pub button: GlassStyle,
    /// Панель/меню: зона линзы 24 pt, сила 28 pt, размытие 8 pt. Сила 24–32 pt по `glass.md`: у
    /// больших форм сильнее (Apple 219, 7:02) — берём середину; размытие мягче (Kyant: 8 dp).
    pub panel: GlassStyle,
    /// Чистое размытие кадра без преломления, блика, тона, тени и насыщения: кадр узнаваем, но мягкий.
    /// Радиус `[estimate]` — подгонка, не Apple: iOS размывает кадр на смене режима (`motion.md` M:108), радиус
    /// не измерен.
    pub frost: GlassStyle,
}

impl Default for GlassTokens {
    fn default() -> Self {
        Self {
            // Значения по умолчанию `GlassStyle` — это и есть пресет кнопки.
            button: GlassStyle::default(),
            panel: GlassStyle {
                lens_zone: Extent::Points(24.0),
                refraction_strength: Extent::Points(28.0),
                blur_radius_pt: 8.0,
                ..GlassStyle::default()
            },
            frost: GlassStyle {
                // Зона линзы не нулевая (деление в профиле), но сила преломления нулевая: сдвига выборки нет.
                lens_zone: Extent::Points(1.0),
                refraction_strength: Extent::Points(0.0),
                blur_radius_pt: 28.0,
                saturation: 1.0,
                dispersion: 0.0,
                specular: SpecularStyle {
                    alpha: 0.0,
                    ..SpecularStyle::default()
                },
                dark_edge: DarkEdgeStyle {
                    strength: 0.0,
                    ..DarkEdgeStyle::default()
                },
                dimming: DimmingStyle {
                    max_alpha: 0.0,
                    ..DimmingStyle::default()
                },
                tint_level: 0.0,
                shadow: ShadowStyle {
                    alpha: 0.0,
                    ..ShadowStyle::default()
                },
                press: PressStyle {
                    scale_gain: 0.0,
                    refraction_boost: 0.0,
                    glow_alpha: 0.0,
                    ..PressStyle::default()
                },
                ..GlassStyle::default()
            },
        }
    }
}

impl GlassTokens {
    /// Пресет по роли.
    pub fn style_for(&self, role: GlassRole) -> &GlassStyle {
        match role {
            GlassRole::Button => &self.button,
            GlassRole::Panel => &self.panel,
            GlassRole::Frost => &self.frost,
        }
    }

    /// Проверить оба пресета.
    pub fn validate(&self) -> Result<(), TokenError> {
        self.button
            .validate()
            .map_err(|source| TokenError::InvalidGlass { role: "button", source })?;
        self.panel
            .validate()
            .map_err(|source| TokenError::InvalidGlass { role: "panel", source })?;
        self.frost
            .validate()
            .map_err(|source| TokenError::InvalidGlass { role: "frost", source })
    }
}
