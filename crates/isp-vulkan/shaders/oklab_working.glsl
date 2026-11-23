// OKLab в f32 для рабочего пространства W на GPU. Эталон — isp-model `color/oklab_working.rs` (OklabWorkspace);
// матрицы приходят из uniform готовыми (считаются на CPU в f64 и сужаются до f32 один раз), здесь только
// применение. Общий файл для двух потребителей: возврата оттенка в основе AgX (`tone_base_agx.glsl`) и стадии
// цвета вида (`look_stage.glsl`): у каждого свои числа матриц в своём uniform-блоке, поэтому функции берут
// матрицы аргументом ([`OklabMatrices`]), а не читают блок сами.
//
// Правила:
//   * Матрицы считаются без FMA слева направо (`precise`) — как PixelMatrix::apply на CPU.
//   * Кубический корень со знаком — `sign(x)·|x|^(1/3)` через `pow` (в GLSL нет `cbrt`), как `signed_cube_root`
//     эталона (`abs().powf(1/3).copysign(x)`).
//   * Включать только один раз на шейдер (защита от повторного включения есть).
#ifndef OKLAB_WORKING_GLSL
#define OKLAB_WORKING_GLSL

// Четыре матрицы 3×3 по строкам (`w` строки — 0): W → LMS, LMS' → Lab, Lab → LMS', LMS → W.
struct OklabMatrices {
    vec4 working_to_lms[3];
    vec4 lms_prime_to_lab[3];
    vec4 lab_to_lms_prime[3];
    vec4 lms_to_working[3];
};

const float OKLAB_CUBE_ROOT_EXPONENT = 1.0 / 3.0;

// Строка матрицы · пиксель слева направо без FMA.
float oklab_matrix_row(vec4 row, vec3 pixel) {
    precise float value = (row.x * pixel.x + row.y * pixel.y) + row.z * pixel.z;
    return value;
}

vec3 oklab_apply(vec4 rows[3], vec3 pixel) {
    return vec3(oklab_matrix_row(rows[0], pixel), oklab_matrix_row(rows[1], pixel), oklab_matrix_row(rows[2], pixel));
}

// Кубический корень со знаком: отрицательный LMS вне охвата не даёт NaN.
float oklab_signed_cube_root(float value) {
    float root = pow(abs(value), OKLAB_CUBE_ROOT_EXPONENT);
    return value < 0.0 ? -root : root;
}

// Линейный RGB пространства W → OKLab (L, a, b) (OklabWorkspace::to_lab).
vec3 oklab_to_lab(OklabMatrices matrices, vec3 working_rgb) {
    vec3 lms = oklab_apply(matrices.working_to_lms, working_rgb);
    vec3 lms_prime = vec3(
        oklab_signed_cube_root(lms.x), oklab_signed_cube_root(lms.y), oklab_signed_cube_root(lms.z));
    return oklab_apply(matrices.lms_prime_to_lab, lms_prime);
}

// OKLab (L, a, b) → линейный RGB пространства W (OklabWorkspace::to_rgb).
vec3 oklab_to_working(OklabMatrices matrices, vec3 lab) {
    vec3 lms_prime = oklab_apply(matrices.lab_to_lms_prime, lab);
    return oklab_apply(matrices.lms_to_working, lms_prime * lms_prime * lms_prime);
}

#endif
