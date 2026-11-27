// Целочисленная проекция кода, общая для правил waveform, parade, вектороскопа Y'CbCr и гистограммы. Подключается ПОСЛЕ
// `scope_accumulate_common.glsl` файлом правила (`scope_accumulate_integer_rule.glsl`, `scope_accumulate_histogram_rule.glsl`).
// CPU-эталон: `scope_model::projection`. Слова проекции в блоке параметров — `w0 w1 w2 offset shift`.
const uint PROJECTION_WORDS = 5u;

// Проекция `(w0*c0 + w1*c1 + w2*c2 + offset) >> shift`, всё в int; `>>` над знаковым числом арифметический. Веса и
// смещение — `xyzw`, сдвиг — отдельно.
struct Projection {
    ivec4 weights_offset;
    int shift;
};

int projection_value(Projection projection, ivec3 code) {
    ivec4 w = projection.weights_offset;
    return (w.x * code.x + w.y * code.y + w.z * code.z + w.w) >> projection.shift;
}

Projection projection_at(uint first_word) {
    return Projection(
        ivec4(
            int(parameters.words[first_word]),
            int(parameters.words[first_word + 1u]),
            int(parameters.words[first_word + 2u]),
            int(parameters.words[first_word + 3u])),
        int(parameters.words[first_word + 4u]));
}
