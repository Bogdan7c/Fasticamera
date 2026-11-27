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
shared uint kept_primitive_index[128];

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

#if defined(SCOPE_RENDER_HISTOGRAM)
// ГИСТОГРАММА (сессия 14s): столбики вместо плотности «строка × столбец». Эталон — `scope_model::histogram_bars`
// (`HistogramBars`) и `scope_model::histogram_height` (кривые): тот же порядок операций f32. Строка сетки одна, первый
// проход (`scope_resample.comp`) уже дал плотность столбцов картинки `resampled.values[x * слотов + слот]`.

// Число нитей группы: литерал, а не `gl_WorkGroupSize` — размер shared-массива через константу специализации на Adreno
// собирался иначе (правила 16a). Совпадение с размером группы Rust сверяет тест `shader_literals_tests`.
const uint HISTOGRAM_GROUP_THREADS = 128u;
const float CURVE_LINEAR = 0.0;
const float CURVE_SQUARE_ROOT = 1.0;
shared float group_peak_density[128];

// Самый высокий столбик картинки: наибольшая плотность по всем столбцам и слотам. Каждая группа находит его сама по
// первым `ширина × слотов` числам слоя (их немного — ≈ 3000): отдельного прохода и сброса буфера не нужно. Вызывается
// ВСЕМИ нитями группы (барьеры), в том числе у пикселей за краем картинки. Опору «среднее» (x ≥ 0) не ищет.
float tallest_density_of_picture() {
    if (parameters.histogram.x >= 0.0) {
        return 0.0;
    }
    uint count = parameters.picture.x * parameters.picture.z;
    float local_peak = 0.0;
    for (uint index = gl_LocalInvocationIndex; index < count; index += HISTOGRAM_GROUP_THREADS) {
        local_peak = max(local_peak, resampled.values[index]);
    }
    group_peak_density[gl_LocalInvocationIndex] = local_peak;
    barrier();
    for (uint half_count = HISTOGRAM_GROUP_THREADS / 2u; half_count > 0u; half_count >>= 1u) {
        if (gl_LocalInvocationIndex < half_count) {
            group_peak_density[gl_LocalInvocationIndex] = max(
                group_peak_density[gl_LocalInvocationIndex], group_peak_density[gl_LocalInvocationIndex + half_count]);
        }
        barrier();
    }
    return group_peak_density[0u];
}

// Доля высоты столбика по счёту `count` и опоре `reference` (отсчётов на уровень) — `HistogramCurve::fraction`.
float bar_fraction(float count, float reference) {
    if (!(count > 0.0 && reference > 0.0)) {
        return 0.0;
    }
    float fraction;
    if (parameters.histogram.y == CURVE_LINEAR) {
        fraction = count / reference;
    } else if (parameters.histogram.y == CURVE_SQUARE_ROOT) {
        fraction = sqrt(count / reference);
    } else {
        fraction = log2(1.0 + count) / log2(1.0 + reference);
    }
    return min(fraction, 1.0);
}

// Доля высоты столбика столбца `x` в слоте `slot` — `HistogramBars::height_fraction`. Все плоскости лежат на всю ширину
// (полосы PARADE друг над другом), поэтому пикселей на уровень одно число — ядро «вся ширина».
float bar_fraction_at(uint x, uint slot, float tallest_density) {
    float per_bin = parameters.resample_axes[AXIS_FULL_WIDTH].x;
    float count = resampled.values[x * parameters.picture.z + slot] * per_bin;
    float reference = parameters.histogram.x >= 0.0 ? parameters.histogram.x : tallest_density * per_bin;
    return bar_fraction(count, reference);
}

// Покрытие заливкой: пиксель на расстоянии `distance_to_baseline` от базовой линии под верхушкой `top_px`; верх мягкий,
// шириной в один пиксель; пустой столбец не закрашивается — `HistogramBars::fill_coverage`.
float fill_coverage(float top_px, float distance_to_baseline) {
    if (top_px <= 0.0) {
        return 0.0;
    }
    return clamp(top_px - distance_to_baseline + 0.5, 0.0, 1.0);
}

// Расстояние от точки до отрезка — `histogram_paint::distance_to_segment` (тот же порядок операций).
float histogram_segment_distance(vec2 point, vec2 start, vec2 end) {
    vec2 along = end - start;
    vec2 relative = point - start;
    float length_squared = along.x * along.x + along.y * along.y;
    float fraction = 0.0;
    if (length_squared > 0.0) {
        fraction = clamp((relative.x * along.x + relative.y * along.y) / length_squared, 0.0, 1.0);
    }
    vec2 offset = vec2(relative.x - along.x * fraction, relative.y - along.y * fraction);
    return sqrt(offset.x * offset.x + offset.y * offset.y);
}

// Покрытие линией верха: расстояние до ломаной по верхушкам столбцов `x − 1`, `x`, `x + 1` (по двум соседним отрезкам;
// отрезок над пустыми столбцами не рисуется) — `histogram_paint::edge_coverage`. Толщина 0 — линии нет.
float histogram_edge_coverage(float distance_to_baseline, vec3 heights, float edge_width_px) {
    if (edge_width_px <= 0.0) {
        return 0.0;
    }
    vec2 point = vec2(0.0, distance_to_baseline);
    float nearest = 1.0e30;
    if (max(heights.x, heights.y) > 0.0) {
        nearest = min(nearest, histogram_segment_distance(point, vec2(-1.0, heights.x), vec2(0.0, heights.y)));
    }
    if (max(heights.y, heights.z) > 0.0) {
        nearest = min(nearest, histogram_segment_distance(point, vec2(0.0, heights.y), vec2(1.0, heights.z)));
    }
    return clamp(edge_width_px * 0.5 + 0.5 - nearest, 0.0, 1.0);
}

// Полоса пикселя с центром по вертикали `centre_y` — `HistogramStrips::locate` (тот же порядок операций f32). Ложь —
// пиксель в зазоре между полосами (или полосы не помещаются).
bool locate_histogram_strip(
    float centre_y, float height_px, uint count, out uint index, out float distance_to_baseline, out float strip_px) {
    float gap_px = parameters.histogram_style.y * height_px;
    strip_px = (height_px - float(count - 1u) * gap_px) / float(count);
    index = 0u;
    distance_to_baseline = 0.0;
    if (strip_px <= 0.0) {
        return false;
    }
    float pitch = strip_px + gap_px;
    index = min(uint(centre_y / pitch), count - 1u);
    float baseline = float(index) * pitch + strip_px;
    distance_to_baseline = baseline - centre_y;
    return distance_to_baseline >= 0.0;
}

// След пикселя гистограммы (решение 337) — `HistogramBars::pixel` и `HistogramBlend`: наложение (RGB) — все плоскости в
// одной полосе, полосы (PARADE, Y) — плоскость полосы. Заливки складываются как свет (прозрачность — по самому высокому
// покрытию), линии верха рисуются поверх своими цветами. Прозрачность следа `trace.y` гистограмма не берёт.
void histogram_trace(uvec2 pixel, uvec2 size, float tallest_density, out vec3 trace_color, out float trace_alpha) {
    trace_color = vec3(0.0);
    trace_alpha = 0.0;
    uint planes = parameters.grid.z;
    bool sectioned = parameters.trace.w == PAINT_HISTOGRAM_SECTIONS;
    uint strip_index;
    float distance_to_baseline;
    float strip_px;
    if (!locate_histogram_strip(float(pixel.y) + 0.5, float(size.y), sectioned ? planes : 1u, strip_index,
                                distance_to_baseline, strip_px)) {
        return;
    }
    uint first_slot = sectioned ? strip_index : 0u;
    uint end_slot = sectioned ? strip_index + 1u : planes;
    uint left = pixel.x > 0u ? pixel.x - 1u : pixel.x;
    uint right = min(pixel.x + 1u, size.x - 1u);
    vec3 fill_light = vec3(0.0);
    vec3 edge_light = vec3(0.0);
    float fill_max = 0.0;
    float edge_max = 0.0;
    for (uint slot = first_slot; slot < end_slot; ++slot) {
        vec3 heights = vec3(
            bar_fraction_at(left, slot, tallest_density) * strip_px,
            bar_fraction_at(pixel.x, slot, tallest_density) * strip_px,
            bar_fraction_at(right, slot, tallest_density) * strip_px);
        float fill = fill_coverage(heights.y, distance_to_baseline);
        float edge = histogram_edge_coverage(distance_to_baseline, heights, parameters.histogram.w);
        vec3 color = parameters.plane_colors[slot].rgb;
        fill_light += fill * color;
        edge_light += edge * color;
        fill_max = max(fill_max, fill);
        edge_max = max(edge_max, edge);
    }
    float fill_alpha = parameters.histogram.z * fill_max;
    float alpha = edge_max + fill_alpha * (1.0 - edge_max);
    if (alpha > 0.0) {
        vec3 edge_premultiplied = min(edge_light, vec3(1.0));
        vec3 fill_premultiplied = parameters.histogram.z * parameters.histogram_style.x * fill_light;
        trace_color = clamp((edge_premultiplied + fill_premultiplied * (1.0 - edge_max)) / alpha, 0.0, 1.0);
        trace_alpha = alpha;
    }
}
#endif

#if defined(SCOPE_RENDER_CIE)
// ДИАГРАММА CIE (сессия 14s): цвет следа по цветности точки, бледная заливка локуса и слоистая композиция. Эталон —
// `scope_model::cie_style` (`chromaticity_colour`, `locus_coverage`, `fill_at`) и `scope_model::render`: тот же порядок
// операций f32; `pow` и деление дают расхождение в пределах допуска картинки.
const float CIE_MIN_COLOUR_Y = 1.0e-4;
const float SRGB_LINEAR_LIMIT = 0.0031308;

// Кодировка sRGB (передача экрана).
float encode_srgb(float linear) {
    return linear <= SRGB_LINEAR_LIMIT ? 12.92 * linear : 1.055 * pow(linear, 1.0 / 2.4) - 0.055;
}

// Цвет цветности точки картинки `uv` (доли картинки, y вниз): XYZ с Y = 1 -> свет измеряемого охвата -> нормировка по
// наибольшему каналу -> кодировка экрана. Вне охвата (отрицательные каналы) каналы зажимаются в ноль.
vec3 cie_chromaticity_colour(vec2 uv) {
    vec2 chart = vec2(
        parameters.cie_chart.x + uv.x * parameters.cie_chart.z,
        parameters.cie_chart.y - uv.y * parameters.cie_chart.z);
    vec2 chromaticity = chart;
    if (parameters.cie_chart.w > 0.5) {
        float denominator = 6.0 * chart.x - 16.0 * chart.y + 12.0;
        if (!(denominator > 0.0)) {
            return vec3(1.0);
        }
        chromaticity = vec2(9.0 * chart.x / denominator, 4.0 * chart.y / denominator);
    }
    if (!(chromaticity.y > CIE_MIN_COLOUR_Y)) {
        return vec3(1.0);
    }
    vec3 xyz = vec3(
        chromaticity.x / chromaticity.y,
        1.0,
        (1.0 - chromaticity.x - chromaticity.y) / chromaticity.y);
    vec3 linear = max(vec3(
        dot(parameters.cie_xyz_to_display[0].xyz, xyz),
        dot(parameters.cie_xyz_to_display[1].xyz, xyz),
        dot(parameters.cie_xyz_to_display[2].xyz, xyz)), vec3(0.0));
    float peak = max(linear.r, max(linear.g, linear.b));
    if (!(peak > 0.0)) {
        return vec3(1.0);
    }
    vec3 normalised = linear / peak;
    return vec3(encode_srgb(normalised.r), encode_srgb(normalised.g), encode_srgb(normalised.b));
}

// Покрытие пикселя внутренностью локуса: непрерывное, по знаковому расстоянию до ближайшего ребра (внутри > 0).
float cie_locus_coverage(vec2 point, vec2 picture_px) {
    float nearest = 3.0e38;
    uint edge_count = uint(parameters.cie_options.w);
    for (uint index = 0u; index < edge_count; ++index) {
        vec4 edge = parameters.cie_locus_edges[index];
        vec2 from = edge.xy * picture_px;
        vec2 along = edge.zw * picture_px - from;
        float length_px = length(along);
        if (!(length_px > 0.0)) {
            continue;
        }
        float cross_product = along.x * (point.y - from.y) - along.y * (point.x - from.x);
        nearest = min(nearest, cross_product / length_px);
    }
    return nearest < 1.0e38 ? clamp(0.5 + nearest, 0.0, 1.0) : 0.0;
}

// Заливка пикселя: rgb и ПРЯМАЯ альфа (0 — заливки нет).
vec4 cie_fill(vec2 point, vec2 picture_px) {
    if (parameters.cie_options.y < 0.5) {
        return vec4(0.0);
    }
    float coverage = cie_locus_coverage(point, picture_px);
    if (coverage <= 0.0) {
        return vec4(0.0);
    }
    return vec4(cie_chromaticity_colour(point / picture_px), coverage * parameters.cie_options.z);
}

// Верхний слой над нижним, прямая альфа: `scope_model::render::composite_over`.
vec4 composite_over_layers(vec3 top, float top_alpha, vec3 below, float below_alpha) {
    float alpha = top_alpha + below_alpha * (1.0 - top_alpha);
    if (alpha <= 0.0) {
        return vec4(0.0);
    }
    return vec4((top * top_alpha + below * below_alpha * (1.0 - top_alpha)) / alpha, alpha);
}
#endif

void main() {
    uvec2 pixel = gl_GlobalInvocationID.xy;
    uvec2 size = uvec2(imageSize(picture));
    vec2 picture_px = vec2(size);
#if !defined(SCOPE_RENDER_WITHOUT_TILE_CULLING)
    // Отбор примитивов — на ВСЕХ нитях группы (барьеры), в том числе у пикселей за краем картинки.
    select_primitives_touching_the_tile(picture_px);
#endif
#if defined(SCOPE_RENDER_HISTOGRAM)
    // Опора «самый высокий столбик» — тоже на ВСЕХ нитях группы (барьеры).
    float tallest_density = tallest_density_of_picture();
#endif
    if (pixel.x >= size.x || pixel.y >= size.y) {
        return;
    }
    uint columns = parameters.grid.x;
    uint rows = parameters.grid.y;
    uint planes = parameters.grid.z;
#if defined(SCOPE_RENDER_HISTOGRAM)
    float trace_alpha = 0.0;
    vec3 trace_color = vec3(0.0);
    histogram_trace(pixel, size, tallest_density, trace_color, trace_alpha);
#else
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
#if defined(SCOPE_RENDER_CIE)
            } else if (parameters.trace.w == PAINT_CIE) {
                // Цвет следа: белый или цветность центра пикселя (доли картинки, y вниз).
                trace_color = parameters.cie_options.x > 0.5
                    ? cie_chromaticity_colour((vec2(pixel) + vec2(0.5)) / picture_px)
                    : vec3(1.0);
#endif
            } else {
                trace_color = parameters.plane_colors[section_of_pixel(pixel.x, size.x, planes).x].rgb;
            }
        }
    }
#endif

    vec2 point = vec2(pixel) + vec2(0.5);
    float grid_alpha = graticule_coverage(point, picture_px) * parameters.trace.z;

#if defined(SCOPE_RENDER_CIE)
    // Слои диаграммы CIE снизу вверх: заливка локуса, сетка, след.
    vec4 fill = cie_fill(point, picture_px);
    vec4 under = composite_over_layers(parameters.graticule_color.rgb, grid_alpha, fill.rgb, fill.a);
    vec4 result = composite_over_layers(trace_color, trace_alpha, under.rgb, under.a);
#else
    // Сетка рисуется ПОД следом: прямая альфа «over».
    float alpha = trace_alpha + grid_alpha * (1.0 - trace_alpha);
    vec4 result = vec4(0.0);
    if (alpha > 0.0) {
        vec3 grid_color = parameters.graticule_color.rgb;
        vec3 color = (trace_color * trace_alpha + grid_color * grid_alpha * (1.0 - trace_alpha)) / alpha;
        result = vec4(color, alpha);
    }
#endif
    imageStore(picture, ivec2(pixel), result);
}
