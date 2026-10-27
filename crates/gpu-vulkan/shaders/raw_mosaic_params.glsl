// Параметры предпросмотра RAW-мозаики (включается из raw_mosaic_preview_raw16.frag / _raw10.frag
// ПЕРЕД определением read_cell; логика — в raw_mosaic_main.glsl).
// Это НЕ ISP: без баланса белого, демозаики и гаммы. Цель — доказать, что кадр камеры дошёл до
// экрана и ориентирован правильно. Формулы повторяет CPU-эталон `raw_mosaic_preview::reference`
// (менять их нужно вместе).

layout(push_constant) uniform MosaicParameters {
    uint target_width;     // физическая ширина цели, пиксели
    uint target_height;    // физическая высота цели, пиксели
    uint display_turns;    // preTransform дисплея: четверти оборота по часовой (0..3)
    uint sensor_turns;     // ориентация сенсора: четверти оборота по часовой (0..3)
    uint valid_left;       // полезная область в буфере: левый край (чётный)
    uint valid_top;        // верхний край (чётный)
    uint valid_width;      // ширина (чётная)
    uint valid_height;     // высота (чётная)
    uint cfa_layout;       // 0 RGGB, 1 GRBG, 2 GBRG, 3 BGGR
    uint white_level;
    uint black_levels[4];  // чёрный по позициям ячейки 2x2 в порядке строк
    uint row_stride_bytes; // шаг строки (используется только RAW10)
} parameters;

layout(location = 0) out vec4 out_color;

