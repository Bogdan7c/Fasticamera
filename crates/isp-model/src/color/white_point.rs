//! Белая точка как «температура + Duv» и привязка цветности к кривой Планка.
//!
//! Нужно для ручного баланса белого («температура + оттенок») и для нашего автобаланса,
//! которому разрешено уходить от кривой Планка лишь в узкой полосе. Между модулями белая сцены
//! ходит как [`Chromaticity`] (или [`CctDuv`]); перевод в нейтраль конкретной камеры делает
//! `DngColorProfile::neutral_for_white` по профилю этой камеры.
//!
//! # Duv
//! [`Duv`] — расстояние от точки до кривой Планка в равноконтрастном пространстве CIE 1960 UCS
//! `(u, v)`; знак по CIE: плюс — выше кривой (к зелёному), минус — ниже (к пурпурному).
//!
//! Связь с «tint» Adobe (в API не выставляется): в Lightroom/Camera Raw и в Adobe DNG SDK
//! (`dng_temperature`) оттенок — это Duv, умноженный на масштаб 3000 с обратным знаком:
//! `tint ≈ −3000 · Duv` (плюс — к пурпурному). Знак и масштаб взяты из общеизвестного описания
//! `dng_temperature` (константа `kTintScale = −3000`); исходники SDK офлайн не проверялись,
//! поэтому для вычислений здесь они не используются. Знак Duv нашего кода проверен по
//! независимому эталону colour-science (фикстура `white-point-reference`).
//!
//! # Метод
//! Робертсон 1968 (JOSA 58, стр. 1528–1535), та же таблица, что в [`super::temperature`]:
//! по майредам интерполируются точка кривой и единичный вектор изотермы; сдвиг на Duv идёт
//! вдоль изотермы — так устроен `dng_temperature::Get_xy_coord`. Реализовано по описанию
//! алгоритма, без заимствования кода.

use super::chromaticity::Chromaticity;
use super::temperature::{ISOTEMPERATURE_LINES, Kelvin, TableRange, locate_mired, ucs_from_chromaticity};

/// Знаковое расстояние от кривой Планка в CIE 1960 UCS; плюс — выше кривой (к зелёному).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct Duv(pub f64);

/// Белая точка в виде температуры и отклонения от кривой Планка.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CctDuv {
    pub temperature: Kelvin,
    pub duv: Duv,
}

/// С какой стороны точка вышла за таблицу Робертсона.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableEdge {
    /// Синее первой линии таблицы (холоднее по цвету света: > 100 000 К).
    BluerThanTable,
    /// Краснее последней линии таблицы (< ≈ 1667 К).
    RedderThanTable,
}

/// Почему перевод белой точки не удался. Ничего не прижимается молча.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum WhitePointError {
    #[error("на входе нечисловое или бесконечное значение")]
    NonFiniteInput,
    #[error("температура {kelvin} К вне таблицы Робертсона (допустимо от ≈ 1667 К, конечная и положительная)")]
    TemperatureOutOfRange { kelvin: f64 },
    #[error("цветность вне таблицы Робертсона: {edge:?}")]
    ChromaticityOutOfTable { edge: TableEdge },
    #[error("вырожденная цветность: у света знаменатель перевода в UCS не положителен")]
    DegenerateChromaticity,
    #[error("полоса вокруг кривой задана неверно: {reason}")]
    InvalidBand { reason: &'static str },
}

/// Точка кривой Планка и единичная ориентация изотермы при заданных майредах.
#[derive(Debug, Clone, Copy)]
struct LocusSample {
    u: f64,
    v: f64,
    /// Единичный вектор изотермы; у таблицы `du > 0`, `dv < 0`, поэтому `−Duv·(du, dv)` при
    /// `Duv > 0` уводит точку вверх (к большему `v`).
    direction_u: f64,
    direction_v: f64,
}

/// Индекс последней линии таблицы.
const LAST_LINE_INDEX: usize = ISOTEMPERATURE_LINES.len() - 1;

/// Единичный вектор изотермы таблицы: `(1, slope) / √(1 + slope²)`.
fn line_direction(line_index: usize) -> (f64, f64) {
    let slope = ISOTEMPERATURE_LINES[line_index].slope;
    let length = (1.0 + slope * slope).sqrt();
    (1.0 / length, slope / length)
}

/// Точка кривой и направление изотермы при `mired` из рабочего диапазона таблицы `[0, 600]`.
fn sample_locus(mired: f64) -> LocusSample {
    // Первая линия, майреды которой строго больше искомых, — правый край отрезка; крайние
    // значения садятся в первый/последний отрезок.
    let upper = ISOTEMPERATURE_LINES
        .partition_point(|line| line.mired <= mired)
        .clamp(1, LAST_LINE_INDEX);
    let (lower_line, upper_line) = (&ISOTEMPERATURE_LINES[upper - 1], &ISOTEMPERATURE_LINES[upper]);
    let upper_weight = (mired - lower_line.mired) / (upper_line.mired - lower_line.mired);
    let lower_weight = 1.0 - upper_weight;

    let (lower_du, lower_dv) = line_direction(upper - 1);
    let (upper_du, upper_dv) = line_direction(upper);
    // Интерполированный вектор короче единицы — возвращаем длину 1 (как делает эталон colour).
    let direction_u = lower_du * lower_weight + upper_du * upper_weight;
    let direction_v = lower_dv * lower_weight + upper_dv * upper_weight;
    let length = direction_u.hypot(direction_v);

    LocusSample {
        u: lower_line.u * lower_weight + upper_line.u * upper_weight,
        v: lower_line.v * lower_weight + upper_line.v * upper_weight,
        direction_u: direction_u / length,
        direction_v: direction_v / length,
    }
}

/// CIE 1960 (u, v) → xy: `x = 3u / (2u − 8v + 4)`, `y = 2v / (2u − 8v + 4)`.
fn chromaticity_from_ucs(u: f64, v: f64) -> Result<Chromaticity, WhitePointError> {
    let denominator = 2.0 * u - 8.0 * v + 4.0;
    if !denominator.is_finite() || denominator <= 0.0 {
        return Err(WhitePointError::DegenerateChromaticity);
    }
    Ok(Chromaticity {
        x: 3.0 * u / denominator,
        y: 2.0 * v / denominator,
    })
}

/// Цветность по температуре и Duv — обратная к Робертсону операция.
///
/// Допустимая температура — от ≈ 1667 К (600 майред, конец таблицы) и выше, любая конечная;
/// за пределами — [`WhitePointError::TemperatureOutOfRange`], а не прижим к краю.
pub fn chromaticity_from_cct_duv(white: CctDuv) -> Result<Chromaticity, WhitePointError> {
    if !white.temperature.0.is_finite() || !white.duv.0.is_finite() {
        return Err(WhitePointError::NonFiniteInput);
    }
    let mired = white.temperature.mired();
    let mired_range = 0.0..=ISOTEMPERATURE_LINES[LAST_LINE_INDEX].mired;
    // `T ≤ 0` даёт отрицательные майреды (или бесконечность при T = 0), вне диапазона.
    if white.temperature.0 <= 0.0 || !mired_range.contains(&mired) {
        return Err(WhitePointError::TemperatureOutOfRange {
            kelvin: white.temperature.0,
        });
    }
    let locus = sample_locus(mired);
    // Сдвиг на Duv вдоль изотермы: плюс — вверх (к зелёному).
    chromaticity_from_ucs(
        locus.u - white.duv.0 * locus.direction_u,
        locus.v - white.duv.0 * locus.direction_v,
    )
}

/// Температура и Duv по цветности.
///
/// Температура — ровно та же, что даёт `temperature_from_chromaticity` (майреды Робертсона),
/// а Duv — знаковое расстояние до точки кривой при этих майредах. В отличие от
/// `temperature_from_chromaticity`, прижим к краям таблицы здесь — ошибка.
pub fn cct_duv_from_chromaticity(white: Chromaticity) -> Result<CctDuv, WhitePointError> {
    if !white.x.is_finite() || !white.y.is_finite() {
        return Err(WhitePointError::NonFiniteInput);
    }
    let (u, v) = ucs_from_chromaticity(white).ok_or(WhitePointError::DegenerateChromaticity)?;
    let location = locate_mired(u, v).ok_or(WhitePointError::NonFiniteInput)?;
    match location.range {
        TableRange::Inside => {}
        TableRange::BeyondBlueEnd => {
            return Err(WhitePointError::ChromaticityOutOfTable {
                edge: TableEdge::BluerThanTable,
            });
        }
        TableRange::BeyondRedEnd => {
            return Err(WhitePointError::ChromaticityOutOfTable {
                edge: TableEdge::RedderThanTable,
            });
        }
    }
    let locus = sample_locus(location.mired);
    let (delta_u, delta_v) = (u - locus.u, v - locus.v);
    // Составляющая смещения вдоль изотермы; сдвиг на +Duv идёт против вектора, отсюда минус.
    let along_isotherm = -(delta_u * locus.direction_u + delta_v * locus.direction_v);
    let magnitude = delta_u.hypot(delta_v);
    let signed_duv = if along_isotherm < 0.0 { -magnitude } else { magnitude };
    Ok(CctDuv {
        temperature: Kelvin(1.0e6 / location.mired),
        duv: Duv(signed_duv),
    })
}

/// Полоса допустимых белых вокруг кривой Планка: диапазон температур и макс. |Duv|.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocusBand {
    min_temperature: Kelvin,
    max_temperature: Kelvin,
    max_abs_duv: Duv,
}

impl LocusBand {
    /// Полоса `[min_temperature, max_temperature] × [−max_abs_duv, +max_abs_duv]`.
    ///
    /// Требования: температуры конечны, `min` не ниже границы таблицы (≈ 1667 К) и не больше
    /// `max`; `max_abs_duv` конечен и неотрицателен.
    pub fn new(min_temperature: Kelvin, max_temperature: Kelvin, max_abs_duv: Duv) -> Result<Self, WhitePointError> {
        let coldest_supported = 1.0e6 / ISOTEMPERATURE_LINES[LAST_LINE_INDEX].mired;
        if !min_temperature.0.is_finite() || !max_temperature.0.is_finite() || !max_abs_duv.0.is_finite() {
            return Err(WhitePointError::NonFiniteInput);
        }
        if min_temperature.0 < coldest_supported {
            return Err(WhitePointError::InvalidBand {
                reason: "минимальная температура ниже границы таблицы Робертсона",
            });
        }
        if min_temperature.0 > max_temperature.0 {
            return Err(WhitePointError::InvalidBand {
                reason: "минимальная температура больше максимальной",
            });
        }
        if max_abs_duv.0 < 0.0 {
            return Err(WhitePointError::InvalidBand {
                reason: "максимальное |Duv| отрицательно",
            });
        }
        Ok(Self {
            min_temperature,
            max_temperature,
            max_abs_duv,
        })
    }

    pub fn min_temperature(&self) -> Kelvin {
        self.min_temperature
    }

    pub fn max_temperature(&self) -> Kelvin {
        self.max_temperature
    }

    pub fn max_abs_duv(&self) -> Duv {
        self.max_abs_duv
    }
}

/// Как температура вышла за полосу (и что было до прижима).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TemperatureClamp {
    /// Температура внутри полосы.
    WithinBand,
    /// Была ниже минимума полосы (краснее) — поднята до минимума.
    RaisedToMinimum { original: Kelvin },
    /// Была выше максимума полосы (синее) — опущена до максимума.
    LoweredToMaximum { original: Kelvin },
}

/// Как Duv вышел за полосу (и что было до прижима).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuvClamp {
    /// Duv внутри полосы.
    WithinBand,
    /// Был выше кривой дальше допустимого (слишком зелёный) — уменьшен до `+max`.
    ReducedFromAbove { original: Duv },
    /// Был ниже кривой дальше допустимого (слишком пурпурный) — увеличен до `−max`.
    RaisedFromBelow { original: Duv },
}

/// Что именно пришлось прижать при привязке к полосе.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocusClamp {
    pub temperature: TemperatureClamp,
    pub duv: DuvClamp,
}

impl LocusClamp {
    /// Прижата ли хотя бы одна из координат.
    pub fn is_any(&self) -> bool {
        self.temperature != TemperatureClamp::WithinBand || self.duv != DuvClamp::WithinBand
    }
}

/// Результат привязки белой точки к полосе.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocusConstrained {
    /// Итоговая цветность; если ничего не прижато — в точности входная.
    pub white: Chromaticity,
    /// Итоговая температура и Duv.
    pub cct_duv: CctDuv,
    pub clamped: LocusClamp,
}

/// Привязывает белую точку к полосе вокруг кривой Планка.
///
/// Точка внутри полосы возвращается без изменений (бит в бит, без прогона через обратный
/// перевод). Иначе прижимаются температура и/или Duv, а в `clamped` записано, что и на сколько
/// было. Цветность вне таблицы Робертсона — ошибка: направление «к полосе» там не определено.
pub fn constrain_to_locus(white: Chromaticity, band: LocusBand) -> Result<LocusConstrained, WhitePointError> {
    let measured = cct_duv_from_chromaticity(white)?;

    let (temperature, temperature_clamp) = if measured.temperature.0 < band.min_temperature.0 {
        (
            band.min_temperature,
            TemperatureClamp::RaisedToMinimum {
                original: measured.temperature,
            },
        )
    } else if measured.temperature.0 > band.max_temperature.0 {
        (
            band.max_temperature,
            TemperatureClamp::LoweredToMaximum {
                original: measured.temperature,
            },
        )
    } else {
        (measured.temperature, TemperatureClamp::WithinBand)
    };

    let max_duv = band.max_abs_duv.0;
    let (duv, duv_clamp) = if measured.duv.0 > max_duv {
        (Duv(max_duv), DuvClamp::ReducedFromAbove { original: measured.duv })
    } else if measured.duv.0 < -max_duv {
        (Duv(-max_duv), DuvClamp::RaisedFromBelow { original: measured.duv })
    } else {
        (measured.duv, DuvClamp::WithinBand)
    };

    let clamped = LocusClamp {
        temperature: temperature_clamp,
        duv: duv_clamp,
    };
    if !clamped.is_any() {
        return Ok(LocusConstrained {
            white,
            cct_duv: measured,
            clamped,
        });
    }
    let cct_duv = CctDuv { temperature, duv };
    Ok(LocusConstrained {
        white: chromaticity_from_cct_duv(cct_duv)?,
        cct_duv,
        clamped,
    })
}

#[cfg(test)]
#[path = "white_point_tests.rs"]
mod tests;
