// Второй проход показа цветовых приборов: промежуточный слой первого прохода (`scope_resample.comp`) → картинка
// прибора RGBA16F, ПРЯМАЯ альфа, цвета экрана. Подключается из `scope_render.comp` (продукт, с отбором примитивов по
// плитке) и `scope_render_reference.comp` (тот же шейдер без отбора — эталон точного равенства отбора).
// CPU-эталон: `scope_model::render` (`graticule_alpha`, `composite_over`), `scope_model::trace_density` (перенос по
// вертикали), `scope_model::trace_painter` (пиксельные формулы трёх способов следа), `scope_model::TraceReference`
// (яркость по попаданиям на пиксель) и `scope_model::graticule` (покрытие примитивов). Три способа следа (`PAINT_*`):
// секции по плоскостям (RGB parade), плоскости наложением (RGB waveform), цвет по положению (вектороскоп). Способ
// называет правило прибора: в шейдер он приходит словом `trace.w`. Сумма весов, `exp`, `log2`, `sqrt` и деление
// дают расхождение с эталоном в пределах допуска картинки (см. тесты).
// Размер группы — specialization-константы 0 и 1 (см. `scope_accumulate_common.glsl`).
layout(local_size_x_id = 0, local_size_y_id = 1) in;

layout(set = 0, binding = 0, rgba16f) uniform writeonly image2D picture;

#include "scope_render_parameters.glsl"

layout(set = 0, binding = 2, std430) readonly buffer ResampledColumns {
    float values[];
} resampled;

const float KIND_SEGMENT = 0.0;
const float KIND_RING = 1.0;

const uint POSITION_PALETTE_ANGLES = 128u;
const float TAU = 6.28318530717958647692;

// Яркость следа по числу попаданий на пиксель: лог-шкала `min(1, log2(1 + hits) / trace.x)` — эталон
// `TraceReference::intensity` (тот же порядок операций f32: `1 + …`, `log2`, деление, `min`).
float trace_intensity(float hits) {
    return hits <= 0.0 ? 0.0 : min(log2(1.0 + hits) / parameters.trace.x, 1.0);
}

// Попадания на пиксель по слотам (до трёх): суммы строк промежуточного слоя с весами вертикального ядра —
// эталон `PictureDensity::compute`. Вес строки один для всех слотов и считается один раз; у каждого слота тот же
// порядок сложения, что у эталона (строка за строкой).
vec3 pixel_hits(uvec2 pixel, uvec2 rows_range, vec4 vertical) {
    uint width = parameters.picture.x;
    uint slots = parameters.picture.z;
    // Шаг между строками слоя и начало пикселя — один раз; два явных пути вместо цикла по слотам (14r, разбор цены).
    uint row_stride = width * slots;
    uint first = (rows_range.x * width + pixel.x) * slots;
    if (slots == 3u) {
        float sum_0 = 0.0;
        float sum_1 = 0.0;
        float sum_2 = 0.0;
        for (uint row = rows_range.x; row < rows_range.y; ++row) {
            float weight = axis_weight(vertical, row, pixel.y);
            sum_0 += resampled.values[first] * weight;
            sum_1 += resampled.values[first + 1u] * weight;
            sum_2 += resampled.values[first + 2u] * weight;
            first += row_stride;
        }
        return vec3(sum_0, sum_1, sum_2);
    }
    float sum = 0.0;
    for (uint row = rows_range.x; row < rows_range.y; ++row) {
        sum += resampled.values[first] * axis_weight(vertical, row, pixel.y);
        first += row_stride;
    }
    return vec3(sum, 0.0, 0.0);
}

// Цвет точки `point` сетки вектороскопа (координаты в клетках от левого верхнего угла) по её положению: эталон —
// `PositionPalette::colour_at_point`. Центр даёт d = (0, 0), а atan(0, 0) в GLSL не определён (на Adreno может быть
// NaN), поэтому ветка «центр -> нейтраль» стоит ДО вычисления угла.
vec3 position_color(vec2 point) {
    float centre = parameters.position_geometry.x;
    vec2 offset = vec2(point.x - centre, centre - point.y);
    float radius = length(offset);
    vec3 neutral = parameters.position_neutral.rgb;
    if (!(radius > parameters.position_geometry.y)) {
        return neutral;
    }
    float turns = atan(offset.y, offset.x) / TAU;
    if (turns < 0.0) {
        turns += 1.0;
    }
    float position = turns * float(POSITION_PALETTE_ANGLES);
    float base = floor(position);
    float fraction = position - base;
    uint first = uint(base) % POSITION_PALETTE_ANGLES;
    uint second = (first + 1u) % POSITION_PALETTE_ANGLES;
    vec4 low = parameters.position_palette[first];
    vec4 high = parameters.position_palette[second];
    float edge_radius = max(mix(low.w, high.w, fraction), parameters.position_geometry.z);
    float reach = clamp(radius / edge_radius, 0.0, 1.0);
    return mix(neutral, mix(low.rgb, high.rgb, fraction), reach);
}

float distance_to_segment(vec2 point, vec2 start, vec2 end) {
    vec2 along = end - start;
    vec2 relative = point - start;
    float length_squared = dot(along, along);
    float fraction = length_squared > 0.0 ? clamp(dot(relative, along) / length_squared, 0.0, 1.0) : 0.0;
    return length(relative - along * fraction);
}

float distance_to_box_outline(vec2 point, vec2 centre, vec2 half_size) {
    vec2 excess = abs(point - centre) - half_size;
    float outside = length(max(excess, vec2(0.0)));
    float inside = min(max(excess.x, excess.y), 0.0);
    return abs(outside + inside);
}

// Расстояние от точки до примитива, пикселей (разность с полуполосой линии даёт покрытие).
float primitive_distance(GraticulePrimitive primitive, vec2 point, vec2 picture_px) {
    vec2 first = primitive.geometry.xy * picture_px;
    vec2 second = primitive.geometry.zw * picture_px;
    float distance_px;
    if (primitive.style.y == KIND_SEGMENT) {
        distance_px = distance_to_segment(point, first, second);
    } else if (primitive.style.y == KIND_RING) {
        float normalized = length((point - first) / second);
        distance_px = abs(normalized - 1.0) * min(second.x, second.y);
    } else {
        distance_px = distance_to_box_outline(point, first, second);
    }
    return distance_px;
}

// Покрытие пикселя примитивом: clamp(полуширина + 0.5 - расстояние, 0, 1).
float primitive_coverage(GraticulePrimitive primitive, vec2 point, vec2 picture_px) {
    float half_width = primitive.style.x;
    float distance_px = primitive_distance(primitive, point, picture_px);
    return clamp(half_width + 0.5 - distance_px, 0.0, 1.0);
}

#if !defined(SCOPE_RENDER_WITHOUT_TILE_CULLING)
// ОТБОР ПРИМИТИВОВ ПО ПЛИТКЕ (точный: картинка бит в бит та же, что без отбора). Каждая нить группы проверяет свои
// примитивы против всей плитки группы; в список попадает примитив, который может дать ненулевое покрытие хоть одному
// пикселю плитки. Расстояние до любого примитива 1-липшицево по точке (отрезок и рамка — настоящее расстояние;
// кольцо — |n - 1| * min(радиусы), где n меняется не быстрее 1 / min(радиусы)), поэтому если расстояние от
// ЦЕНТРА плитки минус радиус плитки не меньше полуширины + 0.5 (+ запас на ошибку f32), покрытие каждого пикселя
// плитки равно нулю и примитив не влияет на `max`. NaN в расстоянии даёт «оставить» (сравнение ложно).
const float CULL_SLACK_PX = 0.01;
shared uint kept_count;
shared uint kept_primitive_index[48];

void select_primitives_touching_the_tile(vec2 picture_px) {
    uint group_threads = gl_WorkGroupSize.x * gl_WorkGroupSize.y;
    if (gl_LocalInvocationIndex == 0u) {
        kept_count = 0u;
    }
    barrier();
    // Центры пикселей плитки заполняют прямоугольник от (начало + 0.5) до (конец - 0.5): его центр и полудиагональ.
    vec2 tile_size = vec2(gl_WorkGroupSize.xy);
    vec2 tile_centre = vec2(gl_WorkGroupID.xy * gl_WorkGroupSize.xy) + 0.5 * tile_size;
    float tile_radius = 0.5 * length(tile_size - vec2(1.0));
    for (uint index = gl_LocalInvocationIndex; index < parameters.grid.w; index += group_threads) {
        GraticulePrimitive primitive = parameters.primitives[index];
        float centre_distance = primitive_distance(primitive, tile_centre, picture_px);
        bool cannot_touch_tile = centre_distance - tile_radius - CULL_SLACK_PX >= primitive.style.x + 0.5;
        if (!cannot_touch_tile) {
            uint slot = atomicAdd(kept_count, 1u);
            kept_primitive_index[slot] = index;
        }
    }
    barrier();
}
#endif

// Наибольшее покрытие пикселя примитивами сетки (с отбором — только по примитивам своей плитки).
float graticule_coverage(vec2 point, vec2 picture_px) {
    float coverage = 0.0;
#if defined(SCOPE_RENDER_WITHOUT_TILE_CULLING)
    for (uint index = 0u; index < parameters.grid.w; ++index) {
        coverage = max(coverage, primitive_coverage(parameters.primitives[index], point, picture_px));
    }
#else
    for (uint kept = 0u; kept < kept_count; ++kept) {
        coverage = max(coverage, primitive_coverage(parameters.primitives[kept_primitive_index[kept]], point, picture_px));
    }
#endif
    return coverage;
}

void main() {
    uvec2 pixel = gl_GlobalInvocationID.xy;
    uvec2 size = uvec2(imageSize(picture));
    vec2 picture_px = vec2(size);
#if !defined(SCOPE_RENDER_WITHOUT_TILE_CULLING)
    // Отбор примитивов — на ВСЕХ нитях группы (барьеры), в том числе у пикселей за краем картинки.
    select_primitives_touching_the_tile(picture_px);
#endif
    if (pixel.x >= size.x || pixel.y >= size.y) {
        return;
    }
    uint columns = parameters.grid.x;
    uint rows = parameters.grid.y;
    uint planes = parameters.grid.z;
    vec4 vertical = parameters.resample_axes[AXIS_VERTICAL];
    uvec2 rows_range = axis_cell_range(vertical, pixel.y, rows);

    vec3 hits = pixel_hits(pixel, rows_range, vertical);
    float trace_alpha = 0.0;
    vec3 trace_color = vec3(0.0);
    if (parameters.trace.w == PAINT_PLANES_OVERLAID) {
        // Все плоскости рисуются в одни пиксели на всю ширину; цвет = смесь цветов плоскостей по их яркостям,
        // делённая на свой наибольший канал; прозрачность — по самой яркой плоскости.
        vec3 weighted = vec3(0.0);
        float brightest = 0.0;
        for (uint plane = 0u; plane < planes; ++plane) {
            float intensity = trace_intensity(hits[plane]);
            weighted += intensity * parameters.plane_colors[plane].rgb;
            brightest = max(brightest, intensity);
        }
        float peak = max(weighted.r, max(weighted.g, weighted.b));
        if (brightest > 0.0 && peak > 0.0) {
            trace_color = weighted / peak;
            trace_alpha = brightest * parameters.trace.y;
        }
    } else {
        if (hits.x > 0.0) {
            trace_alpha = trace_intensity(hits.x) * parameters.trace.y;
            if (parameters.trace.w == PAINT_COLOR_BY_POSITION) {
                // Центр пикселя в координатах клеток сетки (центр клетки i — в i + 0,5), как у эталона.
                vec2 point = vec2(
                    (float(pixel.x) + 0.5) * float(columns) / float(size.x),
                    (float(pixel.y) + 0.5) * float(rows) / float(size.y));
                trace_color = position_color(point);
            } else {
                trace_color = parameters.plane_colors[section_of_pixel(pixel.x, size.x, planes).x].rgb;
            }
        }
    }

    vec2 point = vec2(pixel) + vec2(0.5);
    float grid_alpha = graticule_coverage(point, picture_px) * parameters.trace.z;

    // Сетка рисуется ПОД следом: прямая альфа «over».
    float alpha = trace_alpha + grid_alpha * (1.0 - trace_alpha);
    vec4 result = vec4(0.0);
    if (alpha > 0.0) {
        vec3 grid_color = parameters.graticule_color.rgb;
        vec3 color = (trace_color * trace_alpha + grid_color * grid_alpha * (1.0 - trace_alpha)) / alpha;
        result = vec4(color, alpha);
    }
    imageStore(picture, ivec2(pixel), result);
}
