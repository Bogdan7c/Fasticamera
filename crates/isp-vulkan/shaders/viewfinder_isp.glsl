// ISP видоискателя (сессия 07): ОДИН слитый compute-проход RAW → изображение видоискателя.
//
// Формулы — эталон `isp-model` (вариант Preview), стадия за стадией, в том же порядке и в f32:
//   уровни → коррекция виньетирования (lens_shading.glsl) → баланс белого + обрезка → демозаик 2×2 → камера→XYZ D50 → XYZ→экран (два отдельных
//   произведения, как на CPU) → сжатие охвата → тон с сохранением оттенка → OETF sRGB
// Это прежняя основа. Основа AgX (specialization TONE_BASE_MODEL = 1, tone_base_agx.glsl): обрезка баланса по своему белому
// точки L, XYZ→W (Rec. 2020) → вход → AgX (inset, кривая, outset) → W→цвета экрана → мягкое сжатие охвата → OETF.
// Основа filmic (TONE_BASE_MODEL = 3, tone_base_filmic.glsl): те же шаги, но вместо AgX — норма пикселя → кривая → обесцвечивание
// в светах → вписывание в охват выхода.
// плюс статистика AWB (суммы по плиткам, контракт `isp_model::statistics`) в том же проходе. Статистика: ПЕРЕСВЕТ
// ячейки судится по нормированному отсчёту ДО виньетирования (пересвечивает сенсор, а не наше усиление),
// СУММЫ — по скорректированному (после виньетирования, до баланса белого) с потолком `headroom`, не 1.
//
// Одна нить = один пиксель ВЫХОДА = одна ячейка CFA 2×2 кадрирования. Выход прямой (upright): нить
// по координатам выхода находит ячейку сенсора обратным поворотом (ориентация сенсора).
//
// Включающий файл (`viewfinder_<источник>_<формат>[_subgroup].comp`):
//   * вариант `_subgroup` объявляет `#define AWB_SUBGROUP_REDUCTION` и расширения
//     GL_KHR_shader_subgroup_basic/arithmetic (только если устройство поддерживает арифметику subgroup
//     в compute — проверяет Rust при создании ISP); без него — запасная схема статистики через shared-окно;
//   * ДО #include объявляет привязку 2: `layout(set = 0, binding = 2, <формат>) uniform writeonly image2D
//     viewfinder_image;` (формат — по StorageImageFormat::glsl_format_qualifier);
//   * ПОСЛЕ #include подключает источник `viewfinder_source_<raw10|raw16>.glsl`: привязку 0 (кадр
//     камеры) и тело `read_raw` (прототип ниже) — значение RAW пикселя (x, y) в координатах буфера.
//
// Раскладка uniform-блока (binding 1) — зеркало `src/viewfinder/uniforms.rs`; менять только вместе.

// --- Specialization-константы: размер рабочей группы и выключатели стадий (IspStageSet) --------------
layout(local_size_x_id = 0, local_size_y_id = 1) in;
layout(constant_id = 2) const bool STAGE_LEVELS = true;
layout(constant_id = 3) const bool STAGE_WHITE_BALANCE = true;
layout(constant_id = 4) const bool STAGE_DEMOSAIC = true;
layout(constant_id = 5) const bool STAGE_COLOR = true;
layout(constant_id = 6) const bool STAGE_GAMUT_COMPRESSION = true;
layout(constant_id = 7) const bool STAGE_TONE = true;
layout(constant_id = 8) const bool STAGE_OETF = true;
layout(constant_id = 9) const bool STAGE_AWB_STATISTICS = true;
layout(constant_id = 10) const bool STAGE_LENS_SHADING = true;
layout(constant_id = 11) const bool STAGE_OUTPUT_GAMUT_COMPRESSION = true;
// Слой вида (решения 98, 99, 176): стадии над светом экрана после основы тона. Нейтраль цвета — флаг в uniform
// (не константа: первое касание ручки не меняет вариант); таблица — вариант шейдера «с ней / без» (константа).
layout(constant_id = 13) const bool STAGE_LOOK_COLOR = true;
layout(constant_id = 14) const bool STAGE_LOOK_LUT = true;
// Модель основы тона (не стадия, а выбор формулы: другая модель — другой вариант пайплайна, решения 176, 182).
// 0 — прежняя кривая и обрезка по 1,0; 1 — AgX в рабочем пространстве Rec. 2020 со своим белым точки (решение 177).
// Номера моделей — `TONE_MODEL_*` в `tone_base.glsl` (зеркало `ToneBaseKind::model_id` в Rust).
layout(constant_id = 12) const uint TONE_BASE_MODEL = 0u;
// Каждая рабочая группа лежит целиком в одной плитке статистики AWB (решает CPU по геометрии кадра: `AwbGroupLayout`).
// Тогда вместо общего пути с окном плиток и определением плитки по нитям работает короткий путь
// `accumulate_awb_group_in_one_tile`; код общего пути из варианта вырезается. Только для варианта без subgroup.
layout(constant_id = 16) const bool AWB_GROUPS_IN_ONE_TILE = false;

// --- Параметры кадра (std140, только 32-битные типы: у Adreno 740 нет 16-битного доступа к uniform) ---
layout(set = 0, binding = 1, std140) uniform ViewfinderParameters {
    vec4 black_level;              // чёрный уровень по позициям ячейки 2×2 (порядок строк)
    vec4 level_range;              // white − black по позициям (посчитано на CPU в f32, как в эталоне)
    vec4 white_balance_gain;       // усиление баланса для цвета каждой позиции
    vec4 levels_misc;              // x — белый уровень, y — уровень обрезки после баланса, z — порог пересвета AWB, w — потолок сумм AWB (запас)
    vec4 camera_to_xyz[3];         // строки матрицы камера (после баланса) → XYZ D50, w = 0
    vec4 xyz_to_display[3];        // строки матрицы XYZ D50 → линейный свет экрана, w = 0
    vec4 gamut_threshold_power;    // xyz — порог по каналам, w — степень колена p
    vec4 gamut_scale_inverse_power;// xyz — масштаб s по каналам, w — 1/p (посчитано на CPU)
    vec4 tone;                     // x — контраст c, y — s^c, z — 1/F(x_white), w — доля сохранения оттенка
    uvec4 crop;                    // кадрирование в пикселях буфера: left, top, width, height (чётные)
    uvec4 output_geometry;         // x, y — размер выхода; z — поворот сенсора (четверти по часовой); w — шаг строки RAW10, байт
    uvec4 channel_of_position;     // цвет позиции ячейки: 0 — красный, 1 — зелёный, 2 — синий
    uvec4 awb_grid;                // x — столбцов плиток, y — строк плиток
    uvec4 lens_shading_origin;     // x, y — левый верхний угол полезной области в пикселях буфера (чётный)
} parameters;

// --- Карта коррекции виньетирования (storage-буфер слота, привязка 4; раскладка — lens_shading.glsl) ---
#define LENS_SHADING_MAP_BINDING 4
#include "lens_shading.glsl"
// Блок основы тона (uniform `ToneBaseParameters`, привязка 5; раскладка — `src/tone_base/mod.rs`). Блок привязан
// всегда: у прежней основы шейдер читает из него только баланс белого.
#define TONE_BASE_PARAMS_BINDING 5
#include "tone_base_agx.glsl"
#include "tone_base_aces.glsl"
#include "tone_base_filmic.glsl"
// Слой вида: блок цвета (привязка 6) и таблица (привязка 7); раскладки — `src/look/`.
#define LOOK_PARAMS_BINDING 6
#define LOOK_LUT_BINDING 7
#include "look_stage.glsl"
// Общий хвост цвета (охват, тон, OETF): те же формулы, что в выходном проходе качественного пути и снимка.
#include "color_tail.glsl"

// --- Статистика AWB: плитки по 5 u32 {sum_r, sum_g, sum_b, used_cells, saturated_cells} (isp-model AwbTile) ---
layout(set = 0, binding = 3, std430) buffer AwbTiles {
    uint words[];
} awb_tiles;

const uint CHANNEL_RED = 0u;
const uint CHANNEL_GREEN = 1u;
const uint CHANNEL_BLUE = 2u;

const uint AWB_WORDS_PER_TILE = 5u;
const uint AWB_SUM_R = 0u;
const uint AWB_SUM_G = 1u;
const uint AWB_SUM_B = 2u;
const uint AWB_USED_CELLS = 3u;
const uint AWB_SATURATED_CELLS = 4u;
// Шаг квантования статистики: 12 бит (isp_model::statistics::QUANT_SCALE).
const float AWB_QUANT_SCALE = 4095.0;

// Окно плиток в shared-памяти рабочей группы: группа (по умолчанию 128 нитей) при плитках от ~64 ячеек
// задевает 1–4 плитки; окно 4×4 покрывает их с запасом. Плитки вне окна (мелкая сетка на маленьком кадре)
// идут сразу в глобальный atomicAdd — медленнее, но всегда верно.
const uint AWB_LOCAL_TILE_SPAN = 4u;
const uint AWB_LOCAL_WORDS = AWB_LOCAL_TILE_SPAN * AWB_LOCAL_TILE_SPAN * AWB_WORDS_PER_TILE;
const uint NO_TILE = 0xFFFFFFFFu;

shared uint awb_window_origin_x;
shared uint awb_window_origin_y;
shared uint awb_window_sums[AWB_LOCAL_WORDS];

// Значение RAW пикселя (x, y) в координатах буфера; тело — в `viewfinder_source_*.glsl`.
uint read_raw(uint x, uint y);
// Четыре отсчёта ячейки 2×2 с левым верхним углом `origin` (чётные координаты буфера): [0] — (x, y), [1] — (x+1, y),
// [2] — (x, y+1), [3] — (x+1, y+1). Источник читает пару соседних отсчётов строки за один заход (см. `viewfinder_source_*`).
uvec4 read_raw_cell(uvec2 origin);

// ===== Геометрия ===================================================================================

// Ячейка кадрирования (в ячейках, ориентация сенсора) для пикселя прямого выхода. Обратное к повороту
// по часовой на `turns` четвертей (тот же расчёт, что шаг 5 отладочной мозаики `raw_mosaic_main.glsl`).
uvec2 cell_of_output(uvec2 upright, uint turns, uint cells_width, uint cells_height) {
    if (turns == 1u) {
        return uvec2(upright.y, cells_height - 1u - upright.x);
    }
    if (turns == 2u) {
        return uvec2(cells_width - 1u - upright.x, cells_height - 1u - upright.y);
    }
    if (turns == 3u) {
        return uvec2(cells_width - 1u - upright.y, upright.x);
    }
    return upright;
}

// ===== Стадия 1: уровни ===========================================================================

// max(raw − black, 0) / (white − black) — isp_model::levels::normalize_sample. `precise` запрещает
// компилятору заменить деление умножением на обратное (AllowRecip): это расходилось бы с эталоном
// сильнее неизбежных 2,5 ULP OpFDiv и ломало бы точность статистики AWB.
// Выключено: raw / white без вычитания чёрного (картинка «приподнята», цена стадии — вычитание и max).
float normalize_sample(uint raw_value, uint position) {
    if (STAGE_LEVELS) {
        precise float above_black = max(float(raw_value) - parameters.black_level[position], 0.0);
        precise float normalized = above_black / parameters.level_range[position];
        return normalized;
    }
    return float(raw_value) / parameters.levels_misc.x;
}

// ===== Стадия 2: коррекция виньетирования (isp_model::lens_shading) ================================

// Умножает четыре отсчёта ячейки на усиление карты — ОДНА билинейная выборка на ячейку в её центре
// `(x0 + 0,5; y0 + 0,5)` (`isp_model::lens_shading::LensShadingSampling::PerCellCentre`), канал карты —
// свой у каждого отсчёта. Видоискатель отдаёт один пиксель на ячейку, так что покадровая выборка четырёх
// отсчётов даёт разницу долей процента (меряется тестом на настоящей калибровке) за вчетверо большую цену.
// `cell_origin` — левый верхний пиксель ячейки в буфере СЕНСОРА (до любого поворота выхода: нить уже нашла
// ячейку обратным поворотом, а карта лежит над полезной областью в осях сенсора). Начало полезной области
// и кадрирования чётные (проверяет Rust: геометрия кратна ячейке 2×2), поэтому ячейка начинается с чётных
// координат полезной области, а строка отсчёта позиции p в полезной области — чётная при p < 2, нечётная иначе.
// `sample_count` — сколько позиций реально прочитано (без демозаика и статистики — одна). Выключено
// (константа) или в буфере заглушка «пропуск» — значения как есть.
// Усиления четырёх каналов карты в центре ячейки считаются ОДИН раз (`cell_lens_shading_gains`): они же дают
// свой белый точки (решение 177). Нет коррекции — все четыре равны 1.
vec4 cell_lens_shading_gains(uvec2 cell_origin) {
    if (!STAGE_LENS_SHADING || !lens_shading_enabled()) {
        return vec4(1.0);
    }
    uvec2 valid_origin = cell_origin - parameters.lens_shading_origin.xy;
    return lens_shading_gains(lens_shading_axis_cell_centre_x(valid_origin.x),
                              lens_shading_axis_cell_centre_y(valid_origin.y));
}

// Умножает отсчёты ячейки на усиления карты в её центре (`gains` — результат `cell_lens_shading_gains`), канал
// карты — свой у каждого отсчёта. Коррекция выключена — значения как есть.
vec4 apply_lens_shading(vec4 normalized, uvec2 cell_origin, uint sample_count, vec4 gains) {
    if (!STAGE_LENS_SHADING || !lens_shading_enabled()) {
        return normalized;
    }
    uvec2 valid_origin = cell_origin - parameters.lens_shading_origin.xy;
    vec4 corrected = normalized;
    for (uint position = 0u; position < sample_count; ++position) {
        uint row = position >> 1u;
        uint map_channel = lens_shading_map_channel(parameters.channel_of_position[position], valid_origin.y + row);
        precise float shaded = normalized[position] * gains[map_channel];
        corrected[position] = shaded;
    }
    return corrected;
}

// ===== Статистика AWB (isp_model::statistics) =====================================================

// q = floor(clamp(v, 0, headroom) · 4095 + 0,5), v — СКОРРЕКТИРОВАННЫЙ отсчёт (масштаб 4095 на единицу света
// сохранён, потолок — запас `headroom` из конфига, а не белый): умножение и сложение — ОТДЕЛЬНЫЕ операции с округлением
// (`precise` = NoContraction, без слияния в FMA), иначе значения у границы .5 округлялись бы иначе.
uint quantize_awb_sample(float corrected) {
    precise float scaled = clamp(corrected, 0.0, parameters.levels_misc.w) * AWB_QUANT_SCALE;
    precise float rounded = floor(scaled + 0.5);
    return uint(rounded);
}

// Пересвет: !(v < порог) — NaN тоже исключает ячейку, как в эталоне. v — НОРМИРОВАННЫЙ отсчёт до виньетирования.
bool is_saturated_sample(float normalized) {
    return !(normalized < parameters.levels_misc.z);
}

uvec2 tile_of_cell(uvec2 cell, uint cells_width, uint cells_height) {
    return uvec2(cell.x * parameters.awb_grid.x / cells_width, cell.y * parameters.awb_grid.y / cells_height);
}

// Слово плитки `tile` поля `field` в глобальном буфере статистики.
uint awb_global_word(uvec2 tile, uint field) {
    return (tile.y * parameters.awb_grid.x + tile.x) * AWB_WORDS_PER_TILE + field;
}

// Вклад одной ячейки в поля плитки: `sums_and_used` = {sum_r, sum_g, sum_b, used_cells}, `saturated` —
// saturated_cells. Ячейка с пересветом хоть в одной позиции ИСХОДНОГО (до виньетирования) отсчёта идёт только в
// saturated (isp_model); суммы — по скорректированным отсчётам. Нить вне выхода (`inside` = false) даёт нули:
// так её можно складывать вместе со всеми без ветвления.
void awb_cell_contribution(bool inside, vec4 normalized, vec4 corrected, out uvec4 sums_and_used, out uint saturated) {
    sums_and_used = uvec4(0u);
    saturated = 0u;
    if (!inside) {
        return;
    }
    bool any_saturated = false;
    for (uint position = 0u; position < 4u; ++position) {
        any_saturated = any_saturated || is_saturated_sample(normalized[position]);
    }
    if (any_saturated) {
        saturated = 1u;
        return;
    }
    for (uint position = 0u; position < 4u; ++position) {
        sums_and_used[parameters.channel_of_position[position]] += quantize_awb_sample(corrected[position]);
    }
    sums_and_used[AWB_USED_CELLS] = 1u;
}

// --- Определение пути: вся группа в одной плитке? ----------------------------------------------------
//
// Зависит только от номера группы и uniform-параметров — значит, ОДИНАКОВО у всех нитей группы (ветвление
// по нему однородно: barrier() в медленном пути и subgroup-операции в быстром исполняют все нити группы).
// Нити группы, попавшие в выход, образуют прямоугольник выхода; поворот переводит прямоугольник в
// прямоугольник ячеек, а его противоположные углы — в противоположные углы. Номер плитки
// floor(c · G / W) не убывает по каждой оси, поэтому все ячейки прямоугольника лежат в одной плитке
// тогда и только тогда, когда в одной плитке лежат его минимальный и максимальный углы.
// Возвращает эту плитку или NO_TILE (группа задевает несколько плиток).
uvec2 workgroup_single_tile(uint cells_width, uint cells_height) {
    uvec2 output_size = parameters.output_geometry.xy;
    uvec2 first_output = gl_WorkGroupID.xy * gl_WorkGroupSize.xy;
    // Диспетчер покрывает выход ceil-делением: первая нить группы всегда внутри выхода.
    uvec2 last_output = min(first_output + gl_WorkGroupSize.xy - 1u, output_size - 1u);
    uint turns = parameters.output_geometry.z;
    uvec2 corner_a = cell_of_output(first_output, turns, cells_width, cells_height);
    uvec2 corner_b = cell_of_output(last_output, turns, cells_width, cells_height);
    uvec2 tile_low = tile_of_cell(min(corner_a, corner_b), cells_width, cells_height);
    uvec2 tile_high = tile_of_cell(max(corner_a, corner_b), cells_width, cells_height);
    if (tile_low != tile_high) {
        return uvec2(NO_TILE);
    }
    return tile_low;
}

#ifdef AWB_SUBGROUP_REDUCTION
// --- Быстрый путь: вся группа в одной плитке, сумма по subgroup -----------------------------------------
//
// Каждое поле складывается subgroupAdd по всем активным нитям subgroup (нити вне выхода дают нули — они
// НЕ выходят раньше, иначе пропали бы из subgroup), затем одна выбранная нить (subgroupElect) делает
// по одному глобальному atomicAdd на ненулевое поле. Ни shared-памяти, ни barrier(). Размер subgroup
// не важен (любой, хоть меняющийся между dispatch-ами, SPIR-V 1.6): каждый subgroup добавляет ровно
// свою сумму. Целые суммы u32 не зависят от порядка сложения — результат совпадает с эталоном бит в бит.
void accumulate_awb_single_tile(bool inside, uvec2 tile, vec4 normalized, vec4 corrected) {
    uvec4 sums_and_used;
    uint saturated;
    awb_cell_contribution(inside, normalized, corrected, sums_and_used, saturated);
    uvec4 subgroup_sums_and_used = subgroupAdd(sums_and_used);
    uint subgroup_saturated = subgroupAdd(saturated);
    if (subgroupElect()) {
        for (uint field = AWB_SUM_R; field <= AWB_USED_CELLS; ++field) {
            if (subgroup_sums_and_used[field] != 0u) {
                atomicAdd(awb_tiles.words[awb_global_word(tile, field)], subgroup_sums_and_used[field]);
            }
        }
        if (subgroup_saturated != 0u) {
            atomicAdd(awb_tiles.words[awb_global_word(tile, AWB_SATURATED_CELLS)], subgroup_saturated);
        }
    }
}
#endif

// --- Медленный (и запасной) путь: shared-окно плиток ----------------------------------------------------

void add_to_awb_tile(uvec2 tile, uint field, uint value) {
    if (value == 0u) {
        return;
    }
    uvec2 local_tile = tile - uvec2(awb_window_origin_x, awb_window_origin_y);
    if (local_tile.x < AWB_LOCAL_TILE_SPAN && local_tile.y < AWB_LOCAL_TILE_SPAN) {
        uint local_word = (local_tile.y * AWB_LOCAL_TILE_SPAN + local_tile.x) * AWB_WORDS_PER_TILE + field;
        atomicAdd(awb_window_sums[local_word], value);
    } else {
        atomicAdd(awb_tiles.words[awb_global_word(tile, field)], value);
    }
}

// Вклад ячейки в плитку: сначала в shared-окно группы, затем одна запись atomicAdd на ненулевое слово
// окна. Вызывают ВСЕ нити группы (внутри barrier()), `inside` — нить внутри выхода.
void accumulate_awb_window(bool inside, uvec2 cell, vec4 normalized, vec4 corrected, uint cells_width,
                           uint cells_height) {
    uint thread_index = gl_LocalInvocationIndex;
    uint thread_count = gl_WorkGroupSize.x * gl_WorkGroupSize.y;
    for (uint word = thread_index; word < AWB_LOCAL_WORDS; word += thread_count) {
        awb_window_sums[word] = 0u;
    }
    if (thread_index == 0u) {
        awb_window_origin_x = NO_TILE;
        awb_window_origin_y = NO_TILE;
    }
    barrier();

    uvec2 tile = uvec2(0u);
    if (inside) {
        tile = tile_of_cell(cell, cells_width, cells_height);
        atomicMin(awb_window_origin_x, tile.x);
        atomicMin(awb_window_origin_y, tile.y);
    }
    barrier();

    if (inside) {
        uvec4 sums_and_used;
        uint saturated;
        awb_cell_contribution(inside, normalized, corrected, sums_and_used, saturated);
        for (uint field = AWB_SUM_R; field <= AWB_USED_CELLS; ++field) {
            add_to_awb_tile(tile, field, sums_and_used[field]);
        }
        add_to_awb_tile(tile, AWB_SATURATED_CELLS, saturated);
    }
    barrier();

    // Окно → глобальный буфер: одна атомарная операция на ненулевое слово вместо одной на нить.
    for (uint word = thread_index; word < AWB_LOCAL_WORDS; word += thread_count) {
        uint value = awb_window_sums[word];
        if (value != 0u) {
            uint slot = word / AWB_WORDS_PER_TILE;
            uint field = word % AWB_WORDS_PER_TILE;
            uvec2 window_tile = uvec2(awb_window_origin_x + slot % AWB_LOCAL_TILE_SPAN,
                                      awb_window_origin_y + slot / AWB_LOCAL_TILE_SPAN);
            atomicAdd(awb_tiles.words[awb_global_word(window_tile, field)], value);
        }
    }
}

// Плитка, в которой лежит ВСЯ рабочая группа (при `AWB_GROUPS_IN_ONE_TILE`): плитка ячейки первой нити группы.
// Зависит только от номера группы — однородно по группе, барьеры ниже безопасны.
uvec2 workgroup_tile(uint cells_width, uint cells_height) {
    uvec2 first_output = gl_WorkGroupID.xy * gl_WorkGroupSize.xy;
    uvec2 first_cell = cell_of_output(first_output, parameters.output_geometry.z, cells_width, cells_height);
    return tile_of_cell(first_cell, cells_width, cells_height);
}

// Слова плитки, накопленные группой: поле за полем (порядок `AWB_SUM_R` … `AWB_SATURATED_CELLS`).
shared uint awb_group_sums[AWB_WORDS_PER_TILE];

// Короткий путь статистики: вся группа в плитке `tile`. Нить кладёт вклад ячейки в пять слов shared-памяти
// (`atomicAdd`, нули пропускаются), после барьера пять нитей группы переносят слова в глобальный буфер —
// одна запись на поле на группу вместо одной на subgroup. Целые суммы u32 не зависят от порядка сложения:
// результат совпадает с общим путём и с эталоном бит в бит. Вызывают ВСЕ нити группы (внутри barrier()),
// `inside` — нить внутри выхода (вне выхода вклад нулевой).
void accumulate_awb_group_in_one_tile(bool inside, uvec2 tile, vec4 normalized, vec4 corrected) {
    uvec4 sums_and_used;
    uint saturated;
    awb_cell_contribution(inside, normalized, corrected, sums_and_used, saturated);
    if (gl_LocalInvocationIndex < AWB_WORDS_PER_TILE) {
        awb_group_sums[gl_LocalInvocationIndex] = 0u;
    }
    barrier();
    for (uint field = AWB_SUM_R; field <= AWB_USED_CELLS; ++field) {
        if (sums_and_used[field] != 0u) {
            atomicAdd(awb_group_sums[field], sums_and_used[field]);
        }
    }
    if (saturated != 0u) {
        atomicAdd(awb_group_sums[AWB_SATURATED_CELLS], saturated);
    }
    barrier();
    if (gl_LocalInvocationIndex < AWB_WORDS_PER_TILE) {
        uint total = awb_group_sums[gl_LocalInvocationIndex];
        if (total != 0u) {
            atomicAdd(awb_tiles.words[awb_global_word(tile, gl_LocalInvocationIndex)], total);
        }
    }
}

// Статистика AWB ячейки нити. Вызывают ВСЕ нити группы, включая нити вне выхода (они дают нули).
// С арифметикой subgroup группа целиком в одной плитке (типично: плитки по 64×64 ячейки) идёт быстрым
// путём; группа на стыке плиток (или вариант без subgroup) — через shared-окно.
void accumulate_awb_statistics(bool inside, uvec2 cell, vec4 normalized, vec4 corrected, uint cells_width,
                               uint cells_height) {
#ifdef AWB_SUBGROUP_REDUCTION
    uvec2 single_tile = workgroup_single_tile(cells_width, cells_height);
    if (single_tile.x != NO_TILE) {
        accumulate_awb_single_tile(inside, single_tile, normalized, corrected);
        return;
    }
#endif
    if (AWB_GROUPS_IN_ONE_TILE) {
        accumulate_awb_group_in_one_tile(inside, workgroup_tile(cells_width, cells_height), normalized, corrected);
        return;
    }
    accumulate_awb_window(inside, cell, normalized, corrected, cells_width, cells_height);
}

// ===== Стадии 3–4: баланс белого и демозаик =======================================================

// Уровень обрезки после баланса: у прежней основы — общий (isp_model::white_balance::SENSOR_CLIP_LEVEL), у AgX —
// свой белый точки (`white_level`, isp_model::white_level).
float white_clip_level(float white_level) {
    return TONE_BASE_MODEL != TONE_MODEL_LEGACY ? white_level : parameters.levels_misc.y;
}

// min(v · gain, clip) — isp_model::white_balance / white_level. Выключено: значение как есть (ни усиления, ни обрезки).
vec4 apply_white_balance(vec4 normalized, float clip_level) {
    if (!STAGE_WHITE_BALANCE) {
        return normalized;
    }
    return min(normalized * parameters.white_balance_gain, vec4(clip_level));
}

// Суперпиксель 2×2: R и B — отсчёты своих позиций, G — среднее двух зелёных (isp_model superpixel).
// Выключено: отсчёт позиции 0 во все три канала (картинка серая/мозаичная — цена стадии видна по A/B).
vec3 demosaic_cell(vec4 balanced) {
    if (!STAGE_DEMOSAIC) {
        return vec3(balanced[0]);
    }
    vec3 color = vec3(0.0);
    float green_sum = 0.0;
    for (uint position = 0u; position < 4u; ++position) {
        uint channel = parameters.channel_of_position[position];
        if (channel == CHANNEL_GREEN) {
            green_sum += balanced[position];
        } else {
            color[channel] = balanced[position];
        }
    }
    color.g = green_sum / 2.0;
    return color;
}

// ===== Стадия 5: цвет =============================================================================

// Две последовательные матрицы, как в эталоне (не склеиваются: так паритет сравнивает те же числа).
// Выключено: RGB камеры после баланса идёт дальше как «свет экрана».
vec3 apply_color(vec3 camera_rgb) {
    if (!STAGE_COLOR) {
        return camera_rgb;
    }
    vec3 xyz = vec3(dot(parameters.camera_to_xyz[0].xyz, camera_rgb),
                    dot(parameters.camera_to_xyz[1].xyz, camera_rgb),
                    dot(parameters.camera_to_xyz[2].xyz, camera_rgb));
    return vec3(dot(parameters.xyz_to_display[0].xyz, xyz),
                dot(parameters.xyz_to_display[1].xyz, xyz),
                dot(parameters.xyz_to_display[2].xyz, xyz));
}

// Цвет для основы AgX: те же две матрицы подряд, но вторая — XYZ D50 → рабочее пространство W (Rec. 2020), а не
// сразу цвета экрана. Выключено: RGB камеры идёт дальше как свет сцены в W.
vec3 apply_color_working(vec3 camera_rgb) {
    if (!STAGE_COLOR) {
        return camera_rgb;
    }
    vec3 xyz = vec3(dot(parameters.camera_to_xyz[0].xyz, camera_rgb),
                    dot(parameters.camera_to_xyz[1].xyz, camera_rgb),
                    dot(parameters.camera_to_xyz[2].xyz, camera_rgb));
    if (TONE_BASE_MODEL == TONE_MODEL_FILMIC) {
        return filmic_apply_matrix(FILMIC_ROW_XYZ_D50_TO_WORKING, xyz);
    }
    if (TONE_BASE_MODEL == TONE_MODEL_ACES) {
        return aces_apply_matrix(ACES_ROW_XYZ_D50_TO_WORKING, xyz);
    }
    return agx_apply_matrix(AGX_ROW_XYZ_D50_TO_WORKING, xyz);
}

// ===== Стадии 6а, 6б, 7: охват, тон, OETF =========================================================
// Формулы — в общем `color_tail.glsl` (isp_model::gamut / tone / transfer); здесь только выключатели
// стадий и выбор параметров из uniform-блока видоискателя.

// Выключено: свет как есть (отрицательные каналы потом обнулит тон или OETF).
vec3 compress_gamut(vec3 scene) {
    if (!STAGE_GAMUT_COMPRESSION) {
        return scene;
    }
    return color_tail_compress_gamut(
        scene,
        parameters.gamut_threshold_power.xyz,
        parameters.gamut_scale_inverse_power.xyz,
        parameters.gamut_threshold_power.w,
        parameters.gamut_scale_inverse_power.w);
}

// Выключено: свет сцены идёт на экран как есть (без S-кривой и без сохранения оттенка).
vec3 apply_tone(vec3 scene) {
    if (!STAGE_TONE) {
        return scene;
    }
    return color_tail_apply_tone(scene, parameters.tone);
}

// ----- Основа AgX: те же стадии 6а, 6б (+ новая «выходное сжатие охвата») в своих формулах `tone_base_agx.glsl` -----

// 6а (AgX): вход в W — обрезка нулём или мягкое сжатие. Выключено: свет как есть.
vec3 apply_agx_input(vec3 working_scene) {
    if (!STAGE_GAMUT_COMPRESSION) {
        return working_scene;
    }
    return agx_apply_input(working_scene);
}

// 6б (AgX): inset → кривая со своим белым → outset. Выключено: свет как есть (без кривой).
vec3 apply_agx_tone(vec3 entering, float white_level) {
    if (!STAGE_TONE) {
        return entering;
    }
    return agx_apply_base(entering, white_level);
}

// 6в (AgX): W → цвета экрана, мягкое сжатие охвата выхода, обрезка 0…1. Выключено сжатие охвата — только
// матрица и обрезка (цена сжатия видна по A/B).
vec3 apply_agx_output(vec3 display_working) {
    vec3 output_color = agx_working_to_output(display_working);
    if (STAGE_OUTPUT_GAMUT_COMPRESSION) {
        output_color = agx_compress_output_gamut(output_color);
    }
    return agx_clamp_output(output_color);
}

// ----- Основа filmic: те же стадии 6а, 6б, 6в в формулах `tone_base_filmic.glsl` -----

// 6а (filmic): вход в W — обрезка нулём или мягкое сжатие. Выключено: свет как есть.
vec3 apply_filmic_input(vec3 working_scene) {
    if (!STAGE_GAMUT_COMPRESSION) {
        return working_scene;
    }
    return filmic_apply_input(working_scene);
}

// 6б (filmic): норма → кривая со своим белым → обесцвечивание → вписывание. Выключено: свет как есть (без кривой).
vec3 apply_filmic_tone(vec3 entering, float white_level) {
    if (!STAGE_TONE) {
        return entering;
    }
    return filmic_apply_base(entering, white_level);
}

// 6в (filmic): W → цвета экрана, мягкое сжатие охвата выхода, обрезка 0…1 (выключено сжатие — только матрица и обрезка).
vec3 apply_filmic_output(vec3 display_working) {
    vec3 output_color = filmic_working_to_output(display_working);
    if (STAGE_OUTPUT_GAMUT_COMPRESSION) {
        output_color = filmic_compress_output_gamut(output_color);
    }
    return filmic_clamp_output(output_color);
}

// ----- Основа ACES: те же стадии 6а, 6б, 6в в своих формулах `tone_base_aces.glsl` -----

// 6а (ACES): вход в W — обрезка нулём или мягкое сжатие. Выключено: свет как есть.
vec3 apply_aces_input(vec3 working_scene) {
    if (!STAGE_GAMUT_COMPRESSION) {
        return working_scene;
    }
    return aces_apply_input(working_scene);
}

// 6б (ACES): RRT и выходное преобразование со своим белым. Выключено: свет как есть (без кривой).
vec3 apply_aces_tone(vec3 entering, float white_level) {
    if (!STAGE_TONE) {
        return entering;
    }
    return aces_apply_base(entering, white_level);
}

// 6в (ACES): W → цвета экрана, мягкое сжатие охвата выхода, обрезка 0…1 (как у AgX).
vec3 apply_aces_output(vec3 display_working) {
    vec3 output_color = aces_working_to_output(display_working);
    if (STAGE_OUTPUT_GAMUT_COMPRESSION) {
        output_color = aces_compress_output_gamut(output_color);
    }
    return aces_clamp_output(output_color);
}

// ----- Слой вида (общий для обеих основ): цвет, затем таблица -----

// Цвет вида: выключено набором — свет как есть. Нейтральный вид пропускается флагом в uniform (внутри функции).
vec3 apply_look_color(vec3 display_working) {
    if (!STAGE_LOOK_COLOR) {
        return display_working;
    }
    return look_apply_color(display_working);
}

// Таблица вида: вариант шейдера без неё вырезает код целиком (константа), с ней — заголовок буфера тоже проверяется.
vec3 apply_look_lut(vec3 display_working) {
    if (!STAGE_LOOK_LUT) {
        return display_working;
    }
    return look_apply_lut(display_working);
}

// Выключено: линейный свет пишется как есть (UNORM-картинка обрежет его до 0…1 сама).
vec3 apply_oetf(vec3 display_linear) {
    if (!STAGE_OETF) {
        return display_linear;
    }
    return color_tail_encode_output(display_linear);
}

// ===== Проход =====================================================================================

void main() {
    uvec2 upright = gl_GlobalInvocationID.xy;
    uint output_width = parameters.output_geometry.x;
    uint output_height = parameters.output_geometry.y;
    uint cells_width = parameters.crop.z / 2u;
    uint cells_height = parameters.crop.w / 2u;
    // Крайние группы выходят за картинку: такие нити не читают и не пишут, но участвуют в barrier().
    bool inside = upright.x < output_width && upright.y < output_height;

    uvec2 cell = uvec2(0u);
    // normalized — после уровней, ДО виньетирования (по нему судится пересвет AWB); corrected — после
    // виньетирования (суммы AWB и дальнейшая картинка).
    vec4 normalized = vec4(0.0);
    vec4 corrected = vec4(0.0);
    // Усиления карты в центре ячейки (1, если коррекции нет): дают и коррекцию отсчётов, и свой белый точки.
    vec4 shading_gains = vec4(1.0);
    if (inside) {
        cell = cell_of_output(upright, parameters.output_geometry.z, cells_width, cells_height);
        uvec2 origin = parameters.crop.xy + cell * 2u;
        // Без демозаика и без статистики нужна только позиция 0 — читаем один отсчёт вместо четырёх.
        bool needs_whole_cell = STAGE_DEMOSAIC || STAGE_AWB_STATISTICS;
        if (needs_whole_cell) {
            uvec4 raw_cell = read_raw_cell(origin);
            normalized[0] = normalize_sample(raw_cell[0], 0u);
            normalized[1] = normalize_sample(raw_cell[1], 1u);
            normalized[2] = normalize_sample(raw_cell[2], 2u);
            normalized[3] = normalize_sample(raw_cell[3], 3u);
        } else {
            normalized[0] = normalize_sample(read_raw(origin.x, origin.y), 0u);
        }
        // Коррекция виньетирования — до баланса белого (порядок эталона); исходные отсчёты остаются для пересвета.
        shading_gains = cell_lens_shading_gains(origin);
        corrected = apply_lens_shading(normalized, origin, needs_whole_cell ? 4u : 1u, shading_gains);
    }

    if (STAGE_AWB_STATISTICS) {
        accumulate_awb_statistics(inside, cell, normalized, corrected, cells_width, cells_height);
    }
    if (!inside) {
        return;
    }

    // Свой белый точки L: у AgX — уровень обрезки после баланса и вход кривой; у прежней основы не читается.
    float white_level = lens_shading_white_level(shading_gains, tone_base_white_balance());
    vec3 camera_rgb = demosaic_cell(apply_white_balance(corrected, white_clip_level(white_level)));
    vec3 display_linear;
    if (TONE_BASE_MODEL == TONE_MODEL_AGX) {
        vec3 shaped = apply_agx_tone(apply_agx_input(apply_color_working(camera_rgb)), white_level);
        // Вид в W после основы; значения вне 0…1 принимает выходное сжатие охвата.
        display_linear = apply_agx_output(apply_look_lut(apply_look_color(shaped)));
    } else if (TONE_BASE_MODEL == TONE_MODEL_FILMIC) {
        vec3 shaped = apply_filmic_tone(apply_filmic_input(apply_color_working(camera_rgb)), white_level);
        display_linear = apply_filmic_output(apply_look_lut(apply_look_color(shaped)));
    } else if (TONE_BASE_MODEL == TONE_MODEL_ACES) {
        vec3 shaped = apply_aces_tone(apply_aces_input(apply_color_working(camera_rgb)), white_level);
        display_linear = apply_aces_output(apply_look_lut(apply_look_color(shaped)));
    } else {
        // Прежняя основа: W = цвета выхода; вид над светом после S-кривой, 0…1 обрежет OETF.
        display_linear = apply_look_lut(apply_look_color(apply_tone(compress_gamut(apply_color(camera_rgb)))));
    }
    imageStore(viewfinder_image, ivec2(upright), vec4(apply_oetf(display_linear), 1.0));
}
