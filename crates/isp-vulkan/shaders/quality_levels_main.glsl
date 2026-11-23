// Проход 0 «уровни и баланс»: RAW → нормированная и сбалансированная мозаика f32 (плоскость mosaic).
// Формулы — isp-model levels.rs и white_balance.rs:
//   linear   = max(raw − black[позиция 2x2], 0) / (white − black[позиция 2x2])
//   balanced = min(linear · gain[цвет фильтра], 1)
// Между ними — коррекция виньетирования (isp_model::lens_shading, shaders/lens_shading.glsl):
//   shaded = linear · gain(пиксель полезной области, канал карты); balanced = min(shaded · gain_wb, 1)
// Карта выключена (в буфере заглушка «пропуск») — shaded = linear без умножения: вывод бит в бит прежний.
// Включающий файл до #include определяет uint raw_at(uint frame_x, uint frame_y) — отсчёт RAW в
// координатах всего кадра, и ПОСЛЕ #include — `uvec2 valid_area_pixel(uint x, uint y)`: координаты в
// ПОЛЕЗНОЙ ОБЛАСТИ кадра для пикселя плоскости (x, y) (кадр целиком — те же x, y; полоса снимка — со
// смещением области полосы). Привязку буфера карты (LENS_SHADING_MAP_BINDING) включающий файл тоже задаёт.

#include "quality_common.glsl"
#include "lens_shading.glsl"

// Прототип: тело — у включающего файла (знает, где область плоскостей лежит в полезной области).
uvec2 valid_area_pixel(uint x, uint y);

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
    uint color = channel_at(x, y);
    precise float shaded = linear;
    if (lens_shading_enabled()) {
        uvec2 valid_pixel = valid_area_pixel(x, y);
        precise float shading_gain = lens_shading_gain_at(valid_pixel, lens_shading_map_channel(color, valid_pixel.y));
        shaded = linear * shading_gain;
    }
    float gain = frame.white_and_gains[1u + color];
    precise float balanced = min(shaded * gain, 1.0);
    mosaic.values[y * geometry.width + x] = balanced;
}
