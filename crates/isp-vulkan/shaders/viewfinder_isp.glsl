// ISP видоискателя (сессия 07): ОДИН слитый compute-проход RAW → изображение видоискателя.
//
// Формулы — эталон `isp-model` (вариант Preview), стадия за стадией, в том же порядке и в f32:
//   уровни → коррекция виньетирования (lens_shading.glsl) → баланс белого + обрезка → демозаик 2×2 → камера→XYZ D50 → XYZ→экран (два отдельных
//   произведения, как на CPU) → сжатие охвата → тон с сохранением оттенка → OETF sRGB
// плюс статистика AWB (суммы по плиткам, контракт `isp_model::statistics`) в том же проходе. Статистика
// считается ПОСЛЕ коррекции виньетирования и до баланса белого (контракт isp-model).
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

// --- Параметры кадра (std140, только 32-битные типы: у Adreno 740 нет 16-битного доступа к uniform) ---
layout(set = 0, binding = 1, std140) uniform ViewfinderParameters {
    vec4 black_level;              // чёрный уровень по позициям ячейки 2×2 (порядок строк)
    vec4 level_range;              // white − black по позициям (посчитано на CPU в f32, как в эталоне)
    vec4 white_balance_gain;       // усиление баланса для цвета каждой позиции
    vec4 levels_misc;              // x — белый уровень, y — уровень обрезки после баланса, z — порог пересвета AWB
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

// Окно плиток в shared-памяти рабочей группы: группа (по умолчанию 16×8 нитей) при плитках от ~64 ячеек
// задевает 1–4 плитки; окно 4×4 покрывает их с запасом. Плитки вне окна (мелкая сетка на маленьком кадре)
// идут сразу в глобальный atomicAdd — медленнее, но всегда верно.
const uint AWB_LOCAL_TILE_SPAN = 4u;
const uint AWB_LOCAL_WORDS = AWB_LOCAL_TILE_SPAN * AWB_LOCAL_TILE_SPAN * AWB_WORDS_PER_TILE;
const uint NO_TILE = 0xFFFFFFFFu;

shared uint awb_window_origin_x;
shared uint awb_window_origin_y;
shared uint awb_window_sums[AWB_LOCAL_WORDS];

// sRGB OETF (IEC 61966-2-1), как isp_model::transfer::srgb_encode.
const float OETF_LINEAR_LIMIT = 0.0031308;
const float OETF_LINEAR_SLOPE = 12.92;
const float OETF_POWER_SCALE = 1.055;
const float OETF_POWER_OFFSET = 0.055;
const float OETF_INVERSE_EXPONENT = 1.0 / 2.4;

// Значение RAW пикселя (x, y) в координатах буфера; тело — в `viewfinder_source_*.glsl`.
uint read_raw(uint x, uint y);

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
vec4 apply_lens_shading(vec4 normalized, uvec2 cell_origin, uint sample_count) {
    if (!STAGE_LENS_SHADING || !lens_shading_enabled()) {
        return normalized;
    }
    uvec2 valid_origin = cell_origin - parameters.lens_shading_origin.xy;
    vec4 gains = lens_shading_gains(lens_shading_axis_cell_centre_x(valid_origin.x),
                                    lens_shading_axis_cell_centre_y(valid_origin.y));
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

// q = floor(clamp(v, 0, 1) · 4095 + 0,5): умножение и сложение — ОТДЕЛЬНЫЕ операции с округлением
// (`precise` = NoContraction, без слияния в FMA), иначе значения у границы .5 округлялись бы иначе.
uint quantize_awb_sample(float normalized) {
    precise float scaled = clamp(normalized, 0.0, 1.0) * AWB_QUANT_SCALE;
    precise float rounded = floor(scaled + 0.5);
    return uint(rounded);
}

// Пересвет: !(v < порог) — NaN тоже исключает ячейку, как в эталоне.
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
// saturated_cells. Ячейка с пересветом хоть в одной позиции идёт только в saturated (isp_model). Нить вне
// выхода (`inside` = false) даёт нули: так её можно складывать вместе со всеми без ветвления.
void awb_cell_contribution(bool inside, vec4 normalized, out uvec4 sums_and_used, out uint saturated) {
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
        sums_and_used[parameters.channel_of_position[position]] += quantize_awb_sample(normalized[position]);
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
void accumulate_awb_single_tile(bool inside, uvec2 tile, vec4 normalized) {
    uvec4 sums_and_used;
    uint saturated;
    awb_cell_contribution(inside, normalized, sums_and_used, saturated);
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
void accumulate_awb_window(bool inside, uvec2 cell, vec4 normalized, uint cells_width, uint cells_height) {
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
        awb_cell_contribution(inside, normalized, sums_and_used, saturated);
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

// Статистика AWB ячейки нити. Вызывают ВСЕ нити группы, включая нити вне выхода (они дают нули).
// С арифметикой subgroup группа целиком в одной плитке (типично: плитки по 64×64 ячейки) идёт быстрым
// путём; группа на стыке плиток (или вариант без subgroup) — через shared-окно.
void accumulate_awb_statistics(bool inside, uvec2 cell, vec4 normalized, uint cells_width, uint cells_height) {
#ifdef AWB_SUBGROUP_REDUCTION
    uvec2 single_tile = workgroup_single_tile(cells_width, cells_height);
    if (single_tile.x != NO_TILE) {
        accumulate_awb_single_tile(inside, single_tile, normalized);
        return;
    }
#endif
    accumulate_awb_window(inside, cell, normalized, cells_width, cells_height);
}

// ===== Стадии 3–4: баланс белого и демозаик =======================================================

// min(v · gain, clip) — isp_model::white_balance. Выключено: значение как есть (ни усиления, ни обрезки).
vec4 apply_white_balance(vec4 normalized) {
    if (!STAGE_WHITE_BALANCE) {
        return normalized;
    }
    return min(normalized * parameters.white_balance_gain, vec4(parameters.levels_misc.y));
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

// ===== Стадия 6а: сжатие охвата (isp_model::gamut::GamutCompression::compress_pixel) ==============

float compress_distance(uint channel, float distance) {
    float threshold = parameters.gamut_threshold_power[channel];
    if (distance < threshold) {
        return distance;
    }
    float scale = parameters.gamut_scale_inverse_power[channel];
    float power = parameters.gamut_threshold_power.w;
    float inverse_power = parameters.gamut_scale_inverse_power.w;
    float normalized = (distance - threshold) / scale;
    return threshold + scale * normalized / pow(1.0 + pow(normalized, power), inverse_power);
}

// Выключено: свет как есть (отрицательные каналы потом обнулит тон или OETF).
vec3 compress_gamut(vec3 scene) {
    if (!STAGE_GAMUT_COMPRESSION) {
        return scene;
    }
    if (any(isnan(scene))) {
        return vec3(0.0);
    }
    float achromatic = max(max(scene.r, scene.g), scene.b);
    if (achromatic <= 0.0) {
        return vec3(0.0);
    }
    vec3 compressed;
    for (uint channel = 0u; channel < 3u; ++channel) {
        float distance = (achromatic - scene[channel]) / achromatic;
        float compressed_distance = compress_distance(channel, distance);
        compressed[channel] = max(achromatic - compressed_distance * achromatic, 0.0);
    }
    return compressed;
}

// ===== Стадия 6б: тон (isp_model::tone::ToneCurve::map_pixel) =====================================

float tone_channel(float scene) {
    if (isnan(scene) || scene <= 0.0) {
        return 0.0;
    }
    float powered = pow(scene, parameters.tone.x);
    return min(powered / (powered + parameters.tone.y) * parameters.tone.z, 1.0);
}

// Выключено: свет сцены идёт на экран как есть (без S-кривой и без сохранения оттенка).
vec3 apply_tone(vec3 scene) {
    if (!STAGE_TONE) {
        return scene;
    }
    if (any(isnan(scene))) {
        return vec3(0.0);
    }
    vec3 display = vec3(tone_channel(scene.r), tone_channel(scene.g), tone_channel(scene.b));
    // Порядок каналов по убыванию, при равенстве — R, G, B (устойчивая сортировка эталона):
    // максимум — первый из наибольших, минимум — последний из наименьших.
    uint max_channel = 0u;
    if (scene.g > scene[max_channel]) { max_channel = 1u; }
    if (scene.b > scene[max_channel]) { max_channel = 2u; }
    uint min_channel = 2u;
    if (scene.g < scene[min_channel]) { min_channel = 1u; }
    if (scene.r < scene[min_channel]) { min_channel = 0u; }
    uint middle_channel = 3u - max_channel - min_channel;
    float input_span = scene[max_channel] - scene[min_channel];
    float hue_preservation = parameters.tone.w;
    if (input_span <= 0.0 || hue_preservation <= 0.0) {
        return display;
    }
    float middle_share = (scene[middle_channel] - scene[min_channel]) / input_span;
    float restored_middle = display[min_channel] + middle_share * (display[max_channel] - display[min_channel]);
    display[middle_channel] += hue_preservation * (restored_middle - display[middle_channel]);
    return display;
}

// ===== Стадия 7: OETF =============================================================================

float srgb_encode(float linear_light) {
    if (isnan(linear_light)) {
        return 0.0;
    }
    float clamped = clamp(linear_light, 0.0, 1.0);
    if (clamped <= OETF_LINEAR_LIMIT) {
        return OETF_LINEAR_SLOPE * clamped;
    }
    return OETF_POWER_SCALE * pow(clamped, OETF_INVERSE_EXPONENT) - OETF_POWER_OFFSET;
}

// Выключено: линейный свет пишется как есть (UNORM-картинка обрежет его до 0…1 сама).
vec3 apply_oetf(vec3 display_linear) {
    if (!STAGE_OETF) {
        return display_linear;
    }
    return vec3(srgb_encode(display_linear.r), srgb_encode(display_linear.g), srgb_encode(display_linear.b));
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
    vec4 normalized = vec4(0.0);
    if (inside) {
        cell = cell_of_output(upright, parameters.output_geometry.z, cells_width, cells_height);
        uvec2 origin = parameters.crop.xy + cell * 2u;
        // Без демозаика и без статистики нужна только позиция 0 — читаем один отсчёт вместо четырёх.
        bool needs_whole_cell = STAGE_DEMOSAIC || STAGE_AWB_STATISTICS;
        normalized[0] = normalize_sample(read_raw(origin.x, origin.y), 0u);
        if (needs_whole_cell) {
            normalized[1] = normalize_sample(read_raw(origin.x + 1u, origin.y), 1u);
            normalized[2] = normalize_sample(read_raw(origin.x, origin.y + 1u), 2u);
            normalized[3] = normalize_sample(read_raw(origin.x + 1u, origin.y + 1u), 3u);
        }
        // Коррекция виньетирования — до статистики AWB и до баланса белого (порядок эталона).
        normalized = apply_lens_shading(normalized, origin, needs_whole_cell ? 4u : 1u);
    }

    if (STAGE_AWB_STATISTICS) {
        accumulate_awb_statistics(inside, cell, normalized, cells_width, cells_height);
    }
    if (!inside) {
        return;
    }

    vec3 camera_rgb = demosaic_cell(apply_white_balance(normalized));
    vec3 display_linear = apply_tone(compress_gamut(apply_color(camera_rgb)));
    imageStore(viewfinder_image, ivec2(upright), vec4(apply_oetf(display_linear), 1.0));
}
