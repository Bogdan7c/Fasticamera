#version 450
#extension GL_GOOGLE_include_directive : require
// Материал Liquid Glass. Формулы — выражение в выражение из ui-draw::glass::reference (shade_glass_pixel,
// lens_profile, refraction_offsets, vibrancy, backdrop_dimming, dark_edge, press_glow, specular_intensity).
//
// БЫСТРЫЕ ПУТИ (сессия 16a): пропуск работы, результат которой умножился бы на точный ноль, без изменения
// картинки. Замороженный эталон до оптимизаций — `reference_ui_glass.frag`; тест `glass_fast_paths_exact`
// требует побитового равенства кадров. Правила, выученные тестом (не нарушать при правках):
//  * условие быстрого пути — строгое равенство нейтральному значению (`== 0.0`, `== 1.0`): NaN и всё прочее
//    идёт прежним путём;
//  * ВЕТВЬ ставится вокруг дорогой части (`pow`, выборки, второе поле), а итоговая формула остаётся записанной
//    той же строкой: компилятор вправе склеить умножение со сложением в fma, и если форма выражения изменилась,
//    склейка меняется, а с ней последний бит f32 (так сломался первый вариант блика: `specular` стал phi-значением,
//    и `color + specular * effect` перестало склеиваться — 1 пиксель из 120 000 разошёлся на шаг fp16).
#include "ui_frame.glsl"
#include "ui_field.glsl"
#include "ui_glass_data.glsl"

layout(location = 0) in vec2 v_logical;
layout(location = 1) flat in uint v_group;
layout(location = 0) out vec4 out_color;  // premultiplied

const float EDGE_AA_PX = 1.0;
// Копия ui_draw::glass::reference::MATERIALIZE_LAYER_RAMP (паритет закреплён тестами): [estimate], подгонка, не Apple.
const float MATERIALIZE_LAYER_RAMP = 0.2;

// Как эталонный smoothstep: при edge0 == edge1 — ступенька.
float smoothstep_ref(float edge0, float edge1, float x) {
    if (abs(edge1 - edge0) <= FIELD_EPSILON) {
        return x >= edge1 ? 1.0 : 0.0;
    }
    float t = clamp((x - edge0) / (edge1 - edge0), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// Поле слитой формы группы: члены сворачиваются по порядку списка теми же весами, что в эталоне.
ScreenField group_field(vec2 screen, GlassGroup group) {
    float merge_radius = group.v5.z;
    ScreenField folded;
    vec2 blended = vec2(0.0);
    float folded_distance = 0.0;
    for (int i = 0; i < group.ints.y; ++i) {
        GlassMember member = members[group.ints.x + i];
        ScreenField field = member_field_at(screen, member.inv0, member.inv1, member.geom);
        if (i == 0) {
            folded_distance = field.distance_px;
            blended = field.normal;
        } else {
            Merged merged = smooth_union_ref(folded_distance, field.distance_px, merge_radius);
            blended = blended * merged.weight_first + field.normal * merged.weight_second;
            folded_distance = merged.distance;
        }
    }
    folded.distance_px = folded_distance;
    folded.distance_local = folded_distance;
    folded.scale = 1.0;
    float blended_length = length(blended);
    folded.normal = blended_length > FIELD_EPSILON ? blended / blended_length : vec2(0.0);
    return folded;
}

float lens_profile(float depth, float zone, float profile_kind) {
    if (zone <= 0.0 || depth >= zone) {
        return 0.0;
    }
    float u = 1.0 - max(depth, 0.0) / zone;
    float n = profile_kind > 0.5 ? 4.0 : 2.0;
    return 1.0 - pow(max(1.0 - pow(u, n), 0.0), 1.0 / n);
}

float rim_weight(float depth, float width) {
    if (width <= 0.0) {
        return 0.0;
    }
    return 1.0 - smoothstep_ref(0.0, width, max(depth, 0.0));
}

float luma_rec709(vec3 color) {
    return 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
}

// Средняя яркость фона под группой: одна на всю группу («контейнер выравнивает адаптацию», Apple 284) —
// 5 выборок самого мелкого уровня пирамиды (центр и четверти границ). Считается во ФРАГМЕНТНОМ шейдере:
// барьеры пирамиды (`pyramid.rs`) покрывают только стадию Fragment; чтение из вершинного шейдера было
// бы гонкой RAW/WAR с перестройкой пирамиды (ревью сессии 11). Все фрагменты группы берут одни и те же
// тексели крошечного уровня — выборки попадают в кэш.
float group_mean_luma(GlassGroup group) {
    int deepest = int(frame.light_misc.z + 0.5);
    vec2 center = group.bounds.xy + 0.5 * group.bounds.zw;
    vec2 quarter = 0.25 * group.bounds.zw;
    float luma = luma_rec709(pyramid_color(deepest, center));
    luma += luma_rec709(pyramid_color(deepest, center + vec2(quarter.x, quarter.y)));
    luma += luma_rec709(pyramid_color(deepest, center + vec2(-quarter.x, quarter.y)));
    luma += luma_rec709(pyramid_color(deepest, center + vec2(quarter.x, -quarter.y)));
    luma += luma_rec709(pyramid_color(deepest, center + vec2(-quarter.x, -quarter.y)));
    return luma / 5.0;
}

// Тень под стеклом = ui_draw::glass::reference::shade_glass_shadow / shade_glass_with_shadow (shadow_falloff):
// та же слитая форма, сдвинутая вниз. Возвращает premultiplied-цвет тени (нулевой вектор, если тени нет).
vec4 group_shadow(vec2 logical, GlassGroup group, float effect, float opacity) {
    vec4 shadow = vec4(0.0);
    float shadow_alpha = group.v4.y;
    if (shadow_alpha > 0.0) {
        float radius = group.v4.x;
        float shadow_sd = group_field(logical - vec2(0.0, group.v4.z), group).distance_px;
        float falloff;
        if (radius > 0.0) {
            float t = clamp(0.5 - shadow_sd / (2.0 * radius), 0.0, 1.0);
            falloff = t * t * (3.0 - 2.0 * t);
        } else {
            falloff = coverage_ref(shadow_sd, EDGE_AA_PX);
        }
        shadow = vec4(0.0, 0.0, 0.0, 1.0) * (falloff * shadow_alpha * opacity * effect);
    }
    return shadow;
}

void main() {
    GlassGroup group = groups[v_group];
    float effect = clamp(group.v5.w, 0.0, 1.0);
    float opacity = group.v6.x;

    ScreenField field = group_field(v_logical, group);
    float sd = field.distance_px;
    float layer_alpha = coverage_ref(sd, EDGE_AA_PX) * opacity * smoothstep_ref(0.0, MATERIALIZE_LAYER_RAMP, effect);

    // Стекла нет в этой точке: остаётся только тень (она считается до раннего выхода, как в эталоне).
    if (layer_alpha <= 0.0) {
        out_color = group_shadow(v_logical, group, effect, opacity);
        return;
    }

    float depth = -sd;
    float lens_zone = group.v0.x;
    float refraction_strength = group.v0.y;
    float dispersion = group.v1.x;
    float press_amount = clamp(group.v6.y, 0.0, 1.0);
    vec2 touch_point = group.v6.zw;

    // (1) Линза: каждый канал берётся из своей точки фона (смещение НАРУЖУ по нормали).
    float profile = lens_profile(depth, lens_zone, group.v3.w);
    float press_gain = 1.0 + group.v4.w * press_amount;
    vec2 green_offset = field.normal * (refraction_strength * press_gain * profile) * effect;
    float lod = blur_lod(group.v0.z);
    // Быстрый путь «нет преломления» (сессия 16a): если все три точки выборки совпадают, одного вызова хватает.
    //  * dispersion == 0: красная точка = v + g * (1 - 0) = v + g, синяя = v + g * (1 + 0) = v + g (в f32 g * 1 = g),
    //    зелёная = v + g — все три одинаковы;
    //  * green_offset == 0 (глубже зоны линзы, нулевая сила или материализация): g * (1 ∓ d) = ±0 при конечном d
    //    (дисперсия стиля 0..0,5), а v + (±0) = v — все три точки равны v.
    // Ветвь написана здесь, а не в функции: так компилятор строит те же инструкции, что у эталона.
    vec3 color;
    if (dispersion == 0.0 || green_offset == vec2(0.0)) {
        color = blurred_backdrop(v_logical + green_offset, lod);
    } else {
        color = vec3(
            blurred_backdrop(v_logical + green_offset * (1.0 - dispersion), lod).r,
            blurred_backdrop(v_logical + green_offset, lod).g,
            blurred_backdrop(v_logical + green_offset * (1.0 + dispersion), lod).b);
    }

    // (2) Насыщенность, ослабленная до 1 при effect -> 0.
    float saturation = 1.0 + (group.v0.w - 1.0) * effect;
    float luma = 0.2126 * color.r + 0.7152 * color.g + 0.0722 * color.b;
    color = clamp(vec3(luma) + (color - vec3(luma)) * saturation, 0.0, 1.0);

    // (3) Плёнка тона: светлая над тёмным фоном, тёмная над светлым. (4) Затемнение: смесь с чёрным.
    // Быстрый путь (сессия 16a): если альфа плёнки и потолок затемнения — точные нули, обе смеси равны
    // mix(color, c, 0) = color * (1 - 0) + c * 0 = color (c конечен), и 5 выборок средней яркости не нужны.
    // Оба условия — свойства группы (одинаковы у всех пикселей группы).
    float tint_alpha = group.v3.y * group.v3.z * effect;
    if (tint_alpha != 0.0 || group.v2.z != 0.0) {
        float dimming_factor = smoothstep_ref(group.v2.w, group.v3.x, group_mean_luma(group));
        float tone_value = 1.0 - dimming_factor;
        color = mix(color, vec3(tone_value), tint_alpha);
        color = mix(color, vec3(0.0), group.v2.z * dimming_factor * effect);
    }

    // (5) Тёмный край.
    float dark_edge = group.v2.x * rim_weight(depth, group.v2.y);
    color = mix(color, vec3(0.0), dark_edge * effect);

    // (6) Свечение от касания: смесь с белым.
    float glow_radius = group.v5.x;
    float glow = 0.0;
    if (press_amount > 0.0 && glow_radius > 0.0) {
        float falloff = clamp(1.0 - length(v_logical - touch_point) / glow_radius, 0.0, 1.0);
        glow = group.v5.y * press_amount * falloff * falloff;
    }
    color = mix(color, vec3(1.0), glow * effect);

    // (7) Блик: белый добавляется к цвету; |dot(нормаль, свет)|^показатель на ободке.
    // Быстрый путь (сессия 16a): вне ободка rim_weight — точный ноль, и `pow` не нужен: alignment_power = 0
    // даёт alpha * 0 * 0 * effect = +0, как и у эталона (там pow конечен: основание не меньше 1e-30, показатель
    // стиля 0,1..16, а k * 0 = 0). Итоговая строка `specular` оставлена в прежней форме (см. правила выше).
    float rim = rim_weight(depth, group.v1.z);
    float alignment_power = 0.0;
    if (rim != 0.0) {
        float alignment = abs(dot(field.normal, frame.light_misc.xy));
        alignment_power = pow(max(alignment, 1.0e-30), group.v1.w);
    }
    float specular = group.v1.y * rim * alignment_power * effect;
    color = min(color + vec3(specular), vec3(1.0));

    vec4 glass = vec4(color * layer_alpha, layer_alpha);
    // Быстрый путь (сессия 16a): при layer_alpha == 1 вес тени (1 - glass.a) — точный ноль, а тень конечна,
    // поэтому glass + shadow * 0 = glass точно, и второе поле формы для тени не считается. Для панели
    // на пол-экрана это почти все пиксели (непрозрачность 1, материализация завершена).
    if (layer_alpha == 1.0) {
        out_color = glass;
        return;
    }
    vec4 shadow = group_shadow(v_logical, group, effect, opacity);
    out_color = glass + shadow * (1.0 - glass.a);
}
