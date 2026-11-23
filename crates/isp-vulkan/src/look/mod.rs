//! Слой вида на GPU (решения 98, 99, 176, 178): стадия цвета вида (насыщенность, сочность, полосы оттенка в
//! OKLab/OKLCh) и таблица LUT. ОДИН общий GLSL-файл (`shaders/look_stage.glsl`) и ОДИН упаковщик на все пути:
//! видоискатель, качественный конвейер и проявление снимка берут блок как есть, ничего не пересчитывая. Числа —
//! ровно те f32, что отдаёт `isp_model::look::color::LookColorParams` (эталон подставляет их же).
//!
//! Слой стоит ПОСЛЕ основы тона (любой модели: см. [`crate::tone_base`]) над светом экрана в рабочем
//! пространстве `W`, до перевода в цвета выхода. О модели основы он не знает: от неё берётся только `W`, а
//! матрицы OKLab для `W` приходят готовыми в самом блоке.
//!
//! # Нейтраль — флаг в uniform, не константа специализации
//! Нейтральные ручки цвета пропускают стадию целиком (OKLab туда и обратно не бит-в-бит), но выбор «нейтраль /
//! не нейтраль» живёт в БЛОКЕ ([`LookParameters`] строка 0, x): смена варианта шейдера на первом касании ручки
//! создала бы pipeline посреди жеста. Поэтому и нейтраль, и ненейтраль — один и тот же вариант пайплайна, а
//! стадия цвета в нём — ветка по значению uniform.
//!
//! # Раскладка блока `LookParameters` (std140, [`LOOK_ROWS`] строк по 16 байт; зеркало `look_stage.glsl`)
//! | Строка | Содержимое |
//! |---|---|
//! | 0 | x — цвет вида включён (1.0) или пропущен (0.0), y — занято полос (float), z, w — 0 |
//! | 1 | x — множитель цветности насыщенности `M_sat`, y — сила сочности `k_vib·range_vib`, z — `c_ref` (цветность, при которой вес сочности падает вдвое), w — гладкий порог серого `F` |
//! | 2 | xy — `(cos h₀, sin h₀)` центра окна кожи, z — `cos w` (косинус полуширины окна), w — доля защиты кожи `p` |
//! | 3–5 | OKLab `W → LMS` по строкам, w = 0 |
//! | 6–8 | OKLab `LMS' → Lab` |
//! | 9–11 | OKLab `Lab → LMS'` |
//! | 12–14 | OKLab `LMS → W` |
//! | 15–18 | центры полос, радианы, по возрастанию (16 чисел: строка 15 — полосы 0–3, …) |
//! | 19–22 | сдвиг оттенка полосы, радианы |
//! | 23–26 | множитель цветности полосы (1 — нейтраль) |
//! | 27–30 | множитель светлоты полосы (1 — нейтраль) |
//!
//! Нейтральный вид кладёт нули во все строки: шейдер их не читает.
//!
//! # LUT
//! Таблица лежит в отдельном storage-буфере ([`lut_buffer`]), потому что блок uniform для неё мал; в видоискателе
//! вариант шейдера «с таблицей / без» — константа специализации (`IspStage::LookLut`), в выходном проходе —
//! проверка размера в заголовке буфера.

pub(crate) mod lut_buffer;

pub use lut_buffer::{LookLutError, LookLutLimits};

use isp_model::look::bands::MAX_LOOK_BANDS;
use isp_model::params::IspParams;

use crate::std140::Std140Writer;

/// Число строк блока (зеркало `LOOK_ROWS` в `look_stage.glsl`).
pub(crate) const LOOK_ROWS: usize = 31;

/// Размер блока в байтах.
pub(crate) const LOOK_UNIFORM_BYTES: usize = LOOK_ROWS * 16;

/// Сколько `vec4` занимают [`MAX_LOOK_BANDS`] чисел (по четыре в строке).
const BAND_ROWS_PER_ARRAY: usize = MAX_LOOK_BANDS / 4;

/// Собирает байты блока `LookParameters` по разрешённому цвету вида (раскладка — таблица модуля).
pub(crate) fn pack_look_uniforms(params: &IspParams) -> [u8; LOOK_UNIFORM_BYTES] {
    let mut writer = Std140Writer::default();
    match &params.look.color {
        None => {
            // Нейтраль: флаг 0 и нули. Шейдер дальше строки 0 не читает.
            for _ in 0..LOOK_ROWS {
                writer.vec4([0.0; 4]);
            }
        }
        Some(color) => {
            let band_count = color.bands.map_or(0, |bands| bands.count);
            writer.vec4([1.0, band_count as f32, 0.0, 0.0]);
            writer.vec4([
                color.saturation_multiplier,
                color.vibrance_amount,
                color.vibrance_chroma_reference,
                color.neutral_chroma_floor,
            ]);
            writer.vec4([
                color.skin_direction[0],
                color.skin_direction[1],
                color.skin_cos_half_width,
                color.skin_protection,
            ]);
            writer.matrix3(color.oklab.working_to_lms);
            writer.matrix3(color.oklab.lms_prime_to_lab);
            writer.matrix3(color.oklab.lab_to_lms_prime);
            writer.matrix3(color.oklab.lms_to_working);
            match &color.bands {
                Some(bands) => {
                    write_band_array(&mut writer, &bands.centers_radians);
                    write_band_array(&mut writer, &bands.hue_shift_radians);
                    write_band_array(&mut writer, &bands.chroma_multiplier);
                    write_band_array(&mut writer, &bands.lightness_multiplier);
                }
                None => {
                    // Полос нет (count = 0): шейдер массивы не читает, но блок пишется целиком.
                    for _ in 0..4 * BAND_ROWS_PER_ARRAY {
                        writer.vec4([0.0; 4]);
                    }
                }
            }
        }
    }
    writer.finish::<LOOK_UNIFORM_BYTES>()
}

/// Пишет массив из [`MAX_LOOK_BANDS`] чисел четырьмя строками `vec4`.
fn write_band_array(writer: &mut Std140Writer, values: &[f32; MAX_LOOK_BANDS]) {
    for row in values.chunks_exact(4) {
        writer.vec4([row[0], row[1], row[2], row[3]]);
    }
}

#[cfg(test)]
mod tests;
