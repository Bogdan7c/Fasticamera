//! Замер экспозиции «в форме шейдера»: плитки статистики → одно число «сколько света нужно».
//!
//! Без итераций и ветвлений по данным: на кадр только арифметика над плитками и четыре
//! редукции (взвешенные суммы) — так же ляжет на compute-проход, когда замер уйдёт на GPU.
//! Расчёт в f32; таблицу весов CPU считает один раз на поток ([`MeteringWeights`]).
//!
//! # Формулы
//! На плитку с `used` принятыми и `saturated` пересвеченными ячейками:
//! ```text
//! Y_used = (w_r·sum_r + w_g·sum_g/2 + w_b·sum_b) / (used · 4095)      — яркость принятых ячеек
//! Y_tile = (used·Y_used + saturated·clipped_cell_luminance) / (used + saturated)
//! ```
//! Пересвеченная ячейка получает яркость `clipped_cell_luminance` (~1,0): настоящая не меньше,
//! поэтому яркость плитки — **нижняя оценка**, а это безопасная сторона для светов.
//! Редукции по плиткам с ячейками (веса `w` из таблицы, `W = Σ w`):
//! ```text
//! mean_log2_Y   = Σ w·log2(max(Y_tile, dark_floor)) / W
//! clipped_share = Σ w·saturated/(used+saturated) / W
//! dark_share    = Σ w·[Y_tile < dark_tile_luminance] / W
//! ```
//! Требуемое произведение: `log2 P* = log2 P_frame + log2(target) − mean_log2_Y + protection_ev`.
//! Яркость линейна по экспозиции, поэтому сдвиг на `log2(target) − mean_log2_Y` — ровно то,
//! на сколько ступеней кадр надо сделать ярче или темнее. При `ClippedHighlights` и `TooDark`
//! средняя яркость недостоверна, и вместо неё берётся ограниченный шаг от произведения кадра.

use camera_contract::auto_exposure::{ExposureProduct, MeasurementQuality};
use isp_model::exposure::LuminanceWeights;
use isp_model::statistics::{AwbStatistics, AwbTile, QUANT_SCALE};

use super::config::MeteringConfig;
use super::metering_weights::MeteringWeights;
use crate::error::ConfigError;

/// Яркость, в которую замер переводит средний серый (шкала тона: `scene_middle_gray`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureTarget {
    scene_middle_gray: f32,
}

impl ExposureTarget {
    /// Цель из `ToneCurveSettings::scene_middle_gray` (число не дублируем: его передаёт вызывающий).
    pub fn new(scene_middle_gray: f32) -> Result<Self, ConfigError> {
        if scene_middle_gray.is_finite() && scene_middle_gray > 0.0 && scene_middle_gray < 1.0 {
            Ok(Self { scene_middle_gray })
        } else {
            Err(ConfigError::Invalid {
                field: "exposure_target.scene_middle_gray",
                reason: "нужно число в (0, 1)".to_owned(),
            })
        }
    }

    /// Средний серый сцены.
    pub fn scene_middle_gray(self) -> f32 {
        self.scene_middle_gray
    }
}

/// Почему замер не получился.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ExposureMeasurementError {
    /// Конфиг замера неверен.
    #[error("конфиг замера: {0}")]
    Config(#[from] ConfigError),
    /// Таблица весов построена под другую сетку, чем статистика кадра.
    #[error(
        "сетка статистики {statistics_columns}×{statistics_rows} не совпала с сеткой весов {weights_columns}×{weights_rows}"
    )]
    GridMismatch {
        /// Столбцов в статистике.
        statistics_columns: u32,
        /// Строк в статистике.
        statistics_rows: u32,
        /// Столбцов в весах.
        weights_columns: u32,
        /// Строк в весах.
        weights_rows: u32,
    },
    /// В области почти нет плиток с ячейками.
    #[error("в области замера слишком мало данных: вес плиток с ячейками {used_weight}")]
    NoUsableTiles {
        /// Вес плиток с ячейками.
        used_weight: f32,
    },
    /// Требуемое произведение вышло за конечные положительные числа.
    #[error("требуемое произведение экспозиции вне допустимых чисел")]
    ProductOutOfRange,
}

/// Итог замера кадра.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExposureMeasurement {
    /// «Выдержка × ISO», нужная сцене, чтобы средний серый лёг в цель тона.
    pub required_product: ExposureProduct,
    /// Надёжность замера.
    pub quality: MeasurementQuality,
    /// Взвешенное среднее `log2 Y` по области.
    pub mean_log2_luminance: f32,
    /// Доля (по весу) пересвеченных ячеек.
    pub clipped_fraction: f32,
    /// Доля (по весу) тёмных плиток.
    pub dark_fraction: f32,
    /// Вес плиток с ячейками (1 — вся область измерена).
    pub used_weight: f32,
    /// Поправка защиты светов, EV (<= 0), уже учтённая в `required_product`.
    pub protection_ev: f32,
}

/// Редукции по плиткам (шаг, который на GPU станет проходом редукции).
#[derive(Debug, Clone, Copy, Default)]
struct Reductions {
    used_weight: f32,
    log2_sum: f32,
    clipped_sum: f32,
    dark_sum: f32,
}

/// Замер по статистике кадра. `frame_product` — «выдержка × ISO» именно этого кадра (из его результата).
pub fn measure_exposure(
    statistics: &AwbStatistics,
    weights: &MeteringWeights,
    luminance: LuminanceWeights,
    frame_product: ExposureProduct,
    target: ExposureTarget,
    config: &MeteringConfig,
) -> Result<ExposureMeasurement, ExposureMeasurementError> {
    config.validate()?;
    if statistics.grid() != weights.grid() {
        return Err(ExposureMeasurementError::GridMismatch {
            statistics_columns: statistics.grid().columns,
            statistics_rows: statistics.grid().rows,
            weights_columns: weights.grid().columns,
            weights_rows: weights.grid().rows,
        });
    }
    let reductions = reduce_tiles(statistics.tiles(), weights.weights(), luminance, config);
    if reductions.used_weight.is_nan() || reductions.used_weight < config.min_used_weight {
        return Err(ExposureMeasurementError::NoUsableTiles {
            used_weight: reductions.used_weight,
        });
    }
    let mean_log2_luminance = reductions.log2_sum / reductions.used_weight;
    let clipped_fraction = reductions.clipped_sum / reductions.used_weight;
    let dark_fraction = reductions.dark_sum / reductions.used_weight;
    let band = classify(clipped_fraction, dark_fraction, config);
    let quality = match band {
        ClipBand::Normal => MeasurementQuality::Measured,
        ClipBand::Partial | ClipBand::Severe => MeasurementQuality::ClippedHighlights,
        ClipBand::TooDark => MeasurementQuality::TooDark,
    };
    // Защита светов — по оценке «с учётом измеренного», поэтому нужна и при частичном пересвете.
    let protection_ev = match band {
        ClipBand::Normal | ClipBand::Partial if weights.applies_highlight_protection() => {
            highlight_protection_ev(clipped_fraction, config)
        }
        _ => 0.0,
    };
    let frame_log2 = frame_product.seconds_iso().log2();
    let measured_log2 = || {
        let target_luminance = target.scene_middle_gray() * config.key_offset_ev.exp2();
        frame_log2 + f64::from(target_luminance.log2() - mean_log2_luminance + protection_ev)
    };
    let required_log2 = match band {
        ClipBand::Normal => measured_log2(),
        // Часть кадра за белым: яркость известна лишь снизу, поэтому вверх идти нельзя (иначе предельный
        // цикл на границе порога), а вниз — по измерению.
        ClipBand::Partial if clipped_fraction + dark_fraction >= config.blown_or_black_hold_fraction => frame_log2,
        ClipBand::Partial => measured_log2().min(frame_log2),
        ClipBand::Severe => frame_log2 - f64::from(config.max_step_down_ev),
        ClipBand::TooDark => frame_log2 + f64::from(config.max_step_up_ev),
    };
    let required_product = ExposureProduct::from_seconds_iso(required_log2.exp2())
        .map_err(|_| ExposureMeasurementError::ProductOutOfRange)?;
    Ok(ExposureMeasurement {
        required_product,
        quality,
        mean_log2_luminance,
        clipped_fraction,
        dark_fraction,
        used_weight: reductions.used_weight,
        protection_ev,
    })
}

/// Проход по плиткам: яркость, логарифм, доли — взвешенные суммы.
fn reduce_tiles(
    tiles: &[AwbTile],
    weights: &[f32],
    luminance: LuminanceWeights,
    config: &MeteringConfig,
) -> Reductions {
    let mut totals = Reductions::default();
    for (tile, &weight) in tiles.iter().zip(weights) {
        let cells = tile.used_cells + tile.saturated_cells;
        if cells == 0 {
            // Пустая плитка (края при неровном делении, нулевой кадр) в замере не участвует.
            continue;
        }
        let tile_luminance = tile_luminance(tile, luminance, config);
        totals.used_weight += weight;
        totals.log2_sum += weight * tile_luminance.max(config.dark_floor).log2();
        totals.clipped_sum += weight * (tile.saturated_cells as f32 / cells as f32);
        if tile_luminance < config.dark_tile_luminance {
            totals.dark_sum += weight;
        }
    }
    totals
}

/// Яркость плитки: принятые ячейки по весам яркости, пересвеченные — `clipped_cell_luminance`.
fn tile_luminance(tile: &AwbTile, luminance: LuminanceWeights, config: &MeteringConfig) -> f32 {
    let cells = (tile.used_cells + tile.saturated_cells) as f32;
    let used_luminance_sum = if tile.used_cells == 0 {
        0.0
    } else {
        // Среднее по принятым ячейкам: сумма / (ячейки · 4095); у зелёного два отсчёта на ячейку.
        let per_cell = tile.used_cells as f32 * QUANT_SCALE;
        luminance.luminance(
            tile.sum_r as f32 / per_cell,
            tile.sum_g as f32 / (2.0 * per_cell),
            tile.sum_b as f32 / per_cell,
        ) * tile.used_cells as f32
    };
    (used_luminance_sum + tile.saturated_cells as f32 * config.clipped_cell_luminance) / cells
}

/// Режим замера по пересвету и темноте.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClipBand {
    /// Пересвета нет или он мал: оценка прямая.
    Normal,
    /// Значимый пересвет: оценка есть, но вверх идти нельзя.
    Partial,
    /// Почти всё за белым: ограниченный шаг вниз.
    Severe,
    TooDark,
}

fn classify(clipped_fraction: f32, dark_fraction: f32, config: &MeteringConfig) -> ClipBand {
    if clipped_fraction >= config.severe_clipped_fraction {
        ClipBand::Severe
    } else if clipped_fraction >= config.clipped_fraction_limit {
        ClipBand::Partial
    } else if dark_fraction >= config.too_dark_fraction_limit {
        ClipBand::TooDark
    } else {
        ClipBand::Normal
    }
}

/// Линейная отрицательная поправка: 0 при доле пересвета до `highlight_protect_start`, максимум на пороге пересвета.
fn highlight_protection_ev(clipped_fraction: f32, config: &MeteringConfig) -> f32 {
    let span = config.clipped_fraction_limit - config.highlight_protect_start;
    let depth = ((clipped_fraction - config.highlight_protect_start) / span).clamp(0.0, 1.0);
    -config.highlight_protection_max_ev * depth
}

#[cfg(test)]
#[path = "measurement_tests.rs"]
mod tests;
