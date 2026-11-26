//! Поверхность панели без стекла: тёмная полупрозрачная заливка (14m, решения 263 и 270).
//!
//! В разделе калибровки панели не берут фон кадра в размытие и линзу: это дешёвая закраска цветом
//! с плотностью (непрозрачностью). Пирамида размытия для такого кадра не строится вовсе.
//! `ui-kit` не знает слова «прибор»: роль [`SurfaceRole::Backing`] — просто «подложка плотнее панели»,
//! под данными, которые нельзя терять на ярком фоне.

use serde::{Deserialize, Serialize};
use ui_draw::color::DisplayColor;

use super::TokenError;
use super::glass::GlassRole;

/// Роль поверхности: от неё зависит плотность заливки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// Панели, листы, карточки и кнопки раздела.
    Panel,
    /// Подложка под данными: плотнее панели, чтобы данные читались на любом фоне.
    Backing,
}

/// Материал панели: чем панель закрашена. Его выбирает владелец экрана, а не сам виджет: один и тот
/// же виджет (капсула режимов, кнопка меню) на экране PHOTO стеклянный, а в разделе калибровки —
/// поверхность.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMaterial {
    /// Стекло: размытие фона, линза, блик (прежнее поведение PHOTO).
    Glass(GlassRole),
    /// Тёмная полупрозрачная поверхность без обращения к фону.
    Surface(SurfaceRole),
}

impl PanelMaterial {
    /// Рисуется ли материал через стекло (то есть нужна ли кадру пирамида размытия).
    pub fn is_glass(self) -> bool {
        matches!(self, PanelMaterial::Glass(_))
    }
}

/// Числа поверхности. Все три — конфиг (решение 263).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SurfaceTokens {
    /// Цвет заливки (sRGB, закодированный, как весь `ui-draw`). Альфа здесь не используется: непрозрачность
    /// задаёт плотность роли. `[estimate]` — решение 263: (0,06; 0,06; 0,07), почти чёрный с лёгкой синевой.
    pub fill_color: DisplayColor,
    /// Плотность панели (0..1), решение 263 — 0,70.
    pub panel_density: f32,
    /// Плотность подложки под данными (0..1), решение 263 — 0,88. Не меньше плотности панели.
    pub backing_density: f32,
}

impl Default for SurfaceTokens {
    fn default() -> Self {
        Self {
            fill_color: DisplayColor::rgb(0.06, 0.06, 0.07),
            panel_density: 0.70,
            backing_density: 0.88,
        }
    }
}

impl SurfaceTokens {
    /// Плотность заливки роли в покое (появление 1).
    pub fn density_for(&self, role: SurfaceRole) -> f32 {
        match role {
            SurfaceRole::Panel => self.panel_density,
            SurfaceRole::Backing => self.backing_density,
        }
    }

    /// Итоговый цвет заливки роли: цвет токена с альфой = плотность × появление (0..1).
    /// Появление растит плотность с нуля тем же темпом, что материализация стекла (решение 270).
    pub fn fill_for(&self, role: SurfaceRole, appear: f32) -> DisplayColor {
        self.fill_color
            .with_alpha(self.density_for(role) * appear.clamp(0.0, 1.0))
    }

    /// Проверка: цвет в 0..1, плотности в (0, 1], подложка не прозрачнее панели.
    pub fn validate(&self) -> Result<(), TokenError> {
        if !self.fill_color.is_valid() {
            return Err(TokenError::InvalidColor {
                name: "surface.fill_color",
            });
        }
        for (name, density) in [
            ("surface.panel_density", self.panel_density),
            ("surface.backing_density", self.backing_density),
        ] {
            if !(density.is_finite() && density > 0.0 && density <= 1.0) {
                return Err(TokenError::OutOfRange {
                    name,
                    value: f64::from(density),
                });
            }
        }
        if self.backing_density < self.panel_density {
            return Err(TokenError::OutOfRange {
                name: "surface.backing_density (меньше panel_density)",
                value: f64::from(self.backing_density),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_decision_263_and_valid() {
        let tokens = SurfaceTokens::default();
        assert_eq!(tokens.density_for(SurfaceRole::Panel), 0.70);
        assert_eq!(tokens.density_for(SurfaceRole::Backing), 0.88);
        assert_eq!(tokens.fill_color, DisplayColor::rgb(0.06, 0.06, 0.07));
        assert_eq!(tokens.validate(), Ok(()));
    }

    #[test]
    fn fill_alpha_is_density_times_appear() {
        let tokens = SurfaceTokens::default();
        let half = tokens.fill_for(SurfaceRole::Panel, 0.5);
        assert!((half.a - 0.35).abs() < 1e-6, "плотность × появление: {}", half.a);
        assert_eq!((half.r, half.g, half.b), (0.06, 0.06, 0.07));
        assert_eq!(tokens.fill_for(SurfaceRole::Backing, 0.0).a, 0.0);
        // Появление вне 0..1 не раздувает плотность выше роли.
        assert_eq!(tokens.fill_for(SurfaceRole::Backing, 7.0).a, 0.88);
    }

    #[test]
    fn invalid_values_are_rejected() {
        let zero_density = SurfaceTokens {
            panel_density: 0.0,
            ..SurfaceTokens::default()
        };
        assert!(matches!(zero_density.validate(), Err(TokenError::OutOfRange { .. })));
        let over_one = SurfaceTokens {
            backing_density: 1.2,
            ..SurfaceTokens::default()
        };
        assert!(matches!(over_one.validate(), Err(TokenError::OutOfRange { .. })));
        let not_a_number = SurfaceTokens {
            panel_density: f32::NAN,
            ..SurfaceTokens::default()
        };
        assert!(matches!(not_a_number.validate(), Err(TokenError::OutOfRange { .. })));
        let thinner_backing = SurfaceTokens {
            backing_density: 0.5,
            ..SurfaceTokens::default()
        };
        assert!(matches!(thinner_backing.validate(), Err(TokenError::OutOfRange { .. })));
        let bad_color = SurfaceTokens {
            fill_color: DisplayColor::rgb(1.5, 0.0, 0.0),
            ..SurfaceTokens::default()
        };
        assert_eq!(
            bad_color.validate(),
            Err(TokenError::InvalidColor {
                name: "surface.fill_color"
            })
        );
    }

    #[test]
    fn config_overrides_and_rejects_unknown_fields() {
        let parsed: SurfaceTokens = serde_json::from_str(r#"{"panel_density":0.5}"#).expect("частичный конфиг");
        assert_eq!(parsed.panel_density, 0.5);
        assert_eq!(parsed.backing_density, 0.88);
        assert!(serde_json::from_str::<SurfaceTokens>(r#"{"density":0.5}"#).is_err());
    }

    #[test]
    fn material_reports_glass_only_for_glass() {
        assert!(PanelMaterial::Glass(GlassRole::Button).is_glass());
        assert!(!PanelMaterial::Surface(SurfaceRole::Panel).is_glass());
    }
}
