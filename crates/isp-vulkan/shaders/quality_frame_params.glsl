// Параметры кадра качественного конвейера (uniform, std140). Включающий файл до #include задаёт
// FRAME_PARAMS_BINDING — номер привязки. Раскладку байт пишет Rust: isp_vulkan::quality::frame_params
// (тест там же сверяет смещения полей с этим блоком).
layout(set = 0, binding = FRAME_PARAMS_BINDING, std140) uniform FrameParameters {
    // x, y — левый верхний угол полезной области в кадре; z — шаг строки RAW10 в байтах; w — запас.
    uvec4 source_area;
    // Чёрный уровень по позициям квадрата 2x2 (порядок строк, как Camera2 BLACK_LEVEL_PATTERN).
    vec4 black_levels;
    // x — белый уровень; y, z, w — усиления баланса белого R, G, B.
    vec4 white_and_gains;
    // RGB камеры после баланса → XYZ D50 (строки матрицы в xyz).
    vec4 camera_to_xyz[3];
    // XYZ D50 → свет сцены в основных цветах экрана (строки в xyz).
    vec4 xyz_to_scene[3];
    // Сжатие охвата: xyz — порог по каналам, w — степень.
    vec4 gamut_threshold_power;
    // Сжатие охвата: xyz — масштаб по каналам.
    vec4 gamut_scale;
    // Тон: x — контраст c, y — s^c, z — 1/F(x_white), w — доля сохранения оттенка.
    vec4 tone;
} frame;
