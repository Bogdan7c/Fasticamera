// Тон вида на GPU (решения 236, 242–244): контраст, средние тона, тени, света — над светом экрана W сразу после основы
// тона, одинаково для всех моделей. Эталон — isp-model `look/tone_shape.rs` (`LookTone::{curve, apply}`): формулы,
// обоснование и гарантии там; здесь тот же порядок операций, без FMA (`precise`).
//
// Числа приходят готовыми строками блока `LookParameters` (зеркало `src/look/mod.rs`):
//   LOOK_ROW_TONE     — (тон включён, q, 1/q, 1/(1 − q)),
//   LOOK_ROW_TONE + 1 — (наклоны: тени у чёрного, света у белого, контраст на сером, средние тона у чёрного),
//   LOOK_ROW_TONE + 2 — (доля растянутого цвета, 0, 0, 0).
//
// Включается только из `look_stage.glsl` (там объявлены `look` и `LOOK_ROW_TONE`).
#ifndef LOOK_TONE_GLSL
#define LOOK_TONE_GLSL

// Наибольший канал, ниже которого пиксель — чёрный (зеркало `BLACK_FLOOR` эталона).
const float LOOK_TONE_BLACK_FLOOR = 1e-10;

// Отрезок зоны: 0 → 0, 1 → 1, наклон `slope_at_zero` в нуле и 1 в единице.
float look_tone_zone(float unit, float slope_at_zero) {
    precise float bump = unit * (1.0 - unit);
    precise float numerator = unit * unit + slope_at_zero * bump;
    precise float denominator = 1.0 + (slope_at_zero - 1.0) * bump;
    return numerator / denominator;
}

// Отрезок контраста: 0 → 0, 1 → 1, наклон `slope_at_one` в единице и обратный ему в нуле.
float look_tone_contrast(float unit, float slope_at_one) {
    precise float denominator = unit + slope_at_one * (1.0 - unit);
    return unit / denominator;
}

// Кривая одного канала света экрана; вне (0, 1) — тождество.
float look_tone_curve(float light, vec4 window, vec4 slopes) {
    if (light <= 0.0 || light >= 1.0) {
        return light;
    }
    float t = sqrt(light);
    precise float shaped;
    if (t <= window.y) {
        precise float from_black = min(t * window.z, 1.0);
        shaped = window.y * look_tone_contrast(look_tone_zone(from_black, slopes.x), slopes.z);
    } else {
        precise float from_white = min((1.0 - t) * window.w, 1.0);
        shaped = 1.0 - (1.0 - window.y) * look_tone_contrast(look_tone_zone(from_white, slopes.y), slopes.z);
    }
    precise float denominator = 1.0 + (slopes.w - 1.0) * shaped;
    precise float moved = slopes.w * shaped / denominator;
    return moved * moved;
}

// Стадия тона над светом W одного пикселя (LookTone::apply). Нейтральные ручки тона (флаг 0) — вход как есть.
vec3 look_apply_tone(vec3 working_rgb) {
    vec4 window = look.rows[LOOK_ROW_TONE];
    if (window.x < 0.5) {
        return working_rgb;
    }
    float brightest = max(max(working_rgb.r, working_rgb.g), working_rgb.b);
    float darkest = min(min(working_rgb.r, working_rgb.g), working_rgb.b);
    if (brightest <= LOOK_TONE_BLACK_FLOOR) {
        return working_rgb;
    }
    vec4 slopes = look.rows[LOOK_ROW_TONE + 1u];
    float color_follow = look.rows[LOOK_ROW_TONE + 2u].x;
    float new_brightest = look_tone_curve(brightest, window, slopes);
    float new_darkest = look_tone_curve(darkest, window, slopes);
    precise float gain = new_brightest / brightest;
    precise float span = brightest - darkest;
    precise float new_span = new_brightest - new_darkest;
    vec3 toned;
    for (int channel = 0; channel < 3; ++channel) {
        float value = working_rgb[channel];
        // Отрицательный канал (за охватом) усиление не трогает — как в эталоне.
        precise float similar = value > 0.0 ? value * gain : value;
        precise float stretched = span > 0.0 ? new_darkest + new_span * ((value - darkest) / span) : new_brightest;
        precise float mixed = similar + color_follow * (stretched - similar);
        toned[channel] = mixed;
    }
    return toned;
}

#endif
