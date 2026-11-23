//! Основа ACES в общем блоке `ToneBaseParameters`: упаковщик строк модели (зеркало
//! `shaders/tone_base_aces.glsl`, менять только вместе).
//!
//! Числа — ровно те f32, что отдаёт `isp-model` ([`AcesParameters`]); шейдер ничего не пересчитывает, кроме того,
//! что делает пиксельная формула эталона.
//!
//! # Строки модели (смещение от первой строки модели, то есть от строки 1 блока; 44 строки)
//! | Смещение | Содержимое |
//! |---|---|
//! | 0–2 | матрица `XYZ D50 → W` по строкам, w = 0 |
//! | 3–5 | матрица `W → цвета выхода` |
//! | 6–8 | матрица `W → AP0` (Брэдфорд D65 → D60 внутри) |
//! | 9–11 | матрица `AP0 → AP1` |
//! | 12–14 | матрица `AP1 → W` (десатурация выхода уже внутри) |
//! | 15 | xyz — строка яркости AP1, w — `0,18/grey_eff` (множитель входа) |
//! | 16 | x — сила свечения, y — масштаб красного модификатора, z — десатурация RRT, w — `γ_s − 1` (окружение) |
//! | 17 | флаги (числа float): x — вход (0 обрезка нулём, 1 мягкое сжатие), yzw — 0 |
//! | 18 | вход: порог сжатия по каналам, w — степень `p` |
//! | 19 | вход: масштаб по каналам, w — `1/p` (посчитано здесь) |
//! | 20 | выход: порог по каналам, w — степень |
//! | 21 | выход: масштаб по каналам, w — `1/p` |
//! | 22 | кривая: x — серый сцены `grey_eff`, y — контраст, z — сдвиг теней, w — сдвиг светов |
//! | 23 | кривая: x — ширина перехода, y — `log10(0,18)`, z — сдвиг экспозиции `δ`, w — колено `k` |
//! | 24 | кривая: x — значение в колене, y — наклон в колене, z — предел белого `E_cap`, w — чёрный `Y_min` |
//! | 25 | кривая: x — `1/(Y_max − Y_min)`, y — `log10(Y_min)`, zw — 0 |
//! | 26–34 | сплайн RRT (C5), раскладка — ниже |
//! | 35–43 | сплайн выходного преобразования (C9), раскладка — ниже |
//!
//! Сплайн занимает 9 строк (смещение от его первой строки):
//! `0–2` коэффициенты нижнего участка (10 чисел в 12 ячейках, хвост 0), `3–5` верхнего участка,
//! `6` — `log10` точек min, mid, max и `(N−1)/(log_mid − log_min)`, `7` — `(N−1)/(log_max − log_mid)`,
//! номер последнего участка `N − 2` (целое как float), наклоны нижней и верхней прямых, `8` — свободные члены
//! нижней и верхней прямых, zw — 0.

use isp_model::tone::aces::AcesParameters;
use isp_model::tone::aces::spline::{COEFFICIENT_SLOTS, SplineParameters};

use super::{ToneBaseRows, input_compression_flag};

/// Сколько строк модели занято (тесты сверяют с числом записанных).
#[cfg(test)]
pub(crate) const ROW_COUNT: usize = 44;

/// Сколько строк занимает один сплайн.
#[cfg(test)]
pub(crate) const SPLINE_ROW_COUNT: usize = 9;

/// Пишет строки модели в порядке таблицы модуля.
pub(crate) fn pack(rows: &mut ToneBaseRows, aces: &AcesParameters) {
    rows.push_matrix3(aces.xyz_d50_to_working);
    rows.push_matrix3(aces.working_to_output);
    rows.push_matrix3(aces.working_to_ap0);
    rows.push_matrix3(aces.ap0_to_ap1);
    rows.push_matrix3(aces.ap1_to_working);
    rows.push([
        aces.ap1_luminance[0],
        aces.ap1_luminance[1],
        aces.ap1_luminance[2],
        aces.exposure_scale,
    ]);
    rows.push([
        aces.sweeteners.glow_gain,
        aces.sweeteners.red_modifier_scale,
        aces.rrt_saturation,
        aces.surround_exponent,
    ]);
    rows.push([input_compression_flag(aces.input_compression.as_ref()), 0.0, 0.0, 0.0]);
    rows.push_input_gamut_compression(aces.input_compression.as_ref());
    rows.push_gamut_compression(&aces.output_compression);
    let tone = &aces.tone;
    rows.push([tone.grey, tone.contrast, tone.shadows_ev, tone.highlights_ev]);
    rows.push([tone.shape_width_ev, tone.log_reference, tone.shift_ev, tone.knee_ev]);
    rows.push([tone.knee_value, tone.knee_slope, tone.max_white_ev, tone.black_level]);
    rows.push([tone.inverse_range, tone.log_black, 0.0, 0.0]);
    push_spline(rows, &tone.rrt);
    push_spline(rows, &tone.odt);
}

/// Пишет сплайн: девять строк по таблице модуля.
fn push_spline(rows: &mut ToneBaseRows, spline: &SplineParameters) {
    push_coefficients(rows, &spline.coefficients_low);
    push_coefficients(rows, &spline.coefficients_high);
    rows.push([spline.log_min_x, spline.log_mid_x, spline.log_max_x, spline.low_scale]);
    // Номер последнего участка — малое целое: в f32 хранится точно.
    rows.push([
        spline.high_scale,
        spline.last_segment as f32,
        spline.slope_low,
        spline.slope_high,
    ]);
    rows.push([spline.low_intercept, spline.high_intercept, 0.0, 0.0]);
}

/// Десять коэффициентов в трёх строках (12 ячеек, хвост нулевой).
fn push_coefficients(rows: &mut ToneBaseRows, coefficients: &[f32; COEFFICIENT_SLOTS]) {
    let mut padded = [0.0_f32; 12];
    padded[..COEFFICIENT_SLOTS].copy_from_slice(coefficients);
    for row in padded.chunks_exact(4) {
        rows.push([row[0], row[1], row[2], row[3]]);
    }
}
