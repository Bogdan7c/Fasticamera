// Коррекция виньетирования на GPU: усиление по нашей калибровке камеры (стадия 2 эталона isp-model).
//
// ОДНА реализация для всех шейдеров (видоискатель, качественный конвейер, проявление снимка по полосам).
// Дословное зеркало `isp_model::lens_shading` (модуль и `bilinear_gain`/`grid_coordinate`): для отсчёта в
// пикселе (x, y) ПОЛЕЗНОЙ ОБЛАСТИ размера Wv × Hv, f32, тот же порядок операций:
//   scale_x = (columns − 1) / (Wv − 1)           scale_y = (rows − 1) / (Hv − 1)   — f32, считает CPU ОДИН раз на кадр
//   grid_x = x · scale_x                         grid_y = y · scale_y
//   (последний пиксель x = Wv − 1: grid_x = columns − 1 точно — явный случай, см. lens_shading_axis)
// Это выборка «по отсчёту» (фото, качественный конвейер). Видоискатель берёт усиление ОДИН раз на ячейку 2×2
// в её центре: grid_x = (x0 + 0,5) · scale_x, grid_y = (y0 + 0,5) · scale_y, где (x0, y0) — чётный верхний
// левый отсчёт ячейки (см. lens_shading_axis_cell_centre); канал карты — свой у каждого из четырёх отсчётов.
//   left = min(floor(grid_x), columns − 2)       top = min(floor(grid_y), rows − 2)
//   fx = grid_x − left                           fy = grid_y − top
//   upper = p(left, top)·(1 − fx) + p(left + 1, top)·fx
//   lower = p(left, top + 1)·(1 − fx) + p(left + 1, top + 1)·fx
//   gain  = upper·(1 − fy) + lower·fy
// Скорректированный отсчёт = нормированный отсчёт · gain (умножает вызывающий).
//
// Бит-в-бит с CPU держат три вещи:
//   * `precise` (NoContraction, Vulkan: SPIR-V decoration NoContraction) на каждой арифметической
//     операции — компилятор не сливает a·b + c в FMA и не переставляет сложения;
//   * координата узла — ОДНО умножение `x · scale` с готовым масштабом (деления в шейдере нет): Vulkan
//     (раздел «Precision and Operation of SPIR-V Instructions») требует от OpFMul/OpFAdd/OpFSub результата
//     с правильным округлением, как у IEEE 754 на CPU; масштаб — те же f32, что у CPU, из заголовка карты;
//   * билинейная смесь — те же шесть умножений и три сложения в том же порядке, что на CPU.
//
// Карта — storage-буфер (БЕЗ сэмплера: аппаратная билинейная фильтрация считает веса в фиксированной
// точке с 8 битами дробной части и не даёт паритета с CPU). Раскладка (std430), зеркало
// `src/lens_shading_map.rs`:
//   смещение 0   uvec4 control    x — коррекция включена (0/1), y — столбцов, z — строк, w — 0
//   смещение 16  uvec4 valid_area x, y — ширина и высота полезной области (пиксели), z, w — 0
//   смещение 32  vec4 scales      x, y — масштабы «пиксель → узел» по осям, z, w — 0
//   смещение 48  vec4 points[]    точки карты по строкам, в каждой [R, G_even, G_odd, B]
//
// Включающий файл ДО #include определяет `LENS_SHADING_MAP_BINDING` — номер привязки буфера карты
// (set 0). Файл защищён от повторного включения.
#ifndef LENS_SHADING_GLSL
#define LENS_SHADING_GLSL

layout(set = 0, binding = LENS_SHADING_MAP_BINDING, std430) readonly buffer LensShadingMap {
    uvec4 control;
    uvec4 valid_area;
    vec4 scales;
    vec4 points[];
} lens_shading_map;

// Каналы точки карты (порядок Camera2 LENS_SHADING_MAP: R, G_even, G_odd, B).
const uint LENS_SHADING_CHANNEL_RED = 0u;
const uint LENS_SHADING_CHANNEL_GREEN_EVEN = 1u;
const uint LENS_SHADING_CHANNEL_GREEN_ODD = 2u;
const uint LENS_SHADING_CHANNEL_BLUE = 3u;

// Включена ли коррекция в этом кадре (в карте лежит ли настоящая карта, а не заглушка «пропуск»).
// Однородно для всего dispatch (значение из буфера, одинаковое у всех нитей).
bool lens_shading_enabled() {
    return lens_shading_map.control.x != 0u;
}

// Положение отсчёта вдоль ОДНОЙ оси сетки: левый узел и дробная часть (`left`/`fx` формулы выше).
struct LensShadingAxis {
    uint low_node;
    float fraction;
};

// Ось для пикселя `pixel` полезной области протяжённости `extent` (≥ 2) на сетке из `nodes` узлов (≥ 2) с
// готовым масштабом `scale`. Последний пиксель — ровно последний узел (иначе `(extent − 1)·scale` мог бы
// выйти на 1 ULP за узел или не дойти до него); остальные — одно умножение.
LensShadingAxis lens_shading_axis(uint pixel, uint extent, float scale, uint nodes) {
    precise float scaled = float(pixel) * scale;
    precise float grid = (pixel + 1u >= extent) ? float(nodes - 1u) : scaled;
    uint low_node = min(uint(floor(grid)), nodes - 2u);
    precise float fraction = grid - float(low_node);
    return LensShadingAxis(low_node, fraction);
}

LensShadingAxis lens_shading_axis_x(uint valid_x) {
    return lens_shading_axis(valid_x, lens_shading_map.valid_area.x, lens_shading_map.scales.x,
                             lens_shading_map.control.y);
}

LensShadingAxis lens_shading_axis_y(uint valid_y) {
    return lens_shading_axis(valid_y, lens_shading_map.valid_area.y, lens_shading_map.scales.y,
                             lens_shading_map.control.z);
}

// Ось для ячейки 2×2 с ЧЁТНЫМ верхним левым отсчётом `cell_origin` (координата полезной области): центр ячейки
// `(cell_origin + 0,5) · scale`. Сумма точна в f32; центр не доходит до последнего узла, но ограничение
// `min(floor, nodes − 2)` остаётся (то же, что в isp_model::lens_shading::cell_centre_coordinate).
LensShadingAxis lens_shading_axis_cell_centre(uint cell_origin, float scale, uint nodes) {
    precise float centre = float(cell_origin) + 0.5;
    precise float grid = centre * scale;
    uint low_node = min(uint(floor(grid)), nodes - 2u);
    precise float fraction = grid - float(low_node);
    return LensShadingAxis(low_node, fraction);
}

LensShadingAxis lens_shading_axis_cell_centre_x(uint cell_origin_x) {
    return lens_shading_axis_cell_centre(cell_origin_x, lens_shading_map.scales.x, lens_shading_map.control.y);
}

LensShadingAxis lens_shading_axis_cell_centre_y(uint cell_origin_y) {
    return lens_shading_axis_cell_centre(cell_origin_y, lens_shading_map.scales.y, lens_shading_map.control.z);
}

// Канал карты для цвета фильтра в пикселе строки `valid_y` полезной области: R → 0, B → 3, зелёный → G_even
// в чётной строке и G_odd в нечётной. `color`: 0 — красный, 1 — зелёный, 2 — синий (как CHANNEL_* шейдеров).
// Строка считается от верха ПОЛЕЗНОЙ области (её верх на сенсоре чётный — проверяет isp-model).
uint lens_shading_map_channel(uint color, uint valid_y) {
    if (color == 0u) {
        return LENS_SHADING_CHANNEL_RED;
    }
    if (color == 2u) {
        return LENS_SHADING_CHANNEL_BLUE;
    }
    return (valid_y & 1u) == 0u ? LENS_SHADING_CHANNEL_GREEN_EVEN : LENS_SHADING_CHANNEL_GREEN_ODD;
}

// Точка карты (все четыре канала одним vec4): столбец `column`, строка `row`.
vec4 lens_shading_node(uint column, uint row) {
    return lens_shading_map.points[row * lens_shading_map.control.y + column];
}

// Билинейная смесь четырёх точек карты СРАЗУ по всем каналам (`isp_model::lens_shading::bilinear_gain`): шесть
// умножений и три сложения на канал в порядке эталона. Вызывающему нужен один-два канала — остальные
// компилятор отбрасывает как неиспользуемые.
vec4 lens_shading_blend(vec4 top_left, vec4 top_right, vec4 bottom_left, vec4 bottom_right,
                        LensShadingAxis axis_x, LensShadingAxis axis_y) {
    precise float complement_x = 1.0 - axis_x.fraction;
    precise float complement_y = 1.0 - axis_y.fraction;
    precise vec4 upper_left_part = top_left * complement_x;
    precise vec4 upper_right_part = top_right * axis_x.fraction;
    precise vec4 upper = upper_left_part + upper_right_part;
    precise vec4 lower_left_part = bottom_left * complement_x;
    precise vec4 lower_right_part = bottom_right * axis_x.fraction;
    precise vec4 lower = lower_left_part + lower_right_part;
    precise vec4 upper_part = upper * complement_y;
    precise vec4 lower_part = lower * axis_y.fraction;
    precise vec4 gain = upper_part + lower_part;
    return gain;
}

// Усиления всех четырёх каналов в точке по осям: четыре загрузки vec4.
vec4 lens_shading_gains(LensShadingAxis axis_x, LensShadingAxis axis_y) {
    uint column = axis_x.low_node;
    uint row = axis_y.low_node;
    return lens_shading_blend(lens_shading_node(column, row), lens_shading_node(column + 1u, row),
                              lens_shading_node(column, row + 1u), lens_shading_node(column + 1u, row + 1u),
                              axis_x, axis_y);
}

// Усиление для одного отсчёта по его координате в полезной области (выборка «по отсчёту»).
float lens_shading_gain_at(uvec2 valid_pixel, uint map_channel) {
    return lens_shading_gains(lens_shading_axis_x(valid_pixel.x), lens_shading_axis_y(valid_pixel.y))[map_channel];
}

#endif
