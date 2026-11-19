// Данные стекла: группы и члены (SSBO std430) и выборка фона. Rust-сторона — `src/pack/glass.rs`.
// Привязки: 0 группы, 1 члены, 2 фон (полное разрешение), 3..8 уровни пирамиды 1..6.

struct GlassGroup {
    vec4 bounds;  // область рисования на логическом экране
    vec4 v0;      // толщина зоны линзы, сила преломления, радиус размытия, насыщенность
    vec4 v1;      // дисперсия, блик: альфа, ширина ободка, показатель
    vec4 v2;      // тёмный край: сила, ширина; затемнение: потолок, нижняя яркость
    vec4 v3;      // затемнение: верхняя яркость; тон: уровень, альфа при полном; профиль края (0 — окружность, 1 — суперэллипс)
    vec4 v4;      // тень: радиус, альфа, смещение вниз; нажатие: усиление преломления
    vec4 v5;      // свечение: радиус, альфа; радиус слияния; материализация
    vec4 v6;      // прозрачность группы; нажатие группы: сила, касание x, касание y (экранные px)
    ivec4 ints;   // x: первый член в массиве членов; y: число членов
};

struct GlassMember {
    vec4 inv0;  // обратная матрица, строка 0: a, b, tx, 0
    vec4 inv1;  // строка 1: c, d, ty, 0
    vec4 geom;  // половина ширины, половина высоты, радиус, показатель
};

layout(std430, set = 0, binding = 0) readonly buffer GlassGroups { GlassGroup groups[]; };
layout(std430, set = 0, binding = 1) readonly buffer GlassMembers { GlassMember members[]; };
layout(set = 0, binding = 2) uniform sampler2D backdrop_image;
layout(set = 0, binding = 3) uniform sampler2D pyramid_1;
layout(set = 0, binding = 4) uniform sampler2D pyramid_2;
layout(set = 0, binding = 5) uniform sampler2D pyramid_3;
layout(set = 0, binding = 6) uniform sampler2D pyramid_4;
layout(set = 0, binding = 7) uniform sampler2D pyramid_5;
layout(set = 0, binding = 8) uniform sampler2D pyramid_6;

// Уровень пирамиды k (1..6) в точке экрана: уровень хранится в системе координат экрана,
// тексель уровня k покрывает 2^k px.
vec3 pyramid_color(int level, vec2 screen_px) {
    float texel_px = exp2(float(level));
    switch (level) {
        case 1: return textureLod(pyramid_1, screen_px / (vec2(textureSize(pyramid_1, 0)) * texel_px), 0.0).rgb;
        case 2: return textureLod(pyramid_2, screen_px / (vec2(textureSize(pyramid_2, 0)) * texel_px), 0.0).rgb;
        case 3: return textureLod(pyramid_3, screen_px / (vec2(textureSize(pyramid_3, 0)) * texel_px), 0.0).rgb;
        case 4: return textureLod(pyramid_4, screen_px / (vec2(textureSize(pyramid_4, 0)) * texel_px), 0.0).rgb;
        case 5: return textureLod(pyramid_5, screen_px / (vec2(textureSize(pyramid_5, 0)) * texel_px), 0.0).rgb;
        default: return textureLod(pyramid_6, screen_px / (vec2(textureSize(pyramid_6, 0)) * texel_px), 0.0).rgb;
    }
}

// Фон полного разрешения: вне прямоугольника размещения — чёрный; Absent — цвет заливки.
vec3 sharp_backdrop(vec2 screen_px) {
    if (frame.light_misc.w < 0.5) {
        return frame.absent_fill.rgb;
    }
    vec2 relative = (screen_px - frame.placement.xy) / frame.placement.zw;
    if (any(lessThan(relative, vec2(0.0))) || any(greaterThanEqual(relative, vec2(1.0)))) {
        return vec3(0.0);
    }
    return textureLod(backdrop_image, relative, 0.0).rgb;
}

// Размытый фон: `lod` 0 — резкий, целое k — уровень k, между — смесь соседних уровней.
vec3 blurred_backdrop(vec2 screen_px, float lod) {
    if (lod <= 0.0) {
        return sharp_backdrop(screen_px);
    }
    int levels = int(frame.light_misc.z + 0.5);
    float floor_lod = floor(lod);
    int lower = int(floor_lod);
    int upper = min(lower + 1, levels);
    vec3 lower_color = lower == 0 ? sharp_backdrop(screen_px) : pyramid_color(lower, screen_px);
    vec3 upper_color = pyramid_color(upper, screen_px);
    return mix(lower_color, upper_color, lod - floor_lod);
}

// Уровень по радиусу размытия стиля, px: 0 — резко. Приближение dual Kawase: после k ступеней вниз
// эффективная сигма ≈ 0,75 · 2^k px, радиус стиля ≈ 2 сигмы, поэтому k = log2(radius / 1,5).
// [estimate]: подгонка, не Apple.
float blur_lod(float radius_px) {
    if (radius_px <= 0.0) {
        return 0.0;
    }
    float levels = frame.light_misc.z;
    return clamp(log2(max(radius_px, 1.5) / 1.5), 0.0, levels);
}
