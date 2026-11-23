// Колёса цвета по зонам на GPU (решения 186, 191–193): сдвиг пикселя в OKLab по светлоте — Lift (тени), Gamma
// (середина), Gain (света), Offset (весь кадр). Эталон — isp-model `look/wheels.rs` (`LookWheels::apply`): здесь тот
// же порядок операций, без FMA (`precise`, как `oklab_working.glsl`), и БЕЗ встроенной `smoothstep` (она делит, а
// эталон умножает на обратные числа, посчитанные на CPU).
//
// Числа приходят готовыми в строках блока `LookParameters` (зеркало `src/look/mod.rs`): строка
// `LOOK_ROW_WHEELS` — (колёса включены, `L_g`, `1/L_g`, `1/(1 − L_g)`), следом строки lift, gamma, gain, offset как
// `(ΔL, Δa, Δb, 0)`.
//
// Включается только из `look_stage.glsl` (там объявлены `look` и `LOOK_ROW_WHEELS`); нейтральные колёса вызывающий
// пропускает по флагу в строке `LOOK_ROW_WHEELS`, x.
#ifndef LOOK_WHEELS_GLSL
#define LOOK_WHEELS_GLSL

// Веса зон по светлоте: x — тени, y — середина, z — света (сумма 1).
vec3 look_wheel_weights(float lightness, vec4 window) {
    // Через `L − L_g`, а не `L · (1/L_g)`: на сером и выше вес теней РОВНО 0 при любом сером (док эталона).
    precise float shadow_unit = clamp(1.0 + (lightness - window.y) * window.z, 0.0, 1.0);
    precise float highlight_unit = clamp((lightness - window.y) * window.w, 0.0, 1.0);
    precise float shadow_smooth = shadow_unit * shadow_unit * (3.0 - 2.0 * shadow_unit);
    precise float highlight_smooth = highlight_unit * highlight_unit * (3.0 - 2.0 * highlight_unit);
    precise float shadows = 1.0 - shadow_smooth;
    precise float highlights = highlight_smooth;
    precise float midtones = 1.0 - shadows - highlights;
    return vec3(shadows, midtones, highlights);
}

// Сдвигает пиксель OKLab `lab` = (L, a, b) (LookWheels::apply). Веса — по светлоте ДО сдвига.
vec3 look_apply_wheels(vec3 lab) {
    vec4 window = look.rows[LOOK_ROW_WHEELS];
    // window.x — флаг, y = L_g, z = 1/L_g, w = 1/(1 − L_g).
    vec3 weights = look_wheel_weights(lab.x, window);
    vec3 lift = look.rows[LOOK_ROW_WHEELS + 1u].xyz;
    vec3 gamma = look.rows[LOOK_ROW_WHEELS + 2u].xyz;
    vec3 gain = look.rows[LOOK_ROW_WHEELS + 3u].xyz;
    vec3 offset = look.rows[LOOK_ROW_WHEELS + 4u].xyz;
    vec3 shifted;
    // По каналам отдельно и слева направо — как `std::array::from_fn` на CPU.
    for (int channel = 0; channel < 3; ++channel) {
        precise float sum = lab[channel] + weights.x * lift[channel];
        sum = sum + weights.y * gamma[channel];
        sum = sum + weights.z * gain[channel];
        sum = sum + offset[channel];
        shifted[channel] = sum;
    }
    return shifted;
}

#endif
