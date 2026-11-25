//! Белая точка как «температура + Duv» и привязка цветности к кривой Планка.
//!
//! Нужно для баланса белого «температура + оттенок» (ручная белая, источники белого, белая вспышки). Между модулями белая сцены
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

/// Наибольшие майреды таблицы Робертсона (самая красная линия, ≈ 1667 К): дальше перевод температуры в цветность
/// невозможен. Нужна тем, кто сдвигает белую по майредам и обязан не выйти за таблицу.
pub fn table_mired_limit() -> f64 {
    ISOTEMPERATURE_LINES[LAST_LINE_INDEX].mired
}

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

#[cfg(test)]
#[path = "white_point_tests.rs"]
mod tests;
