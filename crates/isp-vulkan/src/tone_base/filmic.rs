//! Основа filmic в общем блоке `ToneBaseParameters`: упаковщик строк модели (зеркало
//! `shaders/tone_base_filmic.glsl`, менять только вместе). Эталон — `isp-model::tone::filmic`; числа берутся
//! ровно те f32, что отдаёт `FilmicBase::parameters()`, шейдер ничего заново не выводит (порядок формул пикселя —
//! в доке модуля эталона).
//!
//! # Строки модели (смещение от первой строки модели, то есть от строки 1 блока; 28 строк)
//! | Смещение | Содержимое |
//! |---|---|
//! | 0–2 | матрица `XYZ D50 → W` по строкам, w = 0 |
//! | 3–5 | матрица `W → цвета выхода` |
//! | 6 | серый сцены с подтяжкой AE `g`, выход кривой на сером `y_p`, показатель `γ`, наклон `s` |
//! | 7 | носок: длина линейного участка вниз `a`, `y_lo`, ширина `w_t`, `r0^p0 − 1` |
//! | 8 | степени: `p0`, `1/p0`, `p1`, `1/p1` |
//! | 9 | плечо: длина линейного участка вверх `b`, `y_hi`, `h = 1 − y_hi`; w — порог обесцвечивания `o` |
//! | 10 | вход: порог сжатия по каналам, w — степень `p` |
//! | 11 | вход: масштаб по каналам, w — `1/p` (посчитано здесь) |
//! | 12 | выход: порог по каналам, w — степень |
//! | 13 | выход: масштаб по каналам, w — `1/p` |
//! | 14 | флаги (числа float): x — вход (0 обрезка нулём, 1 мягкое сжатие), y — норма ([`norm_code`]), z, w — 0 |
//! | 15 | строка яркости `Y` рабочего пространства (xyz), w = 0 |
//! | 16–18 | OKLab `W → LMS` |
//! | 19–21 | OKLab `LMS' → Lab` |
//! | 22–24 | OKLab `Lab → LMS'` |
//! | 25–27 | OKLab `LMS → W` |
//!
//! Матрицы OKLab лежат всегда (норме `PerChannel` они не нужны, но блок единый для всех норм: смена нормы
//! ручкой не меняет раскладку).

use isp_model::tone::filmic::{FilmicNorm, FilmicParameters};

use super::{ToneBaseRows, input_compression_flag};

/// Сколько строк модели занято (тесты сверяют с числом записанных).
#[cfg(test)]
pub(crate) const ROW_COUNT: usize = 28;

/// Код нормы в строке 14 (зеркало `FILMIC_NORM_*` в `tone_base_filmic.glsl`). Ветки явные, без `_ =>`: новая
/// норма в эталоне не пройдёт молча.
pub(crate) fn norm_code(norm: FilmicNorm) -> f32 {
    match norm {
        FilmicNorm::PerChannel => 0.0,
        FilmicNorm::MaxRgb => 1.0,
        FilmicNorm::Luminance => 2.0,
        FilmicNorm::PowerNorm => 3.0,
    }
}

/// Пишет строки модели в порядке таблицы модуля.
pub(crate) fn pack(rows: &mut ToneBaseRows, filmic: &FilmicParameters) {
    rows.push_matrix3(filmic.xyz_d50_to_working);
    rows.push_matrix3(filmic.working_to_output);
    rows.push([filmic.grey, filmic.gray_output, filmic.gamma, filmic.slope]);
    rows.push([
        filmic.linear_below,
        filmic.toe_value,
        filmic.toe_width,
        filmic.toe_excess,
    ]);
    rows.push([
        filmic.toe_power,
        filmic.toe_inverse_power,
        filmic.shoulder_power,
        filmic.shoulder_inverse_power,
    ]);
    rows.push([
        filmic.linear_above,
        filmic.shoulder_value,
        filmic.shoulder_headroom,
        filmic.desaturation_onset,
    ]);
    let input_flag = input_compression_flag(filmic.input_compression.as_ref());
    rows.push_input_gamut_compression(filmic.input_compression.as_ref());
    rows.push_gamut_compression(&filmic.output_compression);
    rows.push([input_flag, norm_code(filmic.norm), 0.0, 0.0]);
    rows.push([
        filmic.luminance_row[0],
        filmic.luminance_row[1],
        filmic.luminance_row[2],
        0.0,
    ]);
    rows.push_matrix3(filmic.oklab.working_to_lms);
    rows.push_matrix3(filmic.oklab.lms_prime_to_lab);
    rows.push_matrix3(filmic.oklab.lab_to_lms_prime);
    rows.push_matrix3(filmic.oklab.lms_to_working);
}
