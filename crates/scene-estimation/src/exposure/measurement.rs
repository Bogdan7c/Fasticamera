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
//! measured_Y    = Σ w·(used·Y_used/(used+saturated)) / W        — вклад принятых ячеек
//! clipped_Y     = Σ w·(saturated·clipped_cell_luminance/(used+saturated)) / W — вклад пересвета (нижняя оценка)
//! log_Y         = Σ w·log2(max(Y_tile, dark_floor)) / W         — среднее логарифмов (геометрическое)
//! clipped_share = Σ w·saturated/(used+saturated) / W
//! dark_share    = Σ w·[Y_tile < dark_tile_luminance] / W
//! ```
//! Цель с защитой светов: `T' = target · 2^protection_ev`. Способ — `MeteringConfig::scene_average`
//! (решение владельца 133):
//! * по умолчанию — обычное среднее света с предохранителем пересвета (решение 134):
//!   ```text
//!   m     = max(measured_Y, dark_floor)                — растёт вместе с выдержкой
//!   c     = min(clipped_Y, k · T'),  k = clipped_max_share_of_target < 1   — белое остаётся белым
//!   log2 P* = log2 P_frame + log2(T' − c) − log2(m)    — решение уравнения m · (P*/P) + c = T'
//!   ```
//!   Без предела всегда-белое место (окно, солнце) не давало бы равновесия: его нижняя оценка не убывает с
//!   выдержкой, и экспозиция уходила бы в ноль. Пересвет не растёт с выдержкой, поэтому он вычитается из цели,
//!   а не делит её: так замер не зависит от выдержки кадра и компенсация EV ±1 даёт ×2 и ×½;
//! * лог-среднее (прежний способ 08a…13b, для A/B): `log2 P* = log2 P_frame + log2(T') − log_Y`.
//!
//! Обычное среднее не меньше геометрического, поэтому на неровной сцене оно просит меньше света; на ровной
//! они совпадают. При `ClippedHighlights` и `TooDark` средняя яркость недостоверна, и вместо неё берётся
//! ограниченный шаг от произведения кадра.

use camera_contract::auto_exposure::{ExposureProduct, MeasurementQuality};
use isp_model::exposure::LuminanceWeights;
use isp_model::statistics::{AwbStatistics, AwbTile, QUANT_SCALE};

use super::config::{MeteringConfig, SceneAverage};
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
    /// `log2` яркости области способом из `MeteringConfig::scene_average` (по умолчанию — обычное среднее света).
    pub scene_log2_luminance: f32,
    /// Доля (по весу) пересвеченных ячеек.
    pub clipped_fraction: f32,
    /// Доля (по весу) тёмных плиток.
    pub dark_fraction: f32,
    /// Вес плиток с ячейками (1 — вся область измерена).
    pub used_weight: f32,
    /// Поправка защиты светов, EV (<= 0), уже учтённая в `required_product`.
    pub protection_ev: f32,
    /// ЛИНЕЙНАЯ средняя яркость области: `Σw·Y_tile / W` (пересвеченные ячейки — по нижней оценке
    /// `clipped_cell_luminance`, БЕЗ предела `clipped_max_share_of_target` и без защиты светов; не ниже
    /// `dark_floor`). Не зависит от способа усреднения `scene_average`, поэтому годится для сложения света
    /// (расчёт вспышки, 14d): `линейный замер = произведение кадра × цель / эта яркость`.
    pub mean_luminance: f32,
    /// Средний уровень ЗЕЛЁНОГО (нормированный отсчёт ПОСЛЕ коррекции виньетирования, до баланса белого) по области
    /// с теми же весами; пересвеченные (по сырому отсчёту) ячейки считаются белыми (1,0). Принятые ячейки могут дать
    /// значение выше 1,0: в тёмных углах усиление калибровки поднимает несжатый сырой свет выше белого, суммы
    /// статистики это сохраняют (запас `sum_headroom`), а замер не обрезает. Шкала совпадает со шкалой сырых отсчётов RAW после
    /// вычитания чёрного, поэтому по нему сравнивают кадр статистики с самим RAW (проверка света вспышки).
    pub mean_green_level: f32,
}

/// Редукции по плиткам (шаг, который на GPU станет проходом редукции).
#[derive(Debug, Clone, Copy, Default)]
struct Reductions {
    used_weight: f32,
    /// Σ w·(вклад принятых ячеек в яркость плитки) — измеримая часть обычного среднего.
    measured_luminance_sum: f32,
    /// Σ w·(вклад пересвеченных ячеек в яркость плитки) — нижняя оценка, у обычного среднего ограничена.
    clipped_luminance_sum: f32,
    /// Σ w·log2(max(Y_tile, пол)) — для лог-среднего.
    log2_sum: f32,
    clipped_sum: f32,
    dark_sum: f32,
    /// Σ w·(средний зелёный плитки, пересвет = 1,0) — для `mean_green_level`.
    green_sum: f32,
}

/// Фаза замера: от неё зависят ограниченные шаги при пересвете и темноте.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasurementPhase {
    /// У потока ещё не было ни одной надёжной (`Measured`) оценки: шаги `cold_start_max_step_*`.
    ColdStart,
    /// Надёжная оценка потока уже была: обычные шаги `max_step_*`.
    Established,
}

/// Замер в установившейся фазе (обычные шаги).
pub fn measure_exposure(
    statistics: &AwbStatistics,
    weights: &MeteringWeights,
    luminance: LuminanceWeights,
    frame_product: ExposureProduct,
    target: ExposureTarget,
    config: &MeteringConfig,
) -> Result<ExposureMeasurement, ExposureMeasurementError> {
    measure_exposure_in_phase(
        statistics,
        weights,
        luminance,
        frame_product,
        target,
        config,
        MeasurementPhase::Established,
    )
}

/// Замер по статистике кадра. `frame_product` — «выдержка × ISO» именно этого кадра (из его результата).
pub fn measure_exposure_in_phase(
    statistics: &AwbStatistics,
    weights: &MeteringWeights,
    luminance: LuminanceWeights,
    frame_product: ExposureProduct,
    target: ExposureTarget,
    config: &MeteringConfig,
    phase: MeasurementPhase,
) -> Result<ExposureMeasurement, ExposureMeasurementError> {
    config.validate()?;
    let (step_down_ev, step_up_ev) = match phase {
        MeasurementPhase::ColdStart => (config.cold_start_max_step_down_ev, config.cold_start_max_step_up_ev),
        MeasurementPhase::Established => (config.max_step_down_ev, config.max_step_up_ev),
    };
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
    let target_luminance = target.scene_middle_gray() * config.key_offset_ev.exp2();
    // Яркость сцены — после защиты: предохранитель пересвета обычного среднего считается от защищённой цели.
    let scene = scene_luminance(&reductions, target_luminance * protection_ev.exp2(), config);
    let frame_log2 = frame_product.seconds_iso().log2();
    let measured_log2 = || frame_log2 + f64::from(scene.exposure_shift_log2);
    let required_log2 = match band {
        ClipBand::Normal => measured_log2(),
        // Часть кадра за белым: яркость известна лишь снизу, поэтому вверх идти нельзя (иначе предельный
        // цикл на границе порога), а вниз — по измерению.
        ClipBand::Partial if clipped_fraction + dark_fraction >= config.blown_or_black_hold_fraction => frame_log2,
        ClipBand::Partial => measured_log2().min(frame_log2),
        ClipBand::Severe => frame_log2 - f64::from(step_down_ev),
        ClipBand::TooDark => frame_log2 + f64::from(step_up_ev),
    };
    let required_product = ExposureProduct::from_seconds_iso(required_log2.exp2())
        .map_err(|_| ExposureMeasurementError::ProductOutOfRange)?;
    Ok(ExposureMeasurement {
        required_product,
        quality,
        scene_log2_luminance: scene.log2_luminance,
        clipped_fraction,
        dark_fraction,
        used_weight: reductions.used_weight,
        protection_ev,
        mean_luminance: ((reductions.measured_luminance_sum + reductions.clipped_luminance_sum)
            / reductions.used_weight)
            .max(config.dark_floor),
        mean_green_level: reductions.green_sum / reductions.used_weight,
    })
}

/// Проход по плиткам: яркость (два слагаемых), логарифм, доли — взвешенные суммы.
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
        let parts = tile_luminance(tile, luminance, config);
        let tile_luminance = parts.total();
        totals.used_weight += weight;
        totals.measured_luminance_sum += weight * parts.measured;
        totals.clipped_luminance_sum += weight * parts.clipped;
        totals.green_sum += weight * tile_green_level(tile, cells);
        totals.log2_sum += weight * tile_luminance.max(config.dark_floor).log2();
        totals.clipped_sum += weight * (tile.saturated_cells as f32 / cells as f32);
        if tile_luminance < config.dark_tile_luminance {
            totals.dark_sum += weight;
        }
    }
    totals
}

/// Средний уровень зелёного плитки: принятые ячейки — по сумме (после виньетирования, может быть > 1 — углы под
/// усилением), пересвеченные по сырому отсчёту — белые (1,0).
fn tile_green_level(tile: &AwbTile, cells: u32) -> f32 {
    let cells = cells as f32;
    // Зелёный даёт два отсчёта на ячейку (см. `tile_luminance`).
    let used_part = tile.sum_g as f32 / (2.0 * QUANT_SCALE * cells);
    used_part + tile.saturated_cells as f32 / cells
}

/// Яркость области и сдвиг экспозиции выбранным способом усреднения (решения владельца 133, 134).
///
/// Вызывается только при `used_weight >= min_used_weight > 0`, поэтому деление определено.
fn scene_luminance(
    reductions: &Reductions,
    protected_target_luminance: f32,
    config: &MeteringConfig,
) -> SceneLuminance {
    match config.scene_average {
        SceneAverage::LinearLight => {
            // Пол — на измеримую часть: совсем чёрная область (все плитки ≈ 0) не уходит в −∞ и не делит на ноль.
            let measured = (reductions.measured_luminance_sum / reductions.used_weight).max(config.dark_floor);
            let clipped = reductions.clipped_luminance_sum / reductions.used_weight;
            // Решение 134: пересвет (нижняя оценка) «занимает» не больше доли цели; остальное обязана дать
            // измеримая часть. Предел — от цели с защитой светов: равновесие замера — при Y = цель · 2^защита.
            let clipped_effective = clipped.min(config.clipped_max_share_of_target * protected_target_luminance);
            SceneLuminance {
                log2_luminance: (measured + clipped_effective).log2(),
                // Пересвет при смене выдержки остаётся белым, растёт только измеримая часть. Поэтому решаем
                // `measured · (P*/P) + clipped = цель`, а не `P* = P · цель / Y`: иначе замер зависел бы от выдержки
                // кадра и компенсация EV ±1 давала бы ×3 и ×0 вместо ×2 и ×½ (найдено на телефоне, 13c).
                // `цель − clipped ≥ (1 − доля) · цель > 0` — это гарантирует `validate()` (доля < 1).
                exposure_shift_log2: (protected_target_luminance - clipped_effective).log2() - measured.log2(),
            }
        }
        SceneAverage::LogLuminance => {
            let log2_luminance = reductions.log2_sum / reductions.used_weight;
            SceneLuminance {
                log2_luminance,
                exposure_shift_log2: protected_target_luminance.log2() - log2_luminance,
            }
        }
    }
}

/// Яркость области и вывод из неё: на сколько ступеней изменить экспозицию кадра.
#[derive(Debug, Clone, Copy)]
struct SceneLuminance {
    /// `log2` яркости области выбранным способом (с пересветом по нижней оценке и пределу).
    log2_luminance: f32,
    /// `log2(P*/P)`: на сколько ступеней кадр надо сделать ярче (> 0) или темнее, включая защиту светов.
    exposure_shift_log2: f32,
}

/// Яркость плитки: принятые ячейки по весам яркости, пересвеченные — `clipped_cell_luminance`.
fn tile_luminance(tile: &AwbTile, luminance: LuminanceWeights, config: &MeteringConfig) -> TileLuminance {
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
    TileLuminance {
        measured: used_luminance_sum / cells,
        clipped: tile.saturated_cells as f32 * config.clipped_cell_luminance / cells,
    }
}

/// Яркость плитки двумя слагаемыми: вклад принятых ячеек и вклад пересвеченных (каждая — `clipped_cell_luminance`).
#[derive(Debug, Clone, Copy)]
struct TileLuminance {
    /// Вклад принятых ячеек (их яркость, разделённая на все ячейки плитки).
    measured: f32,
    /// Вклад пересвеченных ячеек — нижняя оценка: настоящая яркость не меньше.
    clipped: f32,
}

impl TileLuminance {
    /// Полная яркость плитки (как в 08a: пересвет — по нижней оценке, без ограничения).
    fn total(self) -> f32 {
        self.measured + self.clipped
    }
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
