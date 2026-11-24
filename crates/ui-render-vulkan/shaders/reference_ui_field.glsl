// ЗАМОРОЖЕННАЯ КОПИЯ на коммите 896865d (до оптимизаций быстрых путей, сессия 16a). НЕ МЕНЯТЬ.
// Нужна только тесту `glass_fast_paths_exact`: он сравнивает бит-в-бит боевой шейдер с этим эталоном.
// Боевой рендер эту копию не использует (конвейер собирается только под `test-support`).
// Знаковое расстояние до фигуры и слияние форм. Повторяет ui-draw::shape (sdf.rs, blend.rs) выражение
// в выражение, в f32. Расстояния — в пикселях экрана, внутри фигуры отрицательные.

const float FIELD_EPSILON = 1.1920929e-7;  // f32::EPSILON эталона

// Знак эталона: sign(0) = +1 (у GLSL sign(0) = 0 — другая фигура на осях).
float sign_ref(float value) {
    return value >= 0.0 ? 1.0 : -1.0;
}

struct BoxField {
    float distance;
    vec2 gradient;
};

// geom = (половина ширины, половина высоты, радиус, показатель n); локальные координаты, центр в нуле.
BoxField box_field(vec2 p, vec4 geom) {
    float sx = sign_ref(p.x);
    float sy = sign_ref(p.y);
    float qx = abs(p.x) - (geom.x - geom.z);
    float qy = abs(p.y) - (geom.y - geom.z);
    BoxField result;
    if (qx > 0.0 && qy > 0.0) {
        float n = geom.w;
        float norm = pow(pow(qx, n) + pow(qy, n), 1.0 / n);
        float scale = pow(norm, 1.0 - n);
        result.gradient = vec2(sx * pow(qx, n - 1.0) * scale, sy * pow(qy, n - 1.0) * scale);
        result.distance = norm - geom.z;
        return result;
    }
    if (qx > qy) {
        result.distance = qx - geom.z;
        result.gradient = vec2(sx, 0.0);
    } else {
        result.distance = qy - geom.z;
        result.gradient = vec2(0.0, sy);
    }
    return result;
}

struct ScreenField {
    float distance_px;     // исправленное расстояние в пикселях экрана
    float distance_local;  // исправленное расстояние в локальных единицах (для обводки)
    float scale;           // длина экранного градиента локальной нормали (1/масштаб)
    vec2 normal;           // единичная нормаль наружу в экранных осях (0 — направления нет)
};

// Эталон member_field: inv0 = (a, b, tx), inv1 = (c, d, ty) — обратная матрица, посчитанная на CPU.
ScreenField member_field_at(vec2 screen, vec4 inv0, vec4 inv1, vec4 geom) {
    vec2 local = vec2(inv0.x * screen.x + inv0.y * screen.y + inv0.z,
                      inv1.x * screen.x + inv1.y * screen.y + inv1.z);
    BoxField box = box_field(local, geom);
    float gradient_length = length(box.gradient);
    float distance_local = box.distance;
    vec2 local_normal = vec2(0.0);
    if (gradient_length > FIELD_EPSILON) {
        local_normal = box.gradient / gradient_length;
        distance_local = box.distance / gradient_length;
    }
    // apply_gradient: (a·gx + c·gy, b·gx + d·gy) с коэффициентами ОБРАТНОЙ матрицы.
    vec2 screen_gradient = vec2(inv0.x * local_normal.x + inv1.x * local_normal.y,
                                inv0.y * local_normal.x + inv1.y * local_normal.y);
    float screen_length = length(screen_gradient);
    ScreenField field;
    field.distance_local = distance_local;
    if (screen_length > FIELD_EPSILON) {
        field.distance_px = distance_local / screen_length;
        field.scale = screen_length;
        field.normal = screen_gradient / screen_length;
    } else {
        field.distance_px = distance_local;
        field.scale = 1.0;
        field.normal = vec2(0.0);
    }
    return field;
}

// Эталон shape::coverage.
float coverage_ref(float distance_px, float aa_width_px) {
    if (aa_width_px <= 0.0) {
        return distance_px < 0.0 ? 1.0 : 0.0;
    }
    float t = clamp((0.5 * aa_width_px - distance_px) / aa_width_px, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

struct Merged {
    float distance;
    float weight_first;
    float weight_second;
};

// Эталон smooth_union_with_weights.
Merged smooth_union_ref(float first, float second, float merge_radius) {
    bool first_is_smaller = first <= second;
    float smaller = first_is_smaller ? first : second;
    Merged merged;
    if (merge_radius <= 0.0) {
        merged.distance = smaller;
        merged.weight_first = first_is_smaller ? 1.0 : 0.0;
        merged.weight_second = first_is_smaller ? 0.0 : 1.0;
        return merged;
    }
    float h = max(merge_radius - abs(first - second), 0.0) / merge_radius;
    float weight_smaller = 1.0 - 0.5 * h;
    float weight_larger = 0.5 * h;
    merged.distance = smaller - h * h * merge_radius * 0.25;
    merged.weight_first = first_is_smaller ? weight_smaller : weight_larger;
    merged.weight_second = first_is_smaller ? weight_larger : weight_smaller;
    return merged;
}
