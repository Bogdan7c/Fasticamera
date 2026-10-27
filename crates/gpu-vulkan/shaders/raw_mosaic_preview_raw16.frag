#version 450
#extension GL_EXT_samplerless_texture_functions : require
#extension GL_GOOGLE_include_directive : require
// Предпросмотр RAW16: кадр — картинка R16_UINT, читается как sampled image через texelFetch
// (камера отдаёт AHB только с usage GPU_SAMPLED_IMAGE, решение/03 дефект 6b).

#include "raw_mosaic_params.glsl"

layout(set = 0, binding = 0) uniform utexture2D frame;

uvec4 read_cell(uvec2 cell_origin) {
    ivec2 origin = ivec2(cell_origin);
    return uvec4(
        texelFetch(frame, origin, 0).r,
        texelFetch(frame, origin + ivec2(1, 0), 0).r,
        texelFetch(frame, origin + ivec2(0, 1), 0).r,
        texelFetch(frame, origin + ivec2(1, 1), 0).r);
}

#include "raw_mosaic_main.glsl"
