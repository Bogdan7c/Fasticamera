// Логика предпросмотра RAW-мозаики. Включающий файл обязан до #include определить
//     uvec4 read_cell(uvec2 cell_origin)  - четыре значения ячейки CFA 2x2 с абсолютным началом
//                                           cell_origin, в порядке (x,y), (x+1,y), (x,y+1), (x+1,y+1).

const vec4 LETTERBOX_COLOR = vec4(0.0, 0.0, 0.0, 1.0);

// Канал CFA в позиции ячейки (0..3): 0 = красный, 1 = зелёный, 2 = синий.
uint channel_at(uint layout_code, uint position) {
    // Таблица по раскладкам: RGGB, GRBG, GBRG, BGGR (позиции в порядке строк).
    const uint CHANNELS[16] = uint[16](
        0u, 1u, 1u, 2u,
        1u, 0u, 2u, 1u,
        1u, 2u, 0u, 1u,
        2u, 1u, 1u, 0u);
    return CHANNELS[layout_code * 4u + position];
}

float normalized_level(uint value, uint black) {
    float numerator = float(int(value) - int(black));
    float denominator = float(max(int(parameters.white_level) - int(black), 1));
    return clamp(numerator / denominator, 0.0, 1.0);
}

void main() {
    uint physical_x = uint(gl_FragCoord.x);
    uint physical_y = uint(gl_FragCoord.y);

    // 1. Физические координаты цели -> логические (прямые), как у тестового паттерна.
    uint logical_x;
    uint logical_y;
    uint logical_width;
    uint logical_height;
    if (parameters.display_turns == 1u) {
        logical_x = parameters.target_height - 1u - physical_y;
        logical_y = physical_x;
        logical_width = parameters.target_height;
        logical_height = parameters.target_width;
    } else if (parameters.display_turns == 2u) {
        logical_x = parameters.target_width - 1u - physical_x;
        logical_y = parameters.target_height - 1u - physical_y;
        logical_width = parameters.target_width;
        logical_height = parameters.target_height;
    } else if (parameters.display_turns == 3u) {
        logical_x = physical_y;
        logical_y = parameters.target_width - 1u - physical_x;
        logical_width = parameters.target_height;
        logical_height = parameters.target_width;
    } else {
        logical_x = physical_x;
        logical_y = physical_y;
        logical_width = parameters.target_width;
        logical_height = parameters.target_height;
    }

    // 2. Размер прямой картинки: сенсорный кадр, повёрнутый по часовой на sensor_turns.
    bool sensor_swaps_axes = (parameters.sensor_turns & 1u) == 1u;
    uint upright_width = sensor_swaps_axes ? parameters.valid_height : parameters.valid_width;
    uint upright_height = sensor_swaps_axes ? parameters.valid_width : parameters.valid_height;

    // 3. Вписывание с сохранением пропорций (letterbox), целочисленно.
    uint fitted_width;
    uint fitted_height;
    if (logical_width * upright_height <= logical_height * upright_width) {
        fitted_width = logical_width;
        fitted_height = max(logical_width * upright_height / upright_width, 1u);
    } else {
        fitted_height = logical_height;
        fitted_width = max(logical_height * upright_width / upright_height, 1u);
    }
    uint offset_x = (logical_width - fitted_width) / 2u;
    uint offset_y = (logical_height - fitted_height) / 2u;
    if (logical_x < offset_x || logical_y < offset_y ||
        logical_x >= offset_x + fitted_width || logical_y >= offset_y + fitted_height) {
        out_color = LETTERBOX_COLOR;
        return;
    }

    // 4. Координаты в прямой картинке (floor-масштабирование).
    uint upright_x = min((logical_x - offset_x) * upright_width / fitted_width, upright_width - 1u);
    uint upright_y = min((logical_y - offset_y) * upright_height / fitted_height, upright_height - 1u);

    // 5. Прямая картинка -> координаты внутри полезной области сенсора (обратный поворот).
    uint sensor_x;
    uint sensor_y;
    if (parameters.sensor_turns == 1u) {
        sensor_x = upright_y;
        sensor_y = parameters.valid_height - 1u - upright_x;
    } else if (parameters.sensor_turns == 2u) {
        sensor_x = parameters.valid_width - 1u - upright_x;
        sensor_y = parameters.valid_height - 1u - upright_y;
    } else if (parameters.sensor_turns == 3u) {
        sensor_x = parameters.valid_width - 1u - upright_y;
        sensor_y = upright_x;
    } else {
        sensor_x = upright_x;
        sensor_y = upright_y;
    }

    // 6. Ячейка CFA 2x2, содержащая пиксель; начало ячейки выровнено по началу полезной области.
    uvec2 cell_origin = uvec2(parameters.valid_left + (sensor_x & ~1u), parameters.valid_top + (sensor_y & ~1u));
    uvec4 raw_values = read_cell(cell_origin);

    // 7. Нормализация по чёрному/белому и сборка RGB: R, среднее двух G, B.
    float red = 0.0;
    float blue = 0.0;
    float green_sum = 0.0;
    for (uint position = 0u; position < 4u; ++position) {
        float level = normalized_level(raw_values[position], parameters.black_levels[position]);
        uint channel = channel_at(parameters.cfa_layout, position);
        if (channel == 0u) {
            red = level;
        } else if (channel == 2u) {
            blue = level;
        } else {
            green_sum += level;
        }
    }
    out_color = vec4(red, green_sum * 0.5, blue, 1.0);
}
