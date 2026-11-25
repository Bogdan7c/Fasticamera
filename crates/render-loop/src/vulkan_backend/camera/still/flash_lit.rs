//! Проверка «кадр со вспышкой действительно освещён» по пикселям (сессия 14d, факт зонда: HAL может
//! сообщить `FIRED` на кадре, где светодиод не горел). Чистые функции, без GPU и движка.
//!
//! # Идея
//! Свет кадра пропорционален «выдержке × ISO». Среднее зелёного на единицу произведения — `g = G / P`. У кадра
//! без вспышки (уровень сцены) `g_сцены = G_сц / P_сц`, у кадра со вспышкой `g_кадра = G / P`. Тогда доля света,
//! которую дала вспышка:
//! ```text
//! измеренная_доля = 1 − g_сцены / g_кадра            (0…1)
//! ```
//! Кадр освещён, если `измеренная_доля ≥ порог × предсказанная_доля`.
//!
//! # Одна шкала со статистикой (дефект 14f, решение 175)
//! Уровень сцены `AmbientLevel` приходит из статистики видоискателя (`ExposureMeasurement::mean_green_level`), а она
//! считается ПОСЛЕ коррекции виньетирования. Поэтому зелёный кадра со вспышкой мерится так же, по правилам
//! `isp_model::statistics`, для каждой ячейки 2×2 решётки:
//! 1. отсчёты нормируются (чёрный вычтен, белый = 1) — ДО виньетирования;
//! 2. ячейка, где хоть один из четырёх нормированных отсчётов `!(v < saturation_threshold)`, считается белой
//!    (зелёный = 1,0): пересвет определяет сам сенсор, а не наше усиление;
//! 3. иначе каждый зелёный отсчёт умножается на усиление стадии виньетирования КАДРА (те же `IspParams`, та же
//!    выборка «одно усиление на ячейку», что у видоискателя) и обрезается сверху запасом `sum_headroom`, не белым;
//! 4. зелёный ячейки — среднее двух её зелёных; уровень области — среднее по ячейкам решётки.
//!
//! Баланс белого не участвует (как в статистике). Без коррекции (стадия пропущена) усиление 1 — прежняя шкала.
//!
//! RAW читается РЕДКОЙ решёткой ячеек 2×2 (поток рендера не должен читать ≈ 24 МиБ памяти GPU целиком).
//! Область — кадрирование снимка (то же место, что мерит статистика), а не весь кадр.

use camera_contract::geometry::PixelRect;
use isp_model::cfa::{BayerPattern, CfaCell, RgbChannel};
use isp_model::lens_shading::{LensShadingSampling, LensShadingStep};
use isp_model::levels::{NormalizationLevels, normalize_sample};
use still_contract::shot::RawSamples16;

/// Откуда и как читать RAW.
#[derive(Clone, Copy)]
pub struct GreenSampling<'a> {
    pub samples: &'a RawSamples16,
    pub pattern: BayerPattern,
    pub levels: &'a NormalizationLevels,
    /// Коррекция виньетирования этого кадра (из его разрешённых `IspParams`): нужна, чтобы мерить в шкале статистики.
    pub shading: &'a LensShadingStep,
    /// Область в координатах ОТСЧЁТОВ (начало — угол полезной области).
    pub area: PixelRect,
    /// Шаг решётки в ячейках 2×2 по обеим осям (≥ 1).
    pub stride_cells: u32,
    /// Порог пересвета статистики (нормированный свет ДО виньетирования).
    pub saturation_threshold: f32,
    /// Потолок скорректированного отсчёта статистики (`sum_headroom`).
    pub sum_headroom: f32,
}

/// Средний зелёный области в шкале статистики (см. модуль). `None` — в решётку не попало ни одной ячейки
/// (область пуста или вне отсчётов).
pub fn mean_green_level(sampling: GreenSampling<'_>) -> Option<f64> {
    let size = sampling.samples.size();
    let stride = sampling.stride_cells.max(1);
    // Ячейки начинаются с чётной координаты: рисунок Bayer повторяется с периодом 2.
    let first_x = sampling.area.left.div_ceil(2) * 2;
    let first_y = sampling.area.top.div_ceil(2) * 2;
    let end_x = (sampling.area.left + sampling.area.size.width).min(size.width);
    let end_y = (sampling.area.top + sampling.area.size.height).min(size.height);
    let (mut sum, mut count) = (0.0_f64, 0_u64);
    let mut y = first_y;
    while y + 1 < end_y {
        let mut x = first_x;
        while x + 1 < end_x {
            if let Some(level) = cell_green_level(&sampling, x, y) {
                sum += level;
                count += 1;
            }
            x += 2 * stride;
        }
        y += 2 * stride;
    }
    (count > 0).then(|| sum / count as f64)
}

/// Зелёный одной ячейки 2×2 с началом `(x, y)` (чётным) по правилам статистики; `None` — отсчёты ячейки не читаются.
fn cell_green_level(sampling: &GreenSampling<'_>, x: u32, y: u32) -> Option<f64> {
    let valid_area = sampling.samples.size();
    let mut greens = [0.0_f32; 2];
    let mut green_count = 0;
    let mut saturated = false;
    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        let (px, py) = (x + dx, y + dy);
        let raw = sampling.samples.sample(px, py)?;
        let level = normalize_sample(raw, CfaCell::of_pixel(px, py), sampling.levels);
        // Запись `!(v < t)`, как у статистики: NaN тоже исключает ячейку.
        saturated |= !matches!(
            level.partial_cmp(&sampling.saturation_threshold),
            Some(std::cmp::Ordering::Less)
        );
        let color = sampling.pattern.channel_at(px, py);
        if color == RgbChannel::Green && green_count < greens.len() {
            let gain = sampling.shading.gain_for_sample(
                (px, py),
                valid_area,
                RgbChannel::Green,
                LensShadingSampling::PerCellCentre,
            );
            greens[green_count] = (level * gain).clamp(0.0, sampling.sum_headroom);
            green_count += 1;
        }
    }
    if saturated {
        // Пересвеченная по сырому отсчёту ячейка в статистике — белая.
        return Some(1.0);
    }
    // В ячейке 2×2 любого рисунка Bayer ровно два зелёных.
    (green_count == greens.len()).then(|| f64::from(greens[0] + greens[1]) / 2.0)
}

/// Пороги проверки (из конфига `still`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LitThresholds {
    /// Какую долю предсказанного вклада вспышки надо увидеть.
    pub min_fraction_of_predicted: f32,
    /// Ниже этой предсказанной доли проверка нерешаема.
    pub min_predicted_share: f32,
}

/// Почему проверку решить нельзя (кадр принимается, обход считается).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LitUndecidable {
    /// Предсказанная доля вспышки слишком мала: освещённый кадр не отличить от неосвещённого.
    PredictedShareTooLow,
    /// Нет уровня света сцены того же потока.
    NoAmbientLevel,
    /// Отсчёты RAW или уровни кадра прочитать нельзя.
    SamplesUnavailable,
}

/// Вердикт проверки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LitVerdict {
    Lit { measured_share: f64 },
    NotLit { measured_share: f64 },
    Undecidable(LitUndecidable),
}

/// Решает по измерениям (см. модуль). `*_green_per_product` — средний зелёный на единицу «выдержка × ISO».
pub fn judge_lit(
    predicted_share: f64,
    ambient_green_per_product: Option<f64>,
    flash_frame_green_per_product: f64,
    thresholds: LitThresholds,
) -> LitVerdict {
    if predicted_share < f64::from(thresholds.min_predicted_share) {
        return LitVerdict::Undecidable(LitUndecidable::PredictedShareTooLow);
    }
    let Some(ambient) = ambient_green_per_product else {
        return LitVerdict::Undecidable(LitUndecidable::NoAmbientLevel);
    };
    // Чёрный кадр: света нет вовсе — вспышка не дошла.
    let measured_share = if flash_frame_green_per_product > 0.0 {
        (1.0 - ambient / flash_frame_green_per_product).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if measured_share >= f64::from(thresholds.min_fraction_of_predicted) * predicted_share {
        LitVerdict::Lit { measured_share }
    } else {
        LitVerdict::NotLit { measured_share }
    }
}

#[cfg(test)]
#[path = "flash_lit_tests.rs"]
mod tests;
