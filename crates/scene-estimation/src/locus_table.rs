//! Таблица кривой Планка в пространстве конкретной камеры.
//!
//! # Идея
//! Серая поверхность под светом с цветностью `w` даёт на камере отклик, равный её **нейтрали**
//! `N(w) = (R, G = 1, B)` (DNG: `DngColorProfile::neutral_for_white`). Статистика AWB считается ДО баланса
//! белого, значит, цвет «серой» плитки в логарифмических координатах хроматичности
//! `p = (ln(R/G), ln(B/G))` лежит ровно на кривой `N(w)`. Логарифм делает влияние яркости нулевым
//! (яркость — сдвиг по каналам одинаково) и превращает усиления баланса в сдвиги.
//!
//! # Что в таблице
//! Точки `P_i` кривой Планка (Duv = 0) в этих координатах для температур, равномерных по **майредам**
//! (`1e6 / T`, шаг одинаков для глаза и для интерполяции DNG), и вектор `n_i` — как точка уходит при
//! росте Duv на единицу (снимается пробой `+duv_probe`). На каждый отрезок `i → i+1` заранее считается
//! матрица, обратная `[D n]` (`D = P_{i+1} − P_i`, `n` — среднее двух нормалей):
//! `p − P_i = s·D + duv·n` решается одним умножением 2×2 → `(s, duv)`; майред точки `= m_i + s·Δm`.
//! Отрезки лежат в плоском [`LocusSegment`] (`#[repr(C)]`, 12 f32): тот же массив уйдёт шейдеру
//! как storage-буфер, на кадр ничего не пересчитывается (решение 68).
//!
//! # Раскладка для шейдера (std430)
//! 12 подряд идущих f32; в GLSL — только `vec2` и `float` (без `vec4`, чтобы не появилось выравнивание по 16):
//! ```text
//! struct LocusSegment { vec2 origin; vec2 direction; vec2 normal; vec2 inverse_row0; vec2 inverse_row1;
//!                       float mired_start; float mired_span; }   // 48 байт, выравнивание 8
//! ```
//! Размер и порядок полей закреплены тестом `segment_layout_matches_the_shader_struct`.

use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::dng::DngColorProfile;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, LocusBand, chromaticity_from_cct_duv};

use crate::config::LocusTableConfig;
use crate::error::LocusTableError;

/// Один отрезок кривой в координатах камеры (плоская раскладка для шейдера).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocusSegment {
    /// Начало отрезка `P_i` в `(ln(R/G), ln(B/G))`.
    pub origin: [f32; 2],
    /// Вектор отрезка `D = P_{i+1} − P_i`.
    pub direction: [f32; 2],
    /// Сдвиг точки при росте Duv на единицу (в тех же координатах).
    pub normal: [f32; 2],
    /// Обратная матрица `[D n]`, по строкам: `s = i0·x + i1·y`, `duv = i2·x + i3·y`.
    pub inverse: [f32; 4],
    /// Майред начала отрезка.
    pub mired_start: f32,
    /// Приращение майред вдоль отрезка (`> 0`).
    pub mired_span: f32,
}

/// Где точка относительно кривой.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LocusPosition {
    /// Майред ближайшей точки кривой (в пределах таблицы).
    pub mired: f32,
    /// Знаковое расстояние до кривой (Duv, плюс — к зелёному).
    pub duv: f32,
    /// Точка за краем таблицы (теплее или холоднее неё): положение ненадёжно.
    pub off_table: bool,
}

/// Таблица локуса одной камеры (одного потока).
#[derive(Debug, Clone, PartialEq)]
pub struct CameraLocusTable {
    segments: Vec<LocusSegment>,
    min_mired: f32,
    max_mired: f32,
    band: LocusBand,
}

/// Лог-хроматичность нейтрали: `(ln(R), ln(B))` при `G = 1`.
fn log_chroma(neutral: [f64; 3]) -> [f64; 2] {
    [(neutral[0] / neutral[1]).ln(), (neutral[2] / neutral[1]).ln()]
}

impl CameraLocusTable {
    /// Строит таблицу по профилю цвета потока. Вызывается один раз на поток (f64, CPU).
    ///
    /// `max_abs_duv` попадает в [`LocusBand`] таблицы: итоговая белая обязана остаться в полосе.
    pub fn build(
        profile: &DngColorProfile,
        config: &LocusTableConfig,
        max_abs_duv: f64,
    ) -> Result<Self, LocusTableError> {
        config.validate()?;
        let band = LocusBand::new(
            Kelvin(config.min_temperature_kelvin),
            Kelvin(config.max_temperature_kelvin),
            Duv(max_abs_duv),
        )
        .map_err(LocusTableError::Band)?;
        let min_mired = Kelvin(config.max_temperature_kelvin).mired();
        let max_mired = Kelvin(config.min_temperature_kelvin).mired();
        let count = config.segments as usize;
        let mired_at = |index: usize| min_mired + (max_mired - min_mired) * index as f64 / count as f64;

        // Точки кривой и их нормали (производная по Duv), по одной на границу отрезка.
        let mut points = Vec::with_capacity(count + 1);
        let mut normals = Vec::with_capacity(count + 1);
        for index in 0..=count {
            let kelvin = 1.0e6 / mired_at(index);
            let on_locus = neutral_log_chroma(profile, kelvin, 0.0)?;
            let probed = neutral_log_chroma(profile, kelvin, config.duv_probe)?;
            points.push(on_locus);
            normals.push([
                (probed[0] - on_locus[0]) / config.duv_probe,
                (probed[1] - on_locus[1]) / config.duv_probe,
            ]);
        }

        let mut segments = Vec::with_capacity(count);
        for index in 0..count {
            let origin = points[index];
            let direction = [points[index + 1][0] - origin[0], points[index + 1][1] - origin[1]];
            let normal = [
                0.5 * (normals[index][0] + normals[index + 1][0]),
                0.5 * (normals[index][1] + normals[index + 1][1]),
            ];
            let determinant = direction[0] * normal[1] - direction[1] * normal[0];
            if !determinant.is_finite() || determinant == 0.0 {
                return Err(LocusTableError::DegenerateSegment { index });
            }
            let inverse = [
                normal[1] / determinant,
                -normal[0] / determinant,
                -direction[1] / determinant,
                direction[0] / determinant,
            ];
            // f64 → f32: сужение намеренное, дальше всё в форме шейдера.
            segments.push(LocusSegment {
                origin: origin.map(|value| value as f32),
                direction: direction.map(|value| value as f32),
                normal: normal.map(|value| value as f32),
                inverse: inverse.map(|value| value as f32),
                mired_start: mired_at(index) as f32,
                mired_span: (mired_at(index + 1) - mired_at(index)) as f32,
            });
        }
        Ok(Self {
            segments,
            min_mired: min_mired as f32,
            max_mired: max_mired as f32,
            band,
        })
    }

    /// Отрезки (для шейдера — как есть, `#[repr(C)]`).
    pub fn segments(&self) -> &[LocusSegment] {
        &self.segments
    }

    /// Диапазон майред таблицы `(холоднее, теплее)`.
    pub fn mired_range(&self) -> (f32, f32) {
        (self.min_mired, self.max_mired)
    }

    /// Полоса допустимых белых: диапазон температур таблицы и допуск по Duv.
    pub fn band(&self) -> LocusBand {
        self.band
    }

    /// Положение точки `(ln(R/G), ln(B/G))` относительно кривой — тело цикла по отрезкам, как оно
    /// будет в шейдере: для каждого отрезка `(s, duv)` одним умножением 2×2; выбирается отрезок без
    /// выхода за концы (`s ∈ [0, 1]`), а при нескольких — с наименьшим |duv|; если таких нет —
    /// с наименьшим выходом.
    ///
    /// `off_table` — только когда лучший отрезок крайний и точка вышла за ЕГО внешний конец (теплее или
    /// холоднее всей таблицы). Выход внутри таблицы (щель между лучами соседних отрезков на выпуклой
    /// стороне изгиба при большом |Duv|) — не «вне таблицы»: `s` просто прижимается к отрезку.
    pub fn locate(&self, point: [f32; 2]) -> LocusPosition {
        let last = self.segments.len() - 1;
        let mut best_overshoot = f32::INFINITY;
        let mut best_duv = f32::INFINITY;
        let mut best_s = 0.0_f32;
        let mut best_index = 0;
        for (index, segment) in self.segments.iter().enumerate() {
            let (dx, dy) = (point[0] - segment.origin[0], point[1] - segment.origin[1]);
            let s = segment.inverse[0] * dx + segment.inverse[1] * dy;
            let duv = segment.inverse[2] * dx + segment.inverse[3] * dy;
            // Выход за концы отрезка в долях его длины (0 — внутри).
            let overshoot = (-s).max(s - 1.0).max(0.0);
            let better = overshoot < best_overshoot || (overshoot == best_overshoot && duv.abs() < best_duv.abs());
            if better {
                best_overshoot = overshoot;
                best_duv = duv;
                best_s = s;
                best_index = index;
            }
        }
        let segment = &self.segments[best_index];
        LocusPosition {
            mired: segment.mired_start + best_s.clamp(0.0, 1.0) * segment.mired_span,
            duv: best_duv,
            off_table: (best_index == 0 && best_s < 0.0) || (best_index == last && best_s > 1.0),
        }
    }

    /// Лог-хроматичность нейтрали для белой цветности — вход `locate` (удобство тестов и диагностики).
    pub fn log_chroma_of(profile: &DngColorProfile, white: Chromaticity) -> Option<[f32; 2]> {
        let neutral = profile.neutral_for_white(white).ok()?.values();
        Some(log_chroma(neutral).map(|value| value as f32))
    }
}

/// Лог-хроматичность нейтрали камеры для белой `(T, Duv)`.
fn neutral_log_chroma(profile: &DngColorProfile, kelvin: f64, duv: f64) -> Result<[f64; 2], LocusTableError> {
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(duv),
    })
    .map_err(|source| LocusTableError::WhitePoint { kelvin, source })?;
    let neutral = profile
        .neutral_for_white(white)
        .map_err(|source| LocusTableError::Neutral { kelvin, source })?;
    Ok(log_chroma(neutral.values()))
}

#[cfg(test)]
#[path = "locus_table_tests.rs"]
mod tests;
