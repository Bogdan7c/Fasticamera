//! Настройки правила показа (счётчики → картинка прибора). Каждое значение по умолчанию — оценка
//! `[estimate]`: окончательный вид подбирается глазами в 14b.

use serde::{Deserialize, Serialize};

use crate::rgb_waveform_palette::RgbWaveformPalette;

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
    /// Светлота вектороскопа вне `(0, 1)`: у крайней светлоты нет цветов по кругу.
    #[error("светлота вектороскопа {value} вне (0, 1)")]
    LightnessOutOfRange {
        /// Значение.
        value: f32,
    },
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

/// Ошибка доли отсчётов: доля обязана лежать в `(0, 1]`.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("доля отсчётов {value} вне (0, 1] или не число")]
pub struct TraceShareError {
    /// Недопустимое значение.
    pub value: f32,
}

/// Доля отсчётов, при которой след достигает полной яркости (решение 266): `0 < доля ≤ 1`. Тип проверяет себя
/// при создании и при чтении настроек, поэтому недопустимая доля до правила показа не доходит.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct TraceShare(f32);

impl TraceShare {
    /// Умолчание для столбца waveform/parade: 2 % отсчётов одного столбца (`[estimate]`, решение 266).
    pub const FULL_OF_COLUMN: Self = Self(0.02);

    /// Умолчание для вектороскопа: 0,07 % отсчётов кадра (`[estimate]`, решение 266).
    pub const FULL_OF_FRAME: Self = Self(0.0007);

    /// Проверенная доля.
    pub fn try_new(value: f32) -> Result<Self, TraceShareError> {
        if value.is_finite() && value > 0.0 && value <= 1.0 {
            Ok(Self(value))
        } else {
            Err(TraceShareError { value })
        }
    }

    /// Значение доли.
    pub fn get(self) -> f32 {
        self.0
    }
}

impl TryFrom<f32> for TraceShare {
    type Error = TraceShareError;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<TraceShare> for f32 {
    fn from(share: TraceShare) -> Self {
        share.0
    }
}

/// Настройки правила показа.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RenderSettings {
    /// Доля отсчётов ОДНОГО СТОЛБЦА, при которой след waveform/parade достигает полной яркости (решение 266).
    /// Порог в попаданиях = `max(1, доля · отсчётов / столбцов)`; от числа столбцов и размера кадра зависит
    /// автоматически. `[estimate]` 0,02.
    pub trace_full_share_of_column: TraceShare,
    /// Доля отсчётов КАДРА, при которой след вектороскопа достигает полной яркости (решение 266): порог =
    /// `max(1, доля · отсчётов кадра)`. `[estimate]` 0,0007.
    pub trace_full_share_of_frame: TraceShare,
    /// Наибольшая непрозрачность следа (при насыщении). `[estimate]` 0,9.
    pub trace_opacity: f32,
    /// Основные цвета RGB waveform (три следа наложением; решение 265). `[estimate]` «смягчённая», «чистая» — выбором.
    pub rgb_waveform_palette: RgbWaveformPalette,
    /// Цвета следа parade по плоскостям R, G, B. `[estimate]`.
    pub parade_colors: [[f32; 3]; 3],
    /// Светлота вектороскопа: серое в центре и яркость цветов по кругу (решение 267): для Y'CbCr — серое R'G'B' =
    /// светлота, для OKLab — светлота `L` OKLab. `0 < L < 1`. `[estimate]` 0,70.
    pub vectorscope_lightness: f32,
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
            trace_full_share_of_column: TraceShare::FULL_OF_COLUMN,
            trace_full_share_of_frame: TraceShare::FULL_OF_FRAME,
            trace_opacity: 0.9,
            rgb_waveform_palette: RgbWaveformPalette::Softened,
            parade_colors: [[1.0, 0.25, 0.25], [0.25, 1.0, 0.25], [0.35, 0.5, 1.0]],
            vectorscope_lightness: 0.70,
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
        check_opacity("trace_opacity", self.trace_opacity)?;
        check_opacity("graticule_opacity", self.graticule_opacity)?;
        for color in self.parade_colors {
            check_color("parade_colors", color)?;
        }
        check_finite("vectorscope_lightness", self.vectorscope_lightness)?;
        if !(self.vectorscope_lightness > 0.0 && self.vectorscope_lightness < 1.0) {
            return Err(RenderSettingsError::LightnessOutOfRange {
                value: self.vectorscope_lightness,
            });
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn share_accepts_only_the_half_open_unit_interval() {
        for good in [f32::MIN_POSITIVE, 0.0007, 0.5, 1.0] {
            assert_eq!(TraceShare::try_new(good).map(TraceShare::get), Ok(good));
        }
        for bad in [0.0, -0.1, 1.0001, f32::NAN, f32::INFINITY] {
            assert!(TraceShare::try_new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn default_shares_are_the_valid_decision_values() {
        let defaults = RenderSettings::default();
        assert_eq!(defaults.trace_full_share_of_column.get(), 0.02);
        assert_eq!(defaults.trace_full_share_of_frame.get(), 0.0007);
        assert_eq!(TraceShare::try_new(0.02), Ok(TraceShare::FULL_OF_COLUMN));
        assert_eq!(TraceShare::try_new(0.0007), Ok(TraceShare::FULL_OF_FRAME));
    }

    #[test]
    fn share_is_checked_when_settings_are_read_from_json() {
        let parsed: RenderSettings = serde_json::from_str(r#"{"trace_full_share_of_column": 0.05}"#).unwrap();
        assert_eq!(parsed.trace_full_share_of_column.get(), 0.05);
        assert_eq!(parsed.trace_full_share_of_frame, TraceShare::FULL_OF_FRAME);
        assert!(serde_json::from_str::<RenderSettings>(r#"{"trace_full_share_of_frame": 0}"#).is_err());
        assert!(serde_json::from_str::<RenderSettings>(r#"{"trace_full_share_of_column": 1.5}"#).is_err());
        let round_trip = serde_json::to_string(&parsed).unwrap();
        assert!(
            round_trip.contains("\"trace_full_share_of_column\":0.05"),
            "{round_trip}"
        );
    }
}
