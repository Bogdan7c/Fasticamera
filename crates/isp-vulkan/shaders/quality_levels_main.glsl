// Проход 0 «уровни и баланс»: RAW → нормированная и сбалансированная мозаика f32 (плоскость mosaic).
// Формулы — isp-model levels.rs и white_balance.rs:
//   linear   = max(raw − black[позиция 2x2], 0) / (white − black[позиция 2x2])
//   balanced = min(linear · gain[цвет фильтра], 1)
// Включающий файл до #include определяет uint raw_at(uint frame_x, uint frame_y) — отсчёт RAW в
// координатах всего кадра.

#include "quality_common.glsl"

layout(set = 0, binding = 2, std430) writeonly buffer MosaicPlane {
    float values[];
} mosaic;

void main() {
    uint x;
    uint y;
    if (!pixel_coordinates(x, y)) {
        return;
    }
    uint raw = raw_at(frame.source_area.x + x, frame.source_area.y + y);
    // Позиция квадрата 2x2 считается от начала полезной области (её левый верхний угол чётный).
    uint cell = (y & 1u) * 2u + (x & 1u);
    float black = frame.black_levels[cell];
    precise float range = frame.white_and_gains.x - black;
    precise float linear = divide_correctly_rounded(max(float(raw) - black, 0.0), range);
    float gain = frame.white_and_gains[1u + channel_at(x, y)];
    precise float balanced = min(linear * gain, 1.0);
    mosaic.values[y * geometry.width + x] = balanced;
}
