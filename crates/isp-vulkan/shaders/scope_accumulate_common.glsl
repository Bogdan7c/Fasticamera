// Общая часть проходов накопления счётчиков цветовых приборов (решения 183, 188–190).
//
// Контракт «прибор = накопитель + правило попадания»: файл-прибор подключает ЭТОТ файл и определяет ОДНУ функцию
//     void scope_hits(uint eye_x, uvec3 code)
// которая сообщает попадания отсчёта через `scope_add_hit`. `eye_x` — положение отсчёта на ГОРИЗОНТАЛИ ГЛАЗА
// (с учётом зеркала и положения телефона, см. `scan` ниже), а длина этой оси — глобальная `eye_extent`. Всё остальное — обход отсчётов, зеркало, квантование
// значения экрана в код 0…1023, границы сетки, атомарное сложение — общее и не правится при добавлении прибора.
//
// Привязки (одинаковы у всех приборов; Rust-сторона — `isp-vulkan/src/scopes/accumulators.rs`):
//   0  показанная картинка (sampled image, texelFetch; rgb10_a2 или rgba16f — оба читаются как float);
//   1  параметры прибора (слова + таблица декодирования), буфер только для чтения;
//   2  счётчики (u32), сложение `atomicAdd`.
//
// Паритет с CPU-эталоном `scope-model` БИТ В БИТ: каждая вещественная операция в правилах — f32 `+ − ×`
// на `precise` переменных (NoContraction: ни слияния в FMA, ни перестановок), `floor` и сравнения.

// Размер группы — specialization-константы 0 и 1 (`LOCAL_SIZE_X_ID`/`LOCAL_SIZE_Y_ID` в accumulators.rs): Rust
// сам передаёт размер и сам считает по нему число групп, одно число в двух местах не разъедется.
layout(local_size_x_id = 0, local_size_y_id = 1) in;

layout(set = 0, binding = 0) uniform texture2D shown_image;

layout(set = 0, binding = 1, std430) readonly buffer ScopeParameters {
    // Слова 0..2 — форма сетки (столбцы, строки, плоскости), 3 — как глаз читает картинку (SCAN_*), с 4 — слова прибора.
    uint words[64];
    // Таблица «код 0…1023 → свет экрана» (нужна только приборам, которые её читают).
    float decode_table[1024];
} parameters;

layout(set = 0, binding = 2, std430) buffer ScopeCounters {
    uint counts[];
} counters;

// Прореживание и окно чтения (совпадает с Rust-структурой `SamplingPush`, 7 слов).
layout(push_constant) uniform Sampling {
    uint content_width;   // ширина показанной картинки (содержимое окна), пикселей
    uint content_height;  // высота показанной картинки
    uint stride;          // шаг прореживания >= 1
    uint offset;          // смещение первого отсчёта (stride - 1) / 2
    uint samples_x;       // число отсчётов по x
    uint samples_y;       // число отсчётов по y
    uint mirrored;        // 1 — картинка на экране зеркальна по горизонтали
} sampling;

const uint WORD_GRID_COLUMNS = 0u;
const uint WORD_GRID_ROWS = 1u;
const uint WORD_GRID_PLANES = 2u;
// Как глаз читает показанную картинку слева направо (`ScopeScan` в scope-contract; коды — `scan_code` в
// isp-vulkan/src/scopes/parameter_blocks.rs, их сверяет тест шейдерных литералов).
const uint WORD_SCAN = 3u;
const uint SCAN_UPRIGHT = 0u;                 // по столбцам слева направо
const uint SCAN_ROTATED_CLOCKWISE = 1u;       // по строкам сверху вниз
const uint SCAN_UPSIDE_DOWN = 2u;             // по столбцам справа налево
const uint SCAN_ROTATED_COUNTER_CLOCKWISE = 3u; // по строкам снизу вверх
// Первое слово прибора.
const uint WORD_RULE_FIRST = 4u;

// Единое правило квантования значения экрана в код: floor(clamp(v, 0, 1) * 1023 + 0.5); NaN даёт 0
// (clamp от NaN в GLSL не определён, поэтому ветка явная — как `quantize_display_value` в scope-model).
uint quantize_display_value(float value) {
    if (isnan(value)) {
        return 0u;
    }
    precise float clamped = clamp(value, 0.0, 1.0);
    precise float scaled = clamped * 1023.0;
    precise float shifted = scaled + 0.5;
    return uint(floor(shifted));
}

#if defined(SCOPE_MERGE_HITS)
// Слияние одинаковых ячеек внутри subgroup: нити, попавшие в одну ячейку, дают ОДИН `atomicAdd` с числом попаданий.
// Счётчики целые, сумма не зависит от порядка и разбиения, поэтому итог тот же бит в бит. Цикл идёт по различным
// ячейкам subgroup (у вектороскопа соседние пиксели почти всегда в одной ячейке — 1-3 витка).
void add_hit_merged_in_subgroup(uint cell) {
    bool pending = true;
    while (pending) {
        uint leader_cell = subgroupBroadcastFirst(cell);
        bool same_cell = cell == leader_cell;
        uvec4 same_cell_lanes = subgroupBallot(same_cell);
        if (same_cell) {
            pending = false;
            if (subgroupElect()) {
                atomicAdd(counters.counts[cell], subgroupBallotBitCount(same_cell_lanes));
            }
        }
    }
}
#endif

#if defined(SCOPE_ROWS_PER_THREAD)
// СЛИЯНИЕ СЕРИЙ В РЕГИСТРЕ. Нить обходит SCOPE_ROWS_PER_THREAD отсчётов подряд по оси y; подряд идущие попадания
// нити в одну и ту же ячейку одной плоскости копятся в регистре и уходят ОДНИМ `atomicAdd` с числом попаданий,
// когда серия обрывается (или нить закончила). Счётчики целые, сумма не зависит от разбиения: итог тот же бит в бит.
// Плоскостей у приборов не больше трёх (`MAX_PLANES` в Rust): серии заведены по одной на плоскость и выбираются
// ветвлением, без динамической индексации массива регистров.
uint run_cell_0 = 0u;
uint run_count_0 = 0u;
uint run_cell_1 = 0u;
uint run_count_1 = 0u;
uint run_cell_2 = 0u;
uint run_count_2 = 0u;

#define RUN_STEP(run_cell, run_count)                                \
    if (run_count != 0u && run_cell == cell) {                      \
        run_count += 1u;                                             \
    } else {                                                         \
        if (run_count != 0u) {                                       \
            atomicAdd(counters.counts[run_cell], run_count);         \
        }                                                            \
        run_cell = cell;                                             \
        run_count = 1u;                                              \
    }

void add_hit_to_run(uint plane, uint cell) {
    if (plane == 0u) {
        RUN_STEP(run_cell_0, run_count_0)
    } else if (plane == 1u) {
        RUN_STEP(run_cell_1, run_count_1)
    } else if (plane == 2u) {
        RUN_STEP(run_cell_2, run_count_2)
    } else {
        atomicAdd(counters.counts[cell], 1u);
    }
}

// Нить закончила обход: недописанные серии уходят в счётчики.
void flush_runs() {
    if (run_count_0 != 0u) {
        atomicAdd(counters.counts[run_cell_0], run_count_0);
    }
    if (run_count_1 != 0u) {
        atomicAdd(counters.counts[run_cell_1], run_count_1);
    }
    if (run_count_2 != 0u) {
        atomicAdd(counters.counts[run_cell_2], run_count_2);
    }
}
const uint ROWS_PER_THREAD = SCOPE_ROWS_PER_THREAD;
#else
const uint ROWS_PER_THREAD = 1u;
#endif

// Прибавляет попадание. Ячейка вне сетки не пишется (на CPU это ошибка правила, на GPU выход за буфер
// недопустим); у правил из `scope-model` такого не бывает, паритет-тесты это проверяют.
void scope_add_hit(uint plane, uint row, uint column) {
    uint columns = parameters.words[WORD_GRID_COLUMNS];
    uint rows = parameters.words[WORD_GRID_ROWS];
    uint planes = parameters.words[WORD_GRID_PLANES];
    if (plane >= planes || row >= rows || column >= columns) {
        return;
    }
    uint cell = (plane * rows + row) * columns + column;
#if defined(SCOPE_DIAG_SKIP_WRITE)
    // ДИАГНОСТИКА стенда: ячейка посчитана, запись не делается (ветка не выполняется никогда, но компилятор её не уберёт).
    if (cell == 0xFFFFFFFFu) {
        atomicAdd(counters.counts[0], 1u);
    }
#elif defined(SCOPE_DIAG_WRITE_CELL_ZERO)
    // ДИАГНОСТИКА стенда: все записи в одну ячейку (худшая конкуренция), проекция всё равно считается.
    if (cell != 0xFFFFFFFFu) {
        atomicAdd(counters.counts[0], 1u);
    }
#elif defined(SCOPE_DIAG_WRITE_SPREAD)
    // ДИАГНОСТИКА стенда: у каждой нити своя ячейка (конкуренции нет), проекция всё равно считается.
    if (cell != 0xFFFFFFFFu) {
        uint own = (gl_GlobalInvocationID.y * sampling.samples_x + gl_GlobalInvocationID.x) * 3u + plane;
        atomicAdd(counters.counts[own % (rows * columns * planes)], 1u);
    }
#elif defined(SCOPE_MERGE_HITS)
    add_hit_merged_in_subgroup(cell);
#elif defined(SCOPE_ROWS_PER_THREAD)
    add_hit_to_run(plane, cell);
#else
    atomicAdd(counters.counts[cell], 1u);
#endif
}

// Правило попадания прибора: определяется в файле прибора.
void scope_hits(uint eye_x, uvec3 code);

// Длина оси «слева направо» для глаза: ширина или высота показанной картинки (ставится в main до scope_hits).
uint eye_extent = 1u;

// Положение отсчёта (shown_x, shown_y) показанной картинки на горизонтали глаза — то же, что
// `SamplePosition::from_shown` в scope-model. Зеркало уже учтено в shown_x, поворот накладывается поверх.
uint eye_position(uint shown_x, uint shown_y, uint scan) {
    bool along_rows = scan == SCAN_ROTATED_CLOCKWISE || scan == SCAN_ROTATED_COUNTER_CLOCKWISE;
    bool reversed = scan == SCAN_UPSIDE_DOWN || scan == SCAN_ROTATED_COUNTER_CLOCKWISE;
    eye_extent = along_rows ? sampling.content_height : sampling.content_width;
    uint along = along_rows ? shown_y : shown_x;
    return reversed ? eye_extent - 1u - along : along;
}

float word_as_float(uint index) {
    return uintBitsToFloat(parameters.words[index]);
}

void main() {
    // Нить обходит ROWS_PER_THREAD отсчётов подряд по y (1 — продуктовый обход «одна нить — один отсчёт»).
    uint first_sample_y = gl_GlobalInvocationID.y * ROWS_PER_THREAD;
    if (gl_GlobalInvocationID.x >= sampling.samples_x) {
        return;
    }
    uint scan = parameters.words[WORD_SCAN];
    for (uint step = 0u; step < ROWS_PER_THREAD; ++step) {
        uvec2 sample_index = uvec2(gl_GlobalInvocationID.x, first_sample_y + step);
        if (sample_index.y >= sampling.samples_y) {
            break;
        }
        // Координаты отсчёта на ПОКАЗАННОЙ картинке (так же, как `SampleStride::coordinate` на CPU).
        uint shown_x = sampling.offset + sample_index.x * sampling.stride;
        uint shown_y = sampling.offset + sample_index.y * sampling.stride;
        // Зеркальная картинка: показанный столбец x лежит в буфере на content_width - 1 - x.
        uint texel_x = sampling.mirrored != 0u ? sampling.content_width - 1u - shown_x : shown_x;
        vec4 texel = texelFetch(shown_image, ivec2(int(texel_x), int(shown_y)), 0);
        uvec3 code = uvec3(quantize_display_value(texel.r), quantize_display_value(texel.g), quantize_display_value(texel.b));
        uint eye_x = eye_position(shown_x, shown_y, scan);
        scope_hits(eye_x, code);
    }
#if defined(SCOPE_ROWS_PER_THREAD)
    flush_runs();
#endif
}
