//! Раскладка кнопки белого в ряду затвора (white-balance 02, решение WB35).
//!
//! Кнопка — стеклянный круг того же диаметра, что кнопка меню Controls, на высоте центра затвора. По горизонтали
//! она стоит на `offset_from_shutter_pt` от центра затвора: вправо (между затвором и кнопкой меню) или влево
//! (зеркально), как скажет `WhiteScreenConfig::button_side`. Число — `[owner]` (отмечено владельцем на снимке
//! экрана): середина между краем затвора и кнопкой меню.
//!
//! Проверка [`WhiteButtonLayout::check_clearance`] ловит кнопку, которая налезла бы на затвор, кнопку меню,
//! миниатюру или капсулу режимов, на ОБЕИХ сторонах: сторона — настройка, и конфиг с неудачным числом не должен
//! проходить только потому, что сейчас выбрана «удобная» сторона.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::white_config::WhiteButtonSide;
use crate::widgets_config::{BottomLayout, RoundButtonLayout};

/// Ошибка раскладки кнопки белого.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum WhiteButtonLayoutError {
    /// Число должно быть конечным и > 0.
    #[error("значение раскладки кнопки белого {field} должно быть конечным и > 0, получено {value}")]
    NotPositive { field: &'static str, value: f64 },
    /// Число должно быть конечным и >= 0.
    #[error("значение раскладки кнопки белого {field} должно быть конечным и >= 0, получено {value}")]
    Negative { field: &'static str, value: f64 },
    /// Кнопка подошла к соседу ближе допустимого зазора (отрицательный зазор — наезд).
    #[error("кнопка белого {side:?}: до «{neighbour}» зазор {gap_pt:.1} pt, нужно не меньше {minimum_pt:.1} pt")]
    TooCloseTo {
        side: WhiteButtonSide,
        neighbour: &'static str,
        gap_pt: f64,
        minimum_pt: f64,
    },
    /// Кнопка ушла за левый край экрана.
    #[error("кнопка белого {side:?} уходит за край экрана: левый край {left_pt:.1} pt")]
    OffScreen { side: WhiteButtonSide, left_pt: f64 },
}

/// Раскладка кнопки белого.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhiteButtonLayout {
    /// Расстояние от центра затвора до центра кнопки по x, pt. `[owner]` 92: середина между краем затвора и кнопкой
    /// меню (затвор 220, кнопка меню ≈ 383,6 → центр кнопки ≈ 312). Слева — зеркально (128).
    pub offset_from_shutter_pt: f64,
    /// Наименьший зазор между краями кнопки и соседа, pt. `[estimate]` 8: не меньше, чтобы зоны касания и стекло
    /// не слипались.
    pub min_gap_pt: f64,
    /// Наибольшая ширина числа Кельвинов внутри круга, pt. `[estimate]` 36 при круге 44 (хорда на высоте цифр ≈ 39): четыре цифры «5600» помещаются без сжатия, «12000» шире, поэтому число
    /// сжимается равномерно, а не вылезает за стекло.
    pub number_max_width_pt: f64,
}

impl Default for WhiteButtonLayout {
    fn default() -> Self {
        Self {
            offset_from_shutter_pt: 92.0,
            min_gap_pt: 8.0,
            number_max_width_pt: 36.0,
        }
    }
}

impl WhiteButtonLayout {
    /// Центр кнопки по x для выбранной стороны (системa эталонного экрана).
    pub fn center_x_pt(&self, shutter_center_x_pt: f64, side: WhiteButtonSide) -> f64 {
        match side {
            WhiteButtonSide::Right => shutter_center_x_pt + self.offset_from_shutter_pt,
            WhiteButtonSide::Left => shutter_center_x_pt - self.offset_from_shutter_pt,
        }
    }

    /// Простая проверка чисел самой раскладки.
    pub fn validate(&self) -> Result<(), WhiteButtonLayoutError> {
        for (field, value) in [
            ("white_button.offset_from_shutter_pt", self.offset_from_shutter_pt),
            ("white_button.number_max_width_pt", self.number_max_width_pt),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(WhiteButtonLayoutError::NotPositive { field, value });
            }
        }
        if !self.min_gap_pt.is_finite() || self.min_gap_pt < 0.0 {
            return Err(WhiteButtonLayoutError::Negative {
                field: "white_button.min_gap_pt",
                value: self.min_gap_pt,
            });
        }
        Ok(())
    }

    /// Не наезжает ли кнопка (обе стороны) на затвор, кнопку меню, миниатюру и капсулу режимов.
    /// `shutter_diameter_pt` — токен затвора (в раскладке виджетов его нет); диаметр кнопки — как у кнопки меню.
    pub fn check_clearance(
        &self,
        bottom: &BottomLayout,
        shutter_diameter_pt: f64,
    ) -> Result<(), WhiteButtonLayoutError> {
        let radius_pt = bottom.menu_button.diameter_pt * 0.5;
        for side in [WhiteButtonSide::Right, WhiteButtonSide::Left] {
            let center_x_pt = self.center_x_pt(bottom.shutter_center_x_pt, side);
            let center_y_pt = bottom.shutter_center_y_pt;
            let circle_gap = |other_x: f64, other_y: f64, other_radius: f64| {
                (center_x_pt - other_x).hypot(center_y_pt - other_y) - radius_pt - other_radius
            };
            let round_gap = |button: &RoundButtonLayout| {
                circle_gap(button.center_x_pt, button.center_y_pt, button.diameter_pt * 0.5)
            };
            // Капсула режимов — прямоугольник: зазор считается от ближайшей точки прямоугольника до центра кнопки.
            let capsule_gap = {
                let dx = ((center_x_pt - bottom.capsule_center_x_pt).abs() - bottom.capsule_width_pt * 0.5).max(0.0);
                let dy = ((center_y_pt - bottom.capsule_center_y_pt).abs() - bottom.capsule_height_pt * 0.5).max(0.0);
                dx.hypot(dy) - radius_pt
            };
            let neighbours = [
                (
                    "затвор",
                    circle_gap(
                        bottom.shutter_center_x_pt,
                        bottom.shutter_center_y_pt,
                        shutter_diameter_pt * 0.5,
                    ),
                ),
                ("кнопка меню", round_gap(&bottom.menu_button)),
                ("миниатюра", round_gap(&bottom.thumbnail)),
                ("капсула режимов", capsule_gap),
            ];
            for (neighbour, gap_pt) in neighbours {
                if gap_pt < self.min_gap_pt {
                    return Err(WhiteButtonLayoutError::TooCloseTo {
                        side,
                        neighbour,
                        gap_pt,
                        minimum_pt: self.min_gap_pt,
                    });
                }
            }
            let left_pt = center_x_pt - radius_pt;
            if left_pt < 0.0 {
                return Err(WhiteButtonLayoutError::OffScreen { side, left_pt });
            }
        }
        Ok(())
    }
}
