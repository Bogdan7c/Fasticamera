// Вход ISP видоискателя: RAW16 — картинка `R16_UINT` без сэмплера (камера даёт AHB только с usage
// GPU_SAMPLED_IMAGE), чтение `texelFetch`. Включается ПОСЛЕ `viewfinder_isp.glsl` (прототип `read_raw`).
layout(set = 0, binding = 0) uniform utexture2D raw_frame;

uint read_raw(uint x, uint y) {
    return texelFetch(raw_frame, ivec2(x, y), 0).r;
}
