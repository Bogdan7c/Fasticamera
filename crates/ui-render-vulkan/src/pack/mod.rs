//! Упаковка списка рисования в байты для GPU: раскладка std430, совпадающая со структурами в шейдерах.
//!
//! Одно место на каждый вид данных; соответствие структурам `ui_*_data.glsl` закреплено константами
//! размера и тестами GPU-паритета.

mod glass;
mod glyph;
mod image;
mod push;
mod shape;

pub(crate) use glass::{GLASS_GROUP_BYTES, GLASS_MEMBER_BYTES, pack_glass_group};
pub(crate) use glyph::{GLYPH_INSTANCE_BYTES, pack_icon, pack_text};
pub(crate) use image::{IMAGE_INSTANCE_BYTES, ImageSamplerSlot, pack_image};
pub(crate) use push::{BLUR_PUSH_BYTES, BlurPush, FRAME_PUSH_BYTES, FramePush};
pub(crate) use shape::{SHAPE_INSTANCE_BYTES, pack_shape};

use ui_draw::geometry::PxRect;
use ui_draw::transform::Affine2;

/// Дописывает `f32` в нативном порядке байт (host и GPU — одной endianness).
pub(crate) fn put_floats(out: &mut Vec<u8>, values: &[f32]) {
    for value in values {
        out.extend_from_slice(&value.to_ne_bytes());
    }
}

/// Дописывает целые `i32` (`ivec4` шейдера).
pub(crate) fn put_ints(out: &mut Vec<u8>, values: &[i32]) {
    for value in values {
        out.extend_from_slice(&value.to_ne_bytes());
    }
}

/// Прямоугольник как `vec4` (x, y, ширина, высота).
pub(crate) fn put_rect(out: &mut Vec<u8>, rect: &PxRect) {
    put_floats(out, &[rect.origin.x, rect.origin.y, rect.size.width, rect.size.height]);
}

/// Две строки матрицы 2×3 как два `vec4` (a, b, tx, 0) и (c, d, ty, 0).
pub(crate) fn put_affine_rows(out: &mut Vec<u8>, matrix: &Affine2) {
    put_floats(
        out,
        &[matrix.a, matrix.b, matrix.tx, 0.0, matrix.c, matrix.d, matrix.ty, 0.0],
    );
}
