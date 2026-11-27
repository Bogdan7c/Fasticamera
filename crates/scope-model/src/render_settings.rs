//! Настройки правила показа (счётчики → картинка прибора). Каждое значение по умолчанию — оценка
//! `[estimate]`: окончательный вид подбирается глазами в 14b.

use serde::{Deserialize, Serialize};

use crate::rgb_waveform_palette::RgbWaveformPalette;
use crate::trace_resample::MAX_TRACE_SOFTENING_PX;

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
    /// Мягкость следа вне `0..=MAX_TRACE_SOFTENING_PX` пикселей.
    #[error("мягкость следа {value} вне 0..={MAX_TRACE_SOFTENING_PX} пикселей")]
    TraceSofteningOutOfRange {
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

/// Ошибка плотности полной яркости: значение обязано лежать в `1..=MAX_FULL_DENSITY`.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("плотность полной яркости {value} вне 1..={MAX_FULL_DENSITY} или не число")]
pub struct FullDensityError {
    /// Недопустимое значение.
    pub value: f32,
}

/// Наибольшая плотность полной яркости: дальше след гаснет почти весь.
pub const MAX_FULL_DENSITY: f32 = 10_000.0;

/// С какой плотности след горит в полную силу — во сколько раз гуще, чем если бы все отсчёты легли на картинку
/// РАВНОМЕРНО (решение 266 в единицах 14r, решение 322). Равномерная плотность — отсчётов кадра на пиксель картинки
/// (у waveform и parade — на пиксель своей плоскости), поэтому яркость не зависит ни от размера кадра и шага, ни от
/// размера картинки прибора. Тип проверяет себя при создании и при чтении настроек.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "f32", into = "f32")]
pub struct FullDensity(f32);

impl FullDensity {
    /// Умолчание waveform и parade: в 5,2 раза гуще равномерного (`[estimate]`). Пересчёт решения 266 («2 % отсчётов
    /// столбца на клетку» при сетке 512 × 256 на картинке 1011 × 827) в плотность на пиксель: 0,02 · 256 строк на
    /// 827 px высоты → тот же вид, что принят в 14m, на той же картинке.
    pub const COLUMN_TRACE: Self = Self(5.2);

    /// Умолчание вектороскопа: в 46 раз гуще равномерного (`[estimate]`). Пересчёт решения 266 («0,07 % отсчётов
    /// кадра на клетку» при сетке 256² на картинке 827²): клетка — 10,45 px², 0,0007 · 827² / 10,45 ≈ 46.
    pub const POSITION_TRACE: Self = Self(46.0);

    /// Проверенная плотность.
    pub fn try_new(value: f32) -> Result<Self, FullDensityError> {
        if value.is_finite() && (1.0..=MAX_FULL_DENSITY).contains(&value) {
            Ok(Self(value))
        } else {
            Err(FullDensityError { value })
        }
    }

    /// Значение (во сколько раз гуще равномерного).
    pub fn get(self) -> f32 {
        self.0
    }
}

impl TryFrom<f32> for FullDensity {
    type Error = FullDensityError;

    fn try_from(value: f32) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<FullDensity> for f32 {
    fn from(density: FullDensity) -> Self {
        density.0
    }
}

/// Какие цели цветов рисует вектороскоп (решение 320, «как у про»): Final Cut выбирает 75 или 100 %, Resolve рисует
/// обе. Цель — рамка там, куда правило попадания кладёт цвет полосы этой амплитуды.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VectorscopeTargets {
    /// Только цели полос 75 % (`target_amplitude`).
    Bars75,
    /// Только цели полного размаха 100 %.
    Bars100,
    /// Обе: 75 % — крупные рамки, 100 % — мелкие (`full_target_box_half_cells`).
    Both,
}

impl VectorscopeTargets {
    /// Рисуются ли цели полос `target_amplitude`.
    pub fn draws_bars(self) -> bool {
        matches!(self, Self::Bars75 | Self::Both)
    }

    /// Рисуются ли цели полного размаха.
    pub fn draws_full(self) -> bool {
        matches!(self, Self::Bars100 | Self::Both)
    }
}

/// Настройки правила показа.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct RenderSettings {
    /// С какой плотности след waveform и parade горит в полную силу (см. [`FullDensity`]). `[estimate]` 5,2.
    pub column_trace_full_density: FullDensity,
    /// С какой плотности след вектороскопа горит в полную силу (см. [`FullDensity`]). `[estimate]` 46.
    pub position_trace_full_density: FullDensity,
    /// Мягкость следа, пиксели картинки: сигма гауссианы, которой сетка переносится в пиксели (решение 322,
    /// [`crate::trace_resample`]). 0 — только честная ширина клетки и пикселя. `[estimate]` 0,7 (лист 14r «A»).
    pub trace_softening_px: f32,
    /// Наибольшая непрозрачность следа (при насыщении). `[estimate]` 0,9.
    pub trace_opacity: f32,
    /// Основные цвета RGB waveform (три следа наложением; решение 265). `[estimate]` «смягчённая», «чистая» — выбором.
    pub rgb_waveform_palette: RgbWaveformPalette,
    /// Цвет следа яркостного waveform (решение 321). `[estimate]` почти белый — как след приборов Resolve и FCP.
    pub luma_trace_color: [f32; 3],
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
    /// Полуширина линии тона кожи, пикселей картинки: линия кожи — главный ориентир вектороскопа, она толще прочей
    /// сетки. `[estimate]` 1,25.
    pub skin_line_half_width_px: f32,
    /// Какие цели цветов рисует вектороскоп. `[estimate]` обе (решение 324, как Resolve).
    pub vectorscope_targets: VectorscopeTargets,
    /// Полуразмер рамки цели 100 %, клеток сетки вектороскопа. `[estimate]` 4 (мельче рамки 75 %).
    pub full_target_box_half_cells: f32,
    /// Угол линии тона кожи вектороскопа Y'CbCr, градусы против часовой от +Cb. `[estimate]` 123 (стандарт).
    pub skin_line_angle_degrees: f32,
    /// Цвет тона кожи для линии вектороскопа OKLab: sRGB, 8 бит на канал. `[estimate]` типичная светлая кожа.
    pub skin_tone_srgb: [u8; 3],
    /// Амплитуда цветов целей вектороскопа: 0,75 = полосы 75 %. `[estimate]` (стандарт).
    pub target_amplitude: f32,
    /// Полуразмер рамки цели, клеток сетки вектороскопа. `[estimate]` 6 (при сетке 512).
    pub target_box_half_cells: f32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            column_trace_full_density: FullDensity::COLUMN_TRACE,
            position_trace_full_density: FullDensity::POSITION_TRACE,
            trace_softening_px: 0.7,
            trace_opacity: 0.9,
            rgb_waveform_palette: RgbWaveformPalette::Softened,
            luma_trace_color: [0.92, 0.92, 0.92],
            parade_colors: [[1.0, 0.25, 0.25], [0.25, 1.0, 0.25], [0.35, 0.5, 1.0]],
            vectorscope_lightness: 0.70,
            graticule_color: [0.85, 0.85, 0.85],
            graticule_opacity: 0.45,
            graticule_half_width_px: 0.75,
            skin_line_half_width_px: 1.25,
            // Обе цели (решение 324): масштаб вектороскопа ставит все цели 100 % внутрь круга.
            vectorscope_targets: VectorscopeTargets::Both,
            full_target_box_half_cells: 4.0,
            skin_line_angle_degrees: 123.0,
            skin_tone_srgb: [224, 172, 140],
            target_amplitude: 0.75,
            // 6 клеток сетки 512 — тот же размер рамки на картинке, что 3 клетки сетки 256 до 14r.
            target_box_half_cells: 6.0,
        }
    }
}

impl RenderSettings {
    /// Проверка значений: ни одно не принимается молча.
    pub fn validate(&self) -> Result<(), RenderSettingsError> {
        check_opacity("trace_opacity", self.trace_opacity)?;
        check_finite("trace_softening_px", self.trace_softening_px)?;
        if !(0.0..=MAX_TRACE_SOFTENING_PX).contains(&self.trace_softening_px) {
            return Err(RenderSettingsError::TraceSofteningOutOfRange {
                value: self.trace_softening_px,
            });
        }
        check_opacity("graticule_opacity", self.graticule_opacity)?;
        check_color("luma_trace_color", self.luma_trace_color)?;
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
        check_finite("skin_line_half_width_px", self.skin_line_half_width_px)?;
        if !(0.0..=16.0).contains(&self.skin_line_half_width_px) {
            return Err(RenderSettingsError::LineHalfWidthOutOfRange {
                value: self.skin_line_half_width_px,
            });
        }
        check_finite("full_target_box_half_cells", self.full_target_box_half_cells)?;
        if !(0.5..=32.0).contains(&self.full_target_box_half_cells) {
            return Err(RenderSettingsError::TargetBoxOutOfRange {
                value: self.full_target_box_half_cells,
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
    fn full_density_accepts_one_and_above_up_to_the_ceiling() {
        for good in [1.0, 5.2, 46.0, MAX_FULL_DENSITY] {
            assert_eq!(FullDensity::try_new(good).map(FullDensity::get), Ok(good));
        }
        for bad in [0.0, 0.99, -1.0, MAX_FULL_DENSITY * 1.01, f32::NAN, f32::INFINITY] {
            assert!(FullDensity::try_new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn defaults_are_the_recalculated_decision_266_values() {
        let defaults = RenderSettings::default();
        assert_eq!(defaults.column_trace_full_density, FullDensity::COLUMN_TRACE);
        assert_eq!(defaults.position_trace_full_density, FullDensity::POSITION_TRACE);
        assert_eq!(defaults.trace_softening_px, 0.7);
        assert_eq!(defaults.validate(), Ok(()));
    }

    #[test]
    fn densities_and_softening_are_checked_when_settings_are_read_from_json() {
        let parsed: RenderSettings = serde_json::from_str(r#"{"column_trace_full_density": 8.0}"#).unwrap();
        assert_eq!(parsed.column_trace_full_density.get(), 8.0);
        assert_eq!(parsed.position_trace_full_density, FullDensity::POSITION_TRACE);
        assert!(serde_json::from_str::<RenderSettings>(r#"{"position_trace_full_density": 0.5}"#).is_err());
        let too_soft = RenderSettings {
            trace_softening_px: MAX_TRACE_SOFTENING_PX + 0.5,
            ..RenderSettings::default()
        };
        assert_eq!(
            too_soft.validate(),
            Err(RenderSettingsError::TraceSofteningOutOfRange {
                value: MAX_TRACE_SOFTENING_PX + 0.5
            })
        );
        let round_trip = serde_json::to_string(&parsed).unwrap();
        assert!(round_trip.contains("\"column_trace_full_density\":8.0"), "{round_trip}");
    }
}
