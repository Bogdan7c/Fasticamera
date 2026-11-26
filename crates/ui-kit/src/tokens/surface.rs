//! Поверхность панели без стекла: тёмная полупрозрачная заливка (14m, решения 263 и 270; 14p, решение 297).
//!
//! Панели не берут фон кадра в размытие и линзу: это дешёвая закраска цветом с плотностью
//! (непрозрачностью). Пирамида размытия для такого кадра не строится вовсе.
//! `ui-kit` не знает слов «прибор» и «чёрная полоса»: роль [`SurfaceRole::Backing`] — просто «подложка
//! плотнее панели» под данными, роль [`SurfaceRole::EdgedPanel`] — «панель с тонким светлым ободком»,
//! чей силуэт читается и там, где за ней чёрный фон.

use serde::{Deserialize, Serialize};
use ui_draw::color::DisplayColor;

use super::TokenError;
use super::glass::GlassRole;

/// Роль поверхности: от неё зависят плотность заливки и наличие ободка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRole {
    /// Панели, листы, карточки и кнопки раздела калибровки: только заливка.
    Panel,
    /// Подложка под данными: плотнее панели, чтобы данные читались на любом фоне.
    Backing,
    /// Панель с ободком: заливка как у [`SurfaceRole::Panel`] плюс тонкий светлый ободок по краю силуэта
    /// (решение 297). Нужна, когда панель лежит и на картинке, и на чёрном фоне: тёмная заливка на чёрном
    /// почти не видна, ободок держит силуэт.
    EdgedPanel,
}

/// Материал панели: чем панель закрашена. Его выбирает владелец экрана, а не сам виджет: один и тот
/// же виджет (капсула режимов, кнопка меню) на экране PHOTO — поверхность с ободком, а в разделе
/// калибровки — поверхность без ободка (решение 297).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelMaterial {
    /// Стекло: размытие фона, линза, блик. С 14p экраны камеры его для панелей не выбирают (решение 297);
    /// единственное стекло камеры — завеса перехода, она собирает его сама (решение 299).
    Glass(GlassRole),
    /// Тёмная полупрозрачная поверхность без обращения к фону.
    Surface(SurfaceRole),
}

/// Ободок поверхности с ободком (решение 297): светлая линия по внутреннему краю силуэта.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SurfaceEdgeTokens {
    /// Цвет линии (sRGB, закодированный). Альфа здесь не используется — её задаёт [`Self::alpha`], как у заливки
    /// плотность. `[estimate]` решение 297: белый.
    pub color: DisplayColor,
    /// Непрозрачность линии в покое (появление 1), 0..1. `[estimate]` решение 297: 0,22 — подгонка по эскизам 14p,
    /// не Apple.
    pub alpha: f32,
    /// Толщина линии, pt. `[estimate]` решение 297: 1 pt. Линия лежит ВНУТРИ силуэта: размеры элемента
    /// с ободком и без него одинаковы.
    pub width_pt: f32,
}

impl Default for SurfaceEdgeTokens {
    fn default() -> Self {
        Self {
            color: DisplayColor::WHITE,
            alpha: 0.22,
            width_pt: 1.0,
        }
    }
}

/// Предел толщины ободка, pt: толще — это уже не «тонкая линия», а вторая заливка (скорее опечатка в единицах).
const MAX_EDGE_WIDTH_PT: f32 = 4.0;

/// Числа поверхности. Все — конфиг (решения 263, 297, 301).
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
    /// Ободок роли [`SurfaceRole::EdgedPanel`]; плотность заливки у неё та же, что у панели (решение 298).
    pub edge: SurfaceEdgeTokens,
}

impl Default for SurfaceTokens {
    fn default() -> Self {
        Self {
            fill_color: DisplayColor::rgb(0.06, 0.06, 0.07),
            panel_density: 0.70,
            backing_density: 0.88,
            edge: SurfaceEdgeTokens::default(),
        }
    }
}

impl SurfaceTokens {
    /// Плотность заливки роли в покое (появление 1).
    pub fn density_for(&self, role: SurfaceRole) -> f32 {
        match role {
            // Панель с ободком закрашена так же плотно, как панель: отличие только в ободке (решение 298).
            SurfaceRole::Panel | SurfaceRole::EdgedPanel => self.panel_density,
            SurfaceRole::Backing => self.backing_density,
        }
    }

    /// Ободок роли при появлении `appear` (0..1): цвет с альфой «альфа ободка × появление» и толщина в pt.
    /// `None` — у роли ободка нет. Ободок нарастает вместе с плотностью заливки (решение 270).
    pub fn edge_for(&self, role: SurfaceRole, appear: f32) -> Option<SurfaceEdge> {
        match role {
            SurfaceRole::Panel | SurfaceRole::Backing => None,
            SurfaceRole::EdgedPanel => Some(SurfaceEdge {
                color: self.edge.color.with_alpha(self.edge.alpha * appear.clamp(0.0, 1.0)),
                width_pt: self.edge.width_pt,
            }),
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
        self.edge.validate()
    }
}

impl SurfaceEdgeTokens {
    /// Проверка: цвет в 0..1, альфа в (0, 1], толщина конечна, больше нуля и не толще [`MAX_EDGE_WIDTH_PT`].
    pub fn validate(&self) -> Result<(), TokenError> {
        if !self.color.is_valid() {
            return Err(TokenError::InvalidColor {
                name: "surface.edge.color",
            });
        }
        if !(self.alpha.is_finite() && self.alpha > 0.0 && self.alpha <= 1.0) {
            return Err(TokenError::OutOfRange {
                name: "surface.edge.alpha",
                value: f64::from(self.alpha),
            });
        }
        if !(self.width_pt.is_finite() && self.width_pt > 0.0 && self.width_pt <= MAX_EDGE_WIDTH_PT) {
            return Err(TokenError::OutOfRange {
                name: "surface.edge.width_pt",
                value: f64::from(self.width_pt),
            });
        }
        Ok(())
    }
}

/// Ободок, готовый к рисованию: цвет с итоговой альфой и толщина линии в pt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceEdge {
    pub color: DisplayColor,
    pub width_pt: f32,
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
    fn edged_panel_is_as_dense_as_panel_and_only_it_has_an_edge() {
        let tokens = SurfaceTokens::default();
        // Решение 298: плотность панели с ободком — та же, что у панели раздела.
        assert_eq!(
            tokens.density_for(SurfaceRole::EdgedPanel),
            tokens.density_for(SurfaceRole::Panel)
        );
        assert_eq!(
            tokens.fill_for(SurfaceRole::EdgedPanel, 1.0),
            tokens.fill_for(SurfaceRole::Panel, 1.0)
        );
        // Решение 297: белый α 0,22, 1 pt; у панели раздела и подложки ободка нет (раздел не меняется).
        assert_eq!(
            tokens.edge_for(SurfaceRole::EdgedPanel, 1.0),
            Some(SurfaceEdge {
                color: DisplayColor::new(1.0, 1.0, 1.0, 0.22),
                width_pt: 1.0
            })
        );
        assert_eq!(tokens.edge_for(SurfaceRole::Panel, 1.0), None);
        assert_eq!(tokens.edge_for(SurfaceRole::Backing, 1.0), None);
    }

    #[test]
    fn edge_alpha_grows_with_appear_like_the_fill() {
        let tokens = SurfaceTokens::default();
        let half = tokens.edge_for(SurfaceRole::EdgedPanel, 0.5).expect("ободок");
        assert!(
            (half.color.a - 0.11).abs() < 1e-6,
            "альфа ободка × появление: {}",
            half.color.a
        );
        assert_eq!((half.color.r, half.color.g, half.color.b), (1.0, 1.0, 1.0));
        assert_eq!(
            tokens.edge_for(SurfaceRole::EdgedPanel, 0.0).expect("ободок").color.a,
            0.0
        );
        // Появление вне 0..1 не делает ободок ярче токена; толщина от появления не зависит.
        let over = tokens.edge_for(SurfaceRole::EdgedPanel, 3.0).expect("ободок");
        assert_eq!(over.color.a, 0.22);
        assert_eq!(over.width_pt, 1.0);
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
        for width_pt in [0.0, -1.0, f32::NAN, 4.5] {
            let bad_width = SurfaceTokens {
                edge: SurfaceEdgeTokens {
                    width_pt,
                    ..SurfaceEdgeTokens::default()
                },
                ..SurfaceTokens::default()
            };
            assert!(
                matches!(
                    bad_width.validate(),
                    Err(TokenError::OutOfRange {
                        name: "surface.edge.width_pt",
                        ..
                    })
                ),
                "толщина ободка {width_pt} отвергается"
            );
        }
        for alpha in [0.0, 1.2, f32::NAN] {
            let bad_alpha = SurfaceTokens {
                edge: SurfaceEdgeTokens {
                    alpha,
                    ..SurfaceEdgeTokens::default()
                },
                ..SurfaceTokens::default()
            };
            assert!(
                matches!(
                    bad_alpha.validate(),
                    Err(TokenError::OutOfRange {
                        name: "surface.edge.alpha",
                        ..
                    })
                ),
                "альфа ободка {alpha} отвергается"
            );
        }
        let bad_edge_color = SurfaceTokens {
            edge: SurfaceEdgeTokens {
                color: DisplayColor::rgb(1.4, 1.0, 1.0),
                ..SurfaceEdgeTokens::default()
            },
            ..SurfaceTokens::default()
        };
        assert_eq!(
            bad_edge_color.validate(),
            Err(TokenError::InvalidColor {
                name: "surface.edge.color"
            })
        );
    }

    #[test]
    fn config_overrides_and_rejects_unknown_fields() {
        let parsed: SurfaceTokens = serde_json::from_str(r#"{"panel_density":0.5}"#).expect("частичный конфиг");
        assert_eq!(parsed.panel_density, 0.5);
        assert_eq!(parsed.backing_density, 0.88);
        assert!(serde_json::from_str::<SurfaceTokens>(r#"{"density":0.5}"#).is_err());
        let edge: SurfaceTokens =
            serde_json::from_str(r#"{"edge":{"width_pt":1.5,"alpha":0.3}}"#).expect("частичный ободок");
        assert_eq!((edge.edge.width_pt, edge.edge.alpha), (1.5, 0.3));
        assert_eq!(edge.edge.color, SurfaceEdgeTokens::default().color);
        assert!(serde_json::from_str::<SurfaceTokens>(r#"{"edge":{"opacity":0.3}}"#).is_err());
    }
}
