#version 450
// Тестовый паттерн «градиент + метка кадра», ЦЕЛОЧИСЛЕННО ТОЧНЫЙ.
//
// Все значения каналов считаются в целых числах (0..levels) из целочисленных координат пикселя,
// и только в конце делятся на levels. Для UNORM8 levels = 255: значение v/255.0 после записи
// в цель возвращается ровно в v. Для 10-битных форматов levels = 1023 (v/1023.0 точно так же).
// CPU-эталон (`test_pattern::reference`) повторяет формулу бит-в-бит; менять их нужно вместе.
//
// Поворот: паттерн определён в «логических» (прямых) координатах, а цель повёрнута как
// `preTransform`. quarter_turns = 0/1/2/3 соответствует IDENTITY/ROTATE_90/180/270 (по часовой,
// как в VkSurfaceTransformFlagBitsKHR): дисплей повернёт картинку, и она встанет прямо.

layout(push_constant) uniform PatternParameters {
    uint width;          // физическая ширина цели, пиксели
    uint height;         // физическая высота цели, пиксели
    uint frame;          // номер кадра (его биты рисуются в метке)
    uint quarter_turns;  // 0, 1, 2, 3
    uint levels;         // максимум канала: 255 или 1023
} parameters;

layout(location = 0) out vec4 out_color;

const uint MARKER_CELLS = 8u;           // клеток в метке = бит номера кадра
const uint MARKER_MIN_HEIGHT = 1u;      // минимальная высота полосы метки
const uint MARKER_HEIGHT_DIVISOR = 8u;  // полоса = высота / 8
const uint BLUE_FRAME_STEP = 37u;       // шаг сдвига синего по номеру кадра

void main() {
    // Целочисленные координаты пикселя: gl_FragCoord = (x + 0.5, y + 0.5), отбрасываем 0.5.
    uint physical_x = uint(gl_FragCoord.x);
    uint physical_y = uint(gl_FragCoord.y);

    // Физические координаты -> логические (прямые) и логический размер.
    uint logical_x;
    uint logical_y;
    uint logical_width;
    uint logical_height;
    if (parameters.quarter_turns == 1u) {
        logical_x = parameters.height - 1u - physical_y;
        logical_y = physical_x;
        logical_width = parameters.height;
        logical_height = parameters.width;
    } else if (parameters.quarter_turns == 2u) {
        logical_x = parameters.width - 1u - physical_x;
        logical_y = parameters.height - 1u - physical_y;
        logical_width = parameters.width;
        logical_height = parameters.height;
    } else if (parameters.quarter_turns == 3u) {
        logical_x = physical_y;
        logical_y = parameters.width - 1u - physical_x;
        logical_width = parameters.height;
        logical_height = parameters.width;
    } else {
        logical_x = physical_x;
        logical_y = physical_y;
        logical_width = parameters.width;
        logical_height = parameters.height;
    }

    uint levels = parameters.levels;
    uint red;
    uint green;
    uint blue;
    uint marker_band_height = max(logical_height / MARKER_HEIGHT_DIVISOR, MARKER_MIN_HEIGHT);
    if (logical_y < marker_band_height) {
        // Метка кадра: клетка k светится, если бит k номера кадра равен 1.
        uint cell = logical_x * MARKER_CELLS / logical_width;
        uint bit = (parameters.frame >> cell) & 1u;
        red = bit * levels;
        green = red;
        blue = red;
    } else {
        // Градиент: красный растёт слева направо, зелёный сверху вниз,
        // синий идёт по диагонали и сдвигается номером кадра.
        red = logical_x * levels / max(logical_width - 1u, 1u);
        green = logical_y * levels / max(logical_height - 1u, 1u);
        uint diagonal = (logical_x + logical_y) * levels / max(logical_width + logical_height - 2u, 1u);
        blue = (diagonal + parameters.frame * BLUE_FRAME_STEP) % (levels + 1u);
    }

    float scale = float(levels);
    out_color = vec4(float(red) / scale, float(green) / scale, float(blue) / scale, 1.0);
}
