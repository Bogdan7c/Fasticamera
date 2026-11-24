// Диагностический проход стенда V4 («проба полосы памяти»): ничего не считает, только читает READ_COUNT плоскостей
// и пишет WRITE_COUNT плоскостей того же размера, что у настоящего прохода Menon с таким же числом чтений и
// записей. Цена пробы = цена самого трафика памяти и запуска групп; разница с настоящим проходом = цена его
// математики и разброса адресов. К картинке отношения не имеет: выход пробного кадра бессмыслен (стенд, не продукт).
//
// Включающий файл до #include задаёт:
//   READ_COUNT, WRITE_COUNT — число плоскостей (привязки 0..READ_COUNT-1 читаются, следующие пишутся);
//   PROBE_ELEMENT — тип отсчёта (float — как у настоящих плоскостей f32; float16_t — «как если бы плоскости были fp16»);
//   PROBE_USES_HALF — определено, если нужен fp16-доступ к буферам.
#ifdef PROBE_USES_HALF
#extension GL_EXT_shader_16bit_storage : require
#extension GL_EXT_shader_explicit_arithmetic_types_float16 : require
#endif

#include "quality_common.glsl"

layout(set = 0, binding = 0, std430) readonly buffer Plane0 { PROBE_ELEMENT values[]; } plane_0;
#if READ_COUNT > 1
layout(set = 0, binding = 1, std430) readonly buffer Plane1 { PROBE_ELEMENT values[]; } plane_1;
#endif
#if READ_COUNT > 2
layout(set = 0, binding = 2, std430) readonly buffer Plane2 { PROBE_ELEMENT values[]; } plane_2;
#endif
#if READ_COUNT > 3
layout(set = 0, binding = 3, std430) readonly buffer Plane3 { PROBE_ELEMENT values[]; } plane_3;
#endif
layout(set = 0, binding = READ_COUNT, std430) writeonly buffer Out0 { PROBE_ELEMENT values[]; } out_0;
#if WRITE_COUNT > 1
layout(set = 0, binding = READ_COUNT + 1, std430) writeonly buffer Out1 { PROBE_ELEMENT values[]; } out_1;
#endif

void main() {
    uint x;
    uint y;
    if (!pixel_coordinates(x, y)) {
        return;
    }
    uint index = y * geometry.width + x;
    PROBE_ELEMENT sum = plane_0.values[index];
#if READ_COUNT > 1
    sum += plane_1.values[index];
#endif
#if READ_COUNT > 2
    sum += plane_2.values[index];
#endif
#if READ_COUNT > 3
    sum += plane_3.values[index];
#endif
    out_0.values[index] = sum;
#if WRITE_COUNT > 1
    out_1.values[index] = sum + PROBE_ELEMENT(1.0);
#endif
}
