//! Слой вида на GPU (решения 98, 99, 176, 178, 236): тон вида (контраст, средние тона, тени, света), стадия цвета
//! (насыщенность, сочность, полосы оттенка в OKLab/OKLCh) и таблица LUT. ОДИН общий GLSL-файл (`shaders/look_stage.glsl`) и ОДИН упаковщик на все пути:
//! видоискатель, качественный конвейер и проявление снимка берут блок как есть, ничего не пересчитывая. Числа —
//! ровно те f32, что отдают `isp_model::look::{tone_shape::LookTone, color::LookColorParams}` (эталон берёт их же).
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
//! | 31 | колёса по зонам: x — включены (1.0) или нейтральны (0.0), y — `L_g` (светлота серого экрана), z — `1/L_g`, w — `1/(1 − L_g)` |
//! | 32–35 | сдвиги зон колёс `(ΔL, Δa, Δb, 0)`: Lift, Gamma, Gain, Offset (по порядку строк) |
//! | 36 | тон вида: x — включён (1.0) или нейтрален (0.0), y — `q` (серый экрана, `√Y_g`), z — `1/q`, w — `1/(1 − q)` |
//! | 37 | наклоны тона: x — тени у чёрного, y — света у белого, z — контраст на сером, w — средние тона у чёрного |
//! | 38 | x — доля растянутого цвета, y, z, w — 0 |
//! | 39 | полоски R, G, B (решения 339–343): x — включены (1.0) или нейтральны (0.0), y — `c` («носок» кодирования), z — `√c`, w — `n = √(1 + c) − √c` |
//! | 40 | x — `1/n`, y, z, w — 0 |
//! | 41–43 | `W → основа каналов` (по строкам, w = 0) |
//! | 44–46 | `основа каналов → W` (по строкам, w = 0) |
//! | 47 | Lift: `l` для R, G, B (xyz), w = 0 |
//! | 48 | Gamma: наклон `s` для R, G, B |
//! | 49 | Gain: `G` для R, G, B |
//! | 50 | Offset: `o` для R, G, B |
//!
//! Тон, полоски и цвет независимы: нейтральный цвет кладёт нули в строки 0–35, нейтральный тон — в 36–38, нейтральные
//! полоски — в 39–50; шейдер их не читает (флаг в строках 0, 36 и 39). Нейтральные колёса (`wheels: None`) — нули в
//! строках 31–35 при любом цвете; флаг цвета (строка 0) остаётся 1, если тронуто что-то ещё или сами колёса.
//!
//! # LUT
//! Таблица лежит в отдельном storage-буфере ([`lut_buffer`]), потому что блок uniform для неё мал; в видоискателе
//! вариант шейдера «с таблицей / без» — константа специализации (`IspStage::LookLut`), в выходном проходе —
//! проверка размера в заголовке буфера.

pub(crate) mod lut_buffer;

pub use lut_buffer::{LookLutError, LookLutLimits};

use isp_model::look::bands::MAX_LOOK_BANDS;
use isp_model::look::color::LookColorParams;
use isp_model::look::primaries::LookPrimaries;
use isp_model::look::tone_shape::LookTone;
use isp_model::look::wheels::LookWheels;
use isp_model::params::IspParams;

use crate::std140::Std140Writer;

/// Число строк блока (зеркало `LOOK_ROWS` в `look_stage.glsl`).
pub(crate) const LOOK_ROWS: usize = COLOR_ROWS + TONE_ROWS + PRIMARIES_ROWS;

/// Строк цвета вида (0–35): флаги, числа, матрицы OKLab, полосы, колёса.
const COLOR_ROWS: usize = 36;

/// Строк тона вида (36–38).
const TONE_ROWS: usize = 3;

/// Строк полосок R, G, B (39–50): константы кодирования (2), две матрицы 3×3 (6), Lift, Gamma, Gain, Offset (4).
const PRIMARIES_ROWS: usize = 12;

/// Размер блока в байтах.
pub(crate) const LOOK_UNIFORM_BYTES: usize = LOOK_ROWS * 16;

/// Сколько `vec4` занимают [`MAX_LOOK_BANDS`] чисел (по четыре в строке).
const BAND_ROWS_PER_ARRAY: usize = MAX_LOOK_BANDS / 4;

/// Строк колёс: одна с окном зон и по одной на Lift, Gamma, Gain, Offset.
const WHEEL_ROWS: usize = 5;

/// Собирает байты блока `LookParameters` по разрешённому виду (раскладка — таблица модуля).
pub(crate) fn pack_look_uniforms(params: &IspParams) -> [u8; LOOK_UNIFORM_BYTES] {
    let mut writer = Std140Writer::default();
    write_color(&mut writer, params.look.color.as_ref());
    write_tone(&mut writer, params.look.tone.as_ref());
    write_primaries(&mut writer, params.look.primaries.as_ref());
    writer.finish::<LOOK_UNIFORM_BYTES>()
}

/// Пишет строки цвета 0–35; нейтральный цвет (`None`) — нули, дальше флага в строке 0 шейдер их не читает.
fn write_color(writer: &mut Std140Writer, color: Option<&LookColorParams>) {
    let Some(color) = color else {
        write_zero_rows(writer, COLOR_ROWS);
        return;
    };
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
            write_band_array(writer, &bands.centers_radians);
            write_band_array(writer, &bands.hue_shift_radians);
            write_band_array(writer, &bands.chroma_multiplier);
            write_band_array(writer, &bands.lightness_multiplier);
        }
        // Полос нет (count = 0): шейдер массивы не читает, но блок пишется целиком.
        None => write_zero_rows(writer, 4 * BAND_ROWS_PER_ARRAY),
    }
    write_wheels(writer, color.wheels.as_ref());
}

/// Пишет строки тона 36–38; нейтральный тон (`None`) — нули, шейдер дальше флага их не читает.
fn write_tone(writer: &mut Std140Writer, tone: Option<&LookTone>) {
    let Some(tone) = tone else {
        write_zero_rows(writer, TONE_ROWS);
        return;
    };
    writer.vec4([1.0, tone.pivot, tone.shadow_scale, tone.highlight_scale]);
    writer.vec4([
        tone.shadow_slope,
        tone.highlight_slope,
        tone.contrast_slope,
        tone.midtone_slope,
    ]);
    writer.vec4([tone.color_follow, 0.0, 0.0, 0.0]);
}

/// Пишет строки полосок 39–50; нейтральные полоски (`None`) — нули, шейдер дальше флага их не читает.
fn write_primaries(writer: &mut Std140Writer, primaries: Option<&LookPrimaries>) {
    let Some(primaries) = primaries else {
        write_zero_rows(writer, PRIMARIES_ROWS);
        return;
    };
    let encoding = &primaries.encoding;
    writer.vec4([1.0, encoding.toe, encoding.toe_root, encoding.scale]);
    writer.vec4([encoding.inverse_scale, 0.0, 0.0, 0.0]);
    writer.matrix3(primaries.working_to_basis);
    writer.matrix3(primaries.basis_to_working);
    // По строке на зону; в строке — каналы R, G, B (xyz).
    for [red, green, blue] in [primaries.lift, primaries.gamma_slope, primaries.gain, primaries.offset] {
        writer.vec4([red, green, blue, 0.0]);
    }
}

/// Пишет `count` нулевых строк.
fn write_zero_rows(writer: &mut Std140Writer, count: usize) {
    for _ in 0..count {
        writer.vec4([0.0; 4]);
    }
}

/// Пишет пять строк колёс (31–35); нейтральные колёса (`None`) — нули, шейдер их не читает.
fn write_wheels(writer: &mut Std140Writer, wheels: Option<&LookWheels>) {
    match wheels {
        None => write_zero_rows(writer, WHEEL_ROWS),
        Some(wheels) => {
            writer.vec4([
                1.0,
                wheels.pivot_lightness,
                wheels.shadow_window_scale,
                wheels.highlight_window_scale,
            ]);
            for [lightness, chroma_a, chroma_b] in [wheels.lift, wheels.gamma, wheels.gain, wheels.offset] {
                writer.vec4([lightness, chroma_a, chroma_b, 0.0]);
            }
        }
    }
}

/// Пишет массив из [`MAX_LOOK_BANDS`] чисел четырьмя строками `vec4`.
fn write_band_array(writer: &mut Std140Writer, values: &[f32; MAX_LOOK_BANDS]) {
    for row in values.chunks_exact(4) {
        writer.vec4([row[0], row[1], row[2], row[3]]);
    }
}

#[cfg(test)]
mod tests;
