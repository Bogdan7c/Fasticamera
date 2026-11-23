//! Настройки правила показа (счётчики → картинка прибора). Каждое значение по умолчанию — оценка
//! `[estimate]`: окончательный вид подбирается глазами в 14b.

use serde::{Deserialize, Serialize};

/// Ошибка настроек показа: у каждой причины свой вариант.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RenderSettingsError {
    /// Число (прозрачность, яркость канала, угол, амплитуда, ширина) не конечно.
    #[error("настройка показа `{field}` не число: {value}")]
    NotFinite {
        /// Имя поля.
        field: &'static str,
        /// Значение.
        value: f32,
    },
    /// Прозрачность вне `0..=1`.
    #[error("прозрачность `{field}` {value} вне 0..=1")]
    OpacityOutOfRange {
        /// Имя поля.
        field: &'static str,
        /// Значение.
        value: f32,
    },
    /// Канал цвета вне `0..=1`.
    #[error("канал цвета `{field}` {value} вне 0..=1")]
    ColorOutOfRange {
        /// Имя поля.
        field: &'static str,
        /// Значение.
        value: f32,
    },
    /// Число попаданий насыщения равно нулю.
    #[error("число попаданий, при котором след насыщается, должно быть не меньше 1")]
    ZeroSaturationCount,
    /// Полуширина линии вне `0..=16` пикселей.
    #[error("полуширина линии сетки {value} вне 0..=16 пикселей")]
    LineHalfWidthOutOfRange {
        /// Значение.
        value: f32,
    },
    /// Угол линии кожи вне `0..360`.
    #[error("угол линии тона кожи {value}° вне 0..360")]
    SkinAngleOutOfRange {
        /// Значение.
        value: f32,
    },
    /// Амплитуда целевых цветов вне `(0, 1]`.
    #[error("амплитуда целевых цветов {value} вне (0, 1]")]
    TargetAmplitudeOutOfRange {
        /// Значение.
        value: f32,
    },
    /// Полуразмер целевой рамки вне `0.5..=32` клеток.
    #[error("полуразмер целевой рамки {value} вне 0.5..=32 клеток")]
    TargetBoxOutOfRange {
        /// Значение.
        value: f32,
    },
}

/// Настройки правила показа.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RenderSettings {
    /// Число попаданий в ячейку, при котором след насыщается (яркость 1). `[estimate]` 8: на сетке 256×256
    /// при прореживании 2 в ячейку вектороскопа попадают десятки отсчётов.
    pub trace_saturation_count: u32,
    /// Наибольшая непрозрачность следа (при насыщении). `[estimate]` 0,9.
    pub trace_opacity: f32,
    /// Цвет следа waveform (цвета экрана, прямая альфа). `[estimate]` зеленоватый, как у приборов монтажа.
    pub waveform_color: [f32; 3],
    /// Цвета следа parade по плоскостям R, G, B. `[estimate]`.
    pub parade_colors: [[f32; 3]; 3],
    /// Цвет следа вектороскопа. `[estimate]` светлый нейтральный.
    pub vectorscope_color: [f32; 3],
    /// Цвет линий сетки. `[estimate]` светлый нейтральный.
    pub graticule_color: [f32; 3],
    /// Непрозрачность линий сетки. `[estimate]` 0,45.
    pub graticule_opacity: f32,
    /// Полуширина линий сетки, пикселей картинки. `[estimate]` 0,75.
    pub graticule_half_width_px: f32,
    /// Угол линии тона кожи вектороскопа Y'CbCr, градусы против часовой от +Cb. `[estimate]` 123 (стандарт).
    pub skin_line_angle_degrees: f32,
    /// Цвет тона кожи для линии вектороскопа OKLab: sRGB, 8 бит на канал. `[estimate]` типичная светлая кожа.
    pub skin_tone_srgb: [u8; 3],
    /// Амплитуда цветов целей вектороскопа: 0,75 = полосы 75 %. `[estimate]` (стандарт).
    pub target_amplitude: f32,
    /// Полуразмер рамки цели, клеток сетки вектороскопа. `[estimate]` 3.
    pub target_box_half_cells: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            trace_saturation_count: 8,
            trace_opacity: 0.9,
            waveform_color: [0.35, 1.0, 0.45],
            parade_colors: [[1.0, 0.25, 0.25], [0.25, 1.0, 0.25], [0.35, 0.5, 1.0]],
            vectorscope_color: [0.9, 0.95, 1.0],
            graticule_color: [0.85, 0.85, 0.85],
            graticule_opacity: 0.45,
            graticule_half_width_px: 0.75,
            skin_line_angle_degrees: 123.0,
            skin_tone_srgb: [224, 172, 140],
            target_amplitude: 0.75,
            target_box_half_cells: 3.0,
        }
    }
}

impl RenderSettings {
    /// Проверка значений: ни одно не принимается молча.
    pub fn validate(&self) -> Result<(), RenderSettingsError> {
        if self.trace_saturation_count == 0 {
            return Err(RenderSettingsError::ZeroSaturationCount);
        }
        check_opacity("trace_opacity", self.trace_opacity)?;
        check_opacity("graticule_opacity", self.graticule_opacity)?;
        check_color("waveform_color", self.waveform_color)?;
        for color in self.parade_colors {
            check_color("parade_colors", color)?;
        }
        check_color("vectorscope_color", self.vectorscope_color)?;
        check_color("graticule_color", self.graticule_color)?;
        check_finite("graticule_half_width_px", self.graticule_half_width_px)?;
        if !(0.0..=16.0).contains(&self.graticule_half_width_px) {
            return Err(RenderSettingsError::LineHalfWidthOutOfRange {
                value: self.graticule_half_width_px,
            });
        }
        check_finite("skin_line_angle_degrees", self.skin_line_angle_degrees)?;
        if !(0.0..360.0).contains(&self.skin_line_angle_degrees) {
            return Err(RenderSettingsError::SkinAngleOutOfRange {
                value: self.skin_line_angle_degrees,
            });
        }
        check_finite("target_amplitude", self.target_amplitude)?;
        if !(self.target_amplitude > 0.0 && self.target_amplitude <= 1.0) {
            return Err(RenderSettingsError::TargetAmplitudeOutOfRange {
                value: self.target_amplitude,
            });
        }
        check_finite("target_box_half_cells", self.target_box_half_cells)?;
        if !(0.5..=32.0).contains(&self.target_box_half_cells) {
            return Err(RenderSettingsError::TargetBoxOutOfRange {
                value: self.target_box_half_cells,
            });
        }
        Ok(())
    }
}

fn check_finite(field: &'static str, value: f32) -> Result<(), RenderSettingsError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(RenderSettingsError::NotFinite { field, value })
    }
}

fn check_opacity(field: &'static str, value: f32) -> Result<(), RenderSettingsError> {
    check_finite(field, value)?;
    if (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(RenderSettingsError::OpacityOutOfRange { field, value })
    }
}

fn check_color(field: &'static str, color: [f32; 3]) -> Result<(), RenderSettingsError> {
    for value in color {
        check_finite(field, value)?;
        if !(0.0..=1.0).contains(&value) {
            return Err(RenderSettingsError::ColorOutOfRange { field, value });
        }
    }
    Ok(())
}
