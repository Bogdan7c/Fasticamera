//! Основа AgX в общем блоке `ToneBaseParameters`: упаковщик строк модели (зеркало
//! `shaders/tone_base_agx.glsl`, менять только вместе).
//!
//! # Строки модели (смещение от первой строки модели, то есть от строки 1 блока; 33 строки)
//! | Смещение | Содержимое |
//! |---|---|
//! | 0–2 | матрица `XYZ D50 → W` по строкам, w = 0 |
//! | 3–5 | матрица «внутрь» (inset) |
//! | 6–8 | матрица «наружу» (outset) |
//! | 9–11 | матрица `W → цвета выхода` |
//! | 12 | серый сцены с подтяжкой AE, выход кривой на сером `y_p`, показатель `γ`, наклон `s` |
//! | 13 | носок: степень `p0`, `1/p0`, глубина `Δ_toe`, `r0^p0 − 1` |
//! | 14 | плечо: степень `p1`, `1/p1`, `s/(1 − y_p)`, 0 |
//! | 15 | вход: порог сжатия по каналам, w — степень `p` |
//! | 16 | вход: масштаб по каналам, w — `1/p` (посчитано здесь) |
//! | 17 | выход: порог по каналам, w — степень |
//! | 18 | выход: масштаб по каналам, w — `1/p` |
//! | 19 | флаги (числа float): x — вход (0 обрезка нулём, 1 мягкое сжатие), y — степень возврата оттенка `h` (0 — шаг пропущен), z — гладкий порог серого `F`, w — 0 |
//! | 20–22 | OKLab `W → LMS` |
//! | 23 | строка яркости `Y` рабочего пространства (xyz), w = 0 |
//! | 24–26 | OKLab `LMS' → Lab` |
//! | 27–29 | OKLab `Lab → LMS'` |
//! | 30–32 | OKLab `LMS → W` |
//!
//! Без возврата оттенка (`h = 0`) строки 20–32 нулевые и шейдер их не читает.

use isp_model::tone::agx::AgxParameters;

use super::{ToneBaseRows, input_compression_flag};

/// Сколько строк модели занято (тесты сверяют с числом записанных).
#[cfg(test)]
pub(crate) const ROW_COUNT: usize = 33;

/// Пишет строки модели в порядке таблицы модуля.
pub(crate) fn pack(rows: &mut ToneBaseRows, agx: &AgxParameters) {
    rows.push_matrix3(agx.xyz_d50_to_working);
    rows.push_matrix3(agx.inset);
    rows.push_matrix3(agx.outset);
    rows.push_matrix3(agx.working_to_output);
    rows.push([agx.grey, agx.gray_output, agx.gamma, agx.slope]);
    rows.push([agx.toe_power, agx.toe_inverse_power, agx.toe_depth, agx.toe_excess]);
    rows.push([agx.shoulder_power, agx.shoulder_inverse_power, agx.shoulder_scale, 0.0]);
    let input_flag = input_compression_flag(agx.input_compression.as_ref());
    rows.push_input_gamut_compression(agx.input_compression.as_ref());
    rows.push_gamut_compression(&agx.output_compression);
    match &agx.hue_restoration {
        Some(hue) => {
            rows.push([input_flag, hue.degree, hue.chroma_floor, 0.0]);
            rows.push_matrix3(hue.oklab.working_to_lms);
            rows.push([hue.luminance_row[0], hue.luminance_row[1], hue.luminance_row[2], 0.0]);
            rows.push_matrix3(hue.oklab.lms_prime_to_lab);
            rows.push_matrix3(hue.oklab.lab_to_lms_prime);
            rows.push_matrix3(hue.oklab.lms_to_working);
        }
        None => {
            rows.push([input_flag, 0.0, 0.0, 0.0]);
            // Строки 20–32 нулевые: блок обнуляется целиком, но `len` должен сойтись с `ROW_COUNT`.
            for _ in 0..13 {
                rows.push([0.0; 4]);
            }
        }
    }
}
