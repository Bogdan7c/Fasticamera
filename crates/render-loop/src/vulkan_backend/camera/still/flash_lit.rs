//! Проверка «кадр со вспышкой действительно освещён» по пикселям (сессия 14d, факт зонда: HAL может
//! сообщить `FIRED` на кадре, где светодиод не горел). Чистые функции, без GPU и движка.
//!
//! # Идея
//! Свет кадра пропорционален «выдержке × ISO». Среднее зелёного кадра на единицу произведения —
//! `g = G / P`. У кадра без вспышки (уровень сцены) `g_сцены = G_сц / P_сц`, у кадра со вспышкой
//! `g_кадра = G / P`. Тогда доля света, которую дала вспышка:
//! ```text
//! измеренная_доля = 1 − g_сцены / g_кадра            (0…1)
//! ```
//! Кадр освещён, если `измеренная_доля ≥ порог × предсказанная_доля`. Зелёный берётся нормированным (чёрный
//! вычтен, белый = 1, как в статистике ISP: `ExposureMeasurement::mean_green_level`), поэтому обе стороны на одной
//! шкале и баланс белого не участвует.
//!
//! RAW читается РЕДКОЙ решёткой ячеек 2×2 (поток рендера не должен читать ≈ 24 МиБ памяти GPU целиком).
//! Область — кадрирование снимка (то же место, что мерит статистика), а не весь кадр.

use camera_contract::geometry::PixelRect;
use isp_model::cfa::{BayerPattern, CfaCell, RgbChannel};
use isp_model::levels::{NormalizationLevels, normalize_sample};
use still_contract::shot::RawSamples16;

/// Откуда и как читать RAW.
#[derive(Clone, Copy)]
pub struct GreenSampling<'a> {
    pub samples: &'a RawSamples16,
    pub pattern: BayerPattern,
    pub levels: &'a NormalizationLevels,
    /// Область в координатах ОТСЧЁТОВ (начало — угол полезной области).
    pub area: PixelRect,
    /// Шаг решётки в ячейках 2×2 по обеим осям (≥ 1).
    pub stride_cells: u32,
}

/// Средний нормированный зелёный области (0…1). `None` — в решётку не попало ни одного зелёного отсчёта
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
            for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                let (px, py) = (x + dx, y + dy);
                if sampling.pattern.channel_at(px, py) != RgbChannel::Green {
                    continue;
                }
                if let Some(raw) = sampling.samples.sample(px, py) {
                    let level = normalize_sample(raw, CfaCell::of_pixel(px, py), sampling.levels);
                    // Как в статистике: выше белого не считаем (пересвет = белое).
                    sum += f64::from(level.min(1.0));
                    count += 1;
                }
            }
            x += 2 * stride;
        }
        y += 2 * stride;
    }
    (count > 0).then(|| sum / count as f64)
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
