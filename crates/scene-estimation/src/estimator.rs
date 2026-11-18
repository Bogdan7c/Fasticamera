//! Оценщик света сцены «в форме шейдера»: плитки статистики → [`RawEstimate`].
//!
//! # Что делает (на каждую плитку, независимо — поэтому это один поток шейдера на плитку)
//! 1. Отбор. Плитка пригодна, если непересвеченных ячеек не меньше `min_used_fraction` её ячеек
//!    (`used / (used + saturated)`), все три суммы цвета ненулевые (иначе `ln` не определён — в GLSL он
//!    дал бы `-inf`/NaN) и средняя освещённость (G, доли белого) лежит в `[min_level, max_level]`.
//! 2. Цвет плитки: средние каналов `R = sum_r/used`, `G = sum_g/(2·used)` (два зелёных), `B = sum_b/used`;
//!    лог-хроматичность `p = (ln(R/G), ln(B/G))`.
//! 3. Положение на кривой: [`CameraLocusTable::locate`] → (майред, Duv). Плитка — кандидат в серое, если она
//!    не вышла за концы таблицы и `|Duv| ≤ gray_band_duv`.
//! 4. Вес `w = used_fraction · clamp((level − min_level)/(full_weight_level − min_level), 0, 1)`: тёмные плитки
//!    шумят, а плитки с пересветом цвет уже исказили. Вес растёт линейно по освещённости до порога. Для
//!    редукции кандидат тянется к кривой Планка: `pull = w · exp(−½·(Duv/σ)²)`. Уверенность = Σ pull принятых / Σ w
//!    всех пригодных: сцена из одного цвета у границы серой полосы (Duv ≈ 0,03) не получает высокой уверенности.
//! 5. Редукция кандидатов — [`ReductionConfig`]: взвешенное среднее или (по умолчанию) окно по моде.
//!
//! # Отображение на GLSL (compute, один проход, workgroup на всю сетку 32×24 = 768 потоков)
//! | Здесь | Шейдер |
//! |---|---|
//! | `AwbStatistics::tiles()` | `readonly buffer { uvec4 sums_and_used[]; uint saturated[]; }` (тот же буфер, что пишет проход ISP) |
//! | `CameraLocusTable::segments()` | `readonly buffer { LocusSegment segments[]; }`, цикл `for i < SEGMENTS` (константа специализации) |
//! | `classify_tile` | тело `main()`: поток `gl_LocalInvocationIndex` = плитка |
//! | гистограмма весов | `shared uint bins[BINS]`, `atomicAdd` весов в фиксированной точке (вес × 2^16) |
//! | поиск пика окна | `barrier()`, поток 0 перебирает `BINS` корзин |
//! | сумма `w·mired`, `w·duv`, `w`, число | редукция subgroup/shared (как `awb_reduction` ISP), фиксированная точка |
//! | `RawEstimate` | 5 слов в `writeonly buffer`, которые читает CPU после fence слота |
//! Ни итераций до сходимости, ни f64, ни циклов, зависящих от данных: только `for` по плиткам, отрезкам и корзинам.
//! Точный порядок суммирования f32 может отличаться — сверка CPU ↔ GPU идёт с допуском, не бит-в-бит.

use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, LocusBand, chromaticity_from_cct_duv, constrain_to_locus};
use isp_model::statistics::{AwbStatistics, AwbTile, QUANT_SCALE};

use crate::config::{EstimatorConfig, MAX_HISTOGRAM_BINS, ReductionConfig};
use crate::error::EstimationError;
use crate::estimate::{EstimateOutcome, IlluminantEstimate, RawEstimate};
use crate::locus_table::CameraLocusTable;

/// Что решено про одну плитку.
#[derive(Debug, Clone, Copy)]
enum TileClass {
    /// Не годится для оценки (мало ячеек, пустой канал, слишком темно/светло).
    Unusable,
    /// Годится, но не серая (цвет далеко от кривой Планка или вне таблицы).
    Eligible { weight: f32 },
    /// Кандидат в серое.
    Gray {
        weight: f32,
        pull: f32,
        mired: f32,
        duv: f32,
    },
}

/// Решение по одной плитке (тело шейдера).
fn classify_tile(tile: &AwbTile, table: &CameraLocusTable, config: &EstimatorConfig) -> TileClass {
    let total_cells = tile.used_cells.saturating_add(tile.saturated_cells);
    if tile.used_cells == 0 || total_cells == 0 {
        return TileClass::Unusable;
    }
    let used = tile.used_cells as f32;
    let used_fraction = used / total_cells as f32;
    if used_fraction < config.min_used_fraction || tile.sum_r == 0 || tile.sum_g == 0 || tile.sum_b == 0 {
        return TileClass::Unusable;
    }
    let red = tile.sum_r as f32 / used;
    let green = tile.sum_g as f32 / (2.0 * used);
    let blue = tile.sum_b as f32 / used;
    let level = green / QUANT_SCALE;
    // Запись через отрицание: NaN тоже «непригоден» (как в GLSL `!(a >= b)`).
    if !(level >= config.min_level && level <= config.max_level) {
        return TileClass::Unusable;
    }
    let ramp_span = config.full_weight_level - config.min_level;
    let level_weight = if ramp_span > 0.0 {
        ((level - config.min_level) / ramp_span).clamp(0.0, 1.0)
    } else {
        1.0
    };
    let weight = used_fraction * level_weight;
    let position = table.locate([(red / green).ln(), (blue / green).ln()]);
    // Через переменную: NaN даёт `false` и плитка не считается серой (в GLSL — то же сравнение).
    let near_locus = position.duv.abs() <= config.gray_band_duv;
    if position.off_table || !near_locus {
        return TileClass::Eligible { weight };
    }
    // Тяга к кривой: гауссов спад веса с |Duv| (см. `EstimatorConfig::duv_pull_sigma`).
    let normalized_duv = position.duv / config.duv_pull_sigma;
    TileClass::Gray {
        weight,
        pull: weight * (-0.5 * normalized_duv * normalized_duv).exp(),
        mired: position.mired,
        duv: position.duv,
    }
}

/// Накопитель взвешенного среднего (одна редукция шейдера).
#[derive(Default)]
struct WeightedSums {
    /// Вес с тягой к кривой — для среднего, пика гистограммы и уверенности.
    pull: f32,
    mired: f32,
    duv: f32,
    tiles: u32,
}

impl WeightedSums {
    fn add(&mut self, pull: f32, mired: f32, duv: f32) {
        self.pull += pull;
        self.mired += pull * mired;
        self.duv += pull * duv;
        self.tiles += 1;
    }
}

/// Эталон оценки на CPU: статистика кадра → сырая оценка (см. модуль).
pub fn estimate_scene_light(
    statistics: &AwbStatistics,
    table: &CameraLocusTable,
    config: &EstimatorConfig,
) -> RawEstimate {
    let mut eligible_weight = 0.0_f32;
    let mut eligible_tiles = 0_u32;
    // Проход 1: пригодные плитки и (для режима окна) гистограмма весов по майредам.
    let (min_mired, max_mired) = table.mired_range();
    let bins = histogram_bins(config);
    let bin_width = (max_mired - min_mired) / bins as f32;
    let mut histogram = [0.0_f32; MAX_HISTOGRAM_BINS as usize];
    let bin_of = |mired: f32| (((mired - min_mired) / bin_width) as usize).min(bins - 1);
    for tile in statistics.tiles() {
        match classify_tile(tile, table, config) {
            TileClass::Unusable => {}
            TileClass::Eligible { weight } => {
                eligible_weight += weight;
                eligible_tiles += 1;
            }
            TileClass::Gray {
                weight, pull, mired, ..
            } => {
                eligible_weight += weight;
                eligible_tiles += 1;
                histogram[bin_of(mired)] += pull;
            }
        }
    }
    // Окно приёма по майредам: всё (взвешенное среднее) или три корзины вокруг пика гистограммы.
    let window = match config.reduction {
        ReductionConfig::WeightedMean => 0..bins,
        ReductionConfig::ModeWindow { .. } => peak_window(&histogram[..bins]),
    };
    // Проход 2: взвешенное среднее кандидатов окна.
    let mut sums = WeightedSums::default();
    for tile in statistics.tiles() {
        if let TileClass::Gray { pull, mired, duv, .. } = classify_tile(tile, table, config)
            && window.contains(&bin_of(mired))
        {
            sums.add(pull, mired, duv);
        }
    }
    let has_weight = sums.pull > 0.0;
    if sums.tiles == 0 || !has_weight {
        return RawEstimate {
            mired: 0.0,
            duv: 0.0,
            confidence: 0.0,
            accepted_tiles: 0,
            eligible_tiles,
        };
    }
    RawEstimate {
        mired: sums.mired / sums.pull,
        duv: sums.duv / sums.pull,
        confidence: if eligible_weight > 0.0 {
            (sums.pull / eligible_weight).clamp(0.0, 1.0)
        } else {
            0.0
        },
        accepted_tiles: sums.tiles,
        eligible_tiles,
    }
}

/// Сколько корзин гистограммы нужно режиму (для среднего — одна, окно всё равно «всё»).
fn histogram_bins(config: &EstimatorConfig) -> usize {
    match config.reduction {
        ReductionConfig::WeightedMean => 1,
        ReductionConfig::ModeWindow { histogram_bins } => {
            (histogram_bins as usize).clamp(3, MAX_HISTOGRAM_BINS as usize)
        }
    }
}

/// Корзины окна из трёх подряд вокруг пика (суммы по окну считаются у краёв по существующим соседям).
/// Первая из равных побеждает. Пустая гистограмма даёт окно у нулевой корзины, кандидатов в нём нет.
fn peak_window(histogram: &[f32]) -> std::ops::Range<usize> {
    let last = histogram.len() - 1;
    let window_sum = |center: usize| {
        histogram[center.saturating_sub(1)..=(center + 1).min(last)]
            .iter()
            .sum::<f32>()
    };
    let mut best_center = 0;
    let mut best_sum = f32::NEG_INFINITY;
    for center in 0..histogram.len() {
        let sum = window_sum(center);
        if sum > best_sum {
            best_sum = sum;
            best_center = center;
        }
    }
    best_center.saturating_sub(1)..(best_center + 2).min(histogram.len())
}

/// Скалярный шаг CPU: сырая оценка → белая точка в полосе вокруг кривой Планка.
///
/// Нет принятых плиток — [`EstimateOutcome::NoCandidates`]. Привязка к полосе ничего не прижимает молча:
/// что и на сколько прижато, лежит в `clamped`.
pub fn finalize_estimate(raw: RawEstimate, band: LocusBand) -> Result<EstimateOutcome, EstimationError> {
    if !raw.has_candidates() {
        return Ok(EstimateOutcome::NoCandidates {
            eligible_tiles: raw.eligible_tiles,
        });
    }
    let measured = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(1.0e6 / f64::from(raw.mired)),
        duv: Duv(f64::from(raw.duv)),
    })?;
    let constrained = constrain_to_locus(measured, band)?;
    Ok(EstimateOutcome::Estimated(IlluminantEstimate {
        white: constrained.white,
        cct_duv: constrained.cct_duv,
        confidence: raw.confidence,
        accepted_tiles: raw.accepted_tiles,
        clamped: constrained.clamped,
        raw,
    }))
}

#[cfg(test)]
#[path = "estimator_tests.rs"]
mod tests;
