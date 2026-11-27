//! Настройки диаграммы CIE (сессия 14s, решения 332–334): счёт ([`CieSettings`]) и показ ([`CieRenderSettings`]).
//!
//! Охват, в котором закодирована измеряемая картинка (сегодня — охват экрана), задаётся НЕ здесь: его даёт решение
//! прибора. Здесь — СПИСОК опорных контуров (решение 333): они рисуются ради сравнения и могут отличаться от охвата
//! измерения (будущее видео в Rec.2020 с контуром P3).

use scope_contract::ChromaticityGamut;
use serde::{Deserialize, Serialize};

use crate::cie_chart::ChartWindow;
use crate::grid::GridError;
use crate::sampling::SampleStride;
use crate::vectorscope_geometry::{MAX_VECTORSCOPE_SIZE, MIN_VECTORSCOPE_SIZE};

/// Наибольшая линейная яркость порога «слишком темно»: выше — отбрасывался бы заметный свет.
pub const MAX_CIE_MIN_LUMINANCE: f32 = 0.1;
/// Наименьшее и наибольшее число точек локуса чёрного тела.
pub const MIN_PLANCKIAN_SAMPLES: u32 = 2;
/// См. [`MIN_PLANCKIAN_SAMPLES`]; предел выбран так, чтобы вся сетка влезла в `MAX_GRATICULE_PRIMITIVES`.
pub const MAX_PLANCKIAN_SAMPLES: u32 = 48;
/// Наименьшая и наибольшая сторона окна диаграммы в единицах шкалы.
pub const MIN_WINDOW_EXTENT: f32 = 0.2;
/// См. [`MIN_WINDOW_EXTENT`].
pub const MAX_WINDOW_EXTENT: f32 = 4.0;
/// Наибольший вынос подписи охвата от вершины, в долях картинки.
pub const MAX_CIE_LABEL_OFFSET: f32 = 0.25;

/// Ошибка настроек диаграммы CIE: у каждой причины свой вариант.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CieSettingsError {
    /// Сторона сетки вне допустимых.
    #[error("сторона сетки CIE {size} вне {MIN_VECTORSCOPE_SIZE}..={MAX_VECTORSCOPE_SIZE}")]
    SizeOutOfRange {
        /// Недопустимая сторона.
        size: u32,
    },
    /// Порог яркости вне `0..=MAX_CIE_MIN_LUMINANCE` или не число.
    #[error("порог яркости CIE {value} вне 0..={MAX_CIE_MIN_LUMINANCE}")]
    MinLuminanceOutOfRange {
        /// Недопустимое значение.
        value: f32,
    },
    /// Окно диаграммы не конечно или его сторона вне допустимой.
    #[error("окно диаграммы `{diagram}`: левая {left}, нижняя {bottom}, сторона {extent} недопустимы")]
    WindowInvalid {
        /// Какая шкала.
        diagram: &'static str,
        /// Левая граница.
        left: f32,
        /// Нижняя граница.
        bottom: f32,
        /// Сторона.
        extent: f32,
    },
    /// Один и тот же охват в списке контуров дважды.
    #[error("охват {0:?} в списке контуров CIE дважды")]
    DuplicateGamut(ChromaticityGamut),
    /// Одна и та же метка в списке дважды.
    #[error("метка {0:?} в списке меток CIE дважды")]
    DuplicateMark(CieMark),
    /// Точек локуса чёрного тела вне `MIN..=MAX`.
    #[error("точек локуса чёрного тела {samples} вне {MIN_PLANCKIAN_SAMPLES}..={MAX_PLANCKIAN_SAMPLES}")]
    PlanckianSamplesOutOfRange {
        /// Недопустимое число.
        samples: u32,
    },
    /// Число показа (прозрачность, вынос) вне допустимого или не число.
    #[error("настройка CIE `{field}` {value} вне допустимого")]
    ValueOutOfRange {
        /// Имя поля.
        field: &'static str,
        /// Значение.
        value: f32,
    },
    /// Форма сетки превышает бюджет.
    #[error(transparent)]
    Grid(#[from] GridError),
}

/// Метка диаграммы сверх контуров охвата (решение 333: «точка белого и кривая чёрного тела — настройки»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CieMark {
    /// Кружок на белой точке измеряемого охвата (D65) и подпись.
    WhitePoint,
    /// Кривая чёрного тела (локус Планка) 1667…25000 К, считается из тех же функций сложения цветов, что спектральный локус.
    PlanckianLocus,
}

/// Цвет следа диаграммы (владелец выбирает по листу).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CieTraceColour {
    /// Белый след.
    White,
    /// Цвет точки следа — её собственная цветность, показанная цветом экрана нормированной яркости.
    ByChromaticity,
}

/// Заливка внутри спектрального локуса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CieLocusFill {
    /// Только контур.
    Outline,
    /// Контур и бледная заливка цветностью каждой точки.
    FaintColour,
}

/// Настройки счёта диаграммы CIE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CieSettings {
    /// Сторона квадратной сетки. `[estimate]` 512, как у вектороскопа.
    pub size: u32,
    /// Линейная яркость Y (1 — белый экрана), ниже которой пиксель не считается: у почти чёрного кода цветность —
    /// шум 10-битного квантования и шума теней. 0,01 (1 %, ≈ код 100 из 1023) — выбор владельца по листу тёмной сцены
    /// (решение 338): без порога шум теней раскидан по всему треугольнику и ложно упирается в край охвата.
    pub min_luminance: f32,
    /// Прореживание входа по обеим осям. 2 — каждый второй пиксель, как у вектороскопа: весь кадр идёт в одну плоскость.
    pub sample_stride: SampleStride,
    /// Окно шкалы xy 1931.
    pub xy_window: ChartWindow,
    /// Окно шкалы u′v′ 1976.
    pub uv_window: ChartWindow,
}

impl Default for CieSettings {
    fn default() -> Self {
        Self {
            size: 512,
            min_luminance: 0.01,
            sample_stride: SampleStride::EVERY_SECOND_PIXEL,
            xy_window: ChartWindow::XY_DEFAULT,
            uv_window: ChartWindow::UV_DEFAULT,
        }
    }
}

impl CieSettings {
    /// Проверка значений (бюджет сетки проверяет геометрия правила).
    pub fn validate(&self) -> Result<(), CieSettingsError> {
        if !(MIN_VECTORSCOPE_SIZE..=MAX_VECTORSCOPE_SIZE).contains(&self.size) {
            return Err(CieSettingsError::SizeOutOfRange { size: self.size });
        }
        if !(0.0..=MAX_CIE_MIN_LUMINANCE).contains(&self.min_luminance) {
            return Err(CieSettingsError::MinLuminanceOutOfRange {
                value: self.min_luminance,
            });
        }
        for (diagram, window) in [("xy_1931", self.xy_window), ("uv_prime_1976", self.uv_window)] {
            let finite = window.left.is_finite() && window.bottom.is_finite();
            if !finite || !(MIN_WINDOW_EXTENT..=MAX_WINDOW_EXTENT).contains(&window.extent) {
                return Err(CieSettingsError::WindowInvalid {
                    diagram,
                    left: window.left,
                    bottom: window.bottom,
                    extent: window.extent,
                });
            }
        }
        Ok(())
    }
}

/// Настройки показа диаграммы CIE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CieRenderSettings {
    /// Цвет следа: цветом своей цветности — выбор владельца по листам (решение 338).
    pub trace_colour: CieTraceColour,
    /// Заливка локуса: только контур — выбор владельца по листам (решение 338).
    pub locus_fill: CieLocusFill,
    /// Непрозрачность заливки `FaintColour`. `[estimate]` 0,22.
    pub locus_fill_opacity: f32,
    /// Контуры охватов (решение 333). По умолчанию только P3; список без повторов.
    pub reference_gamuts: Vec<ChromaticityGamut>,
    /// Метки сверх контуров (решение 333). По умолчанию пусто.
    pub marks: Vec<CieMark>,
    /// Точек локуса чёрного тела (по равным шагам обратной температуры, 1667…25000 К).
    pub planckian_samples: u32,
    /// Вынос подписи охвата от его вершины наружу, в долях картинки. `[estimate]` 0,05.
    pub gamut_label_offset: f32,
    /// Поле от края картинки, в которое прижимаются подписи, в долях картинки. `[estimate]` 0,04.
    pub label_margin: f32,
    /// Радиус кружка белой точки, в долях картинки. `[estimate]` 0,012.
    pub white_mark_radius: f32,
}

impl Default for CieRenderSettings {
    fn default() -> Self {
        Self {
            trace_colour: CieTraceColour::ByChromaticity,
            locus_fill: CieLocusFill::Outline,
            locus_fill_opacity: 0.22,
            reference_gamuts: vec![ChromaticityGamut::DisplayP3],
            marks: Vec::new(),
            planckian_samples: 24,
            gamut_label_offset: 0.05,
            label_margin: 0.04,
            white_mark_radius: 0.012,
        }
    }
}

impl CieRenderSettings {
    /// Нарисована ли метка.
    pub fn has_mark(&self, mark: CieMark) -> bool {
        self.marks.contains(&mark)
    }

    /// Проверка значений: числа в допусках, в списках нет повторов.
    pub fn validate(&self) -> Result<(), CieSettingsError> {
        for (index, gamut) in self.reference_gamuts.iter().enumerate() {
            if self.reference_gamuts[..index].contains(gamut) {
                return Err(CieSettingsError::DuplicateGamut(*gamut));
            }
        }
        for (index, mark) in self.marks.iter().enumerate() {
            if self.marks[..index].contains(mark) {
                return Err(CieSettingsError::DuplicateMark(*mark));
            }
        }
        if !(MIN_PLANCKIAN_SAMPLES..=MAX_PLANCKIAN_SAMPLES).contains(&self.planckian_samples) {
            return Err(CieSettingsError::PlanckianSamplesOutOfRange {
                samples: self.planckian_samples,
            });
        }
        let ranged = [
            ("locus_fill_opacity", self.locus_fill_opacity, 1.0),
            ("gamut_label_offset", self.gamut_label_offset, MAX_CIE_LABEL_OFFSET),
            ("label_margin", self.label_margin, 0.5),
            ("white_mark_radius", self.white_mark_radius, 0.25),
        ];
        for (field, value, largest) in ranged {
            if !(0.0..=largest).contains(&value) {
                return Err(CieSettingsError::ValueOutOfRange { field, value });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_draw_only_the_p3_triangle() {
        assert_eq!(CieSettings::default().validate(), Ok(()));
        let render = CieRenderSettings::default();
        assert_eq!(render.validate(), Ok(()));
        assert_eq!(render.reference_gamuts, [ChromaticityGamut::DisplayP3]);
        assert!(render.marks.is_empty());
    }

    #[test]
    fn duplicates_and_out_of_range_values_are_typed_errors() {
        let duplicated = CieRenderSettings {
            reference_gamuts: vec![ChromaticityGamut::Srgb, ChromaticityGamut::Srgb],
            ..CieRenderSettings::default()
        };
        assert_eq!(
            duplicated.validate(),
            Err(CieSettingsError::DuplicateGamut(ChromaticityGamut::Srgb))
        );
        let marks = CieRenderSettings {
            marks: vec![CieMark::WhitePoint, CieMark::WhitePoint],
            ..CieRenderSettings::default()
        };
        assert_eq!(
            marks.validate(),
            Err(CieSettingsError::DuplicateMark(CieMark::WhitePoint))
        );
        let planck = CieRenderSettings {
            planckian_samples: 49,
            ..CieRenderSettings::default()
        };
        assert_eq!(
            planck.validate(),
            Err(CieSettingsError::PlanckianSamplesOutOfRange { samples: 49 })
        );
        let dark = CieSettings {
            min_luminance: f32::NAN,
            ..CieSettings::default()
        };
        assert!(matches!(
            dark.validate(),
            Err(CieSettingsError::MinLuminanceOutOfRange { .. })
        ));
        let small = CieSettings {
            size: 4,
            ..CieSettings::default()
        };
        assert_eq!(small.validate(), Err(CieSettingsError::SizeOutOfRange { size: 4 }));
    }
}
