//! Push-константы: блок кадра (`FrameParams`, 96 байт) и блок ступени размытия (`BlurParams`, 32 байта).

use super::put_floats;

/// Размер блока кадра.
pub(crate) const FRAME_PUSH_BYTES: u32 = 96;
/// Размер блока ступени размытия.
pub(crate) const BLUR_PUSH_BYTES: u32 = 32;

/// Блок кадра для конвейеров фигур, глифов и стекла.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FramePush {
    /// Логический размер, px.
    pub logical_size: [f32; 2],
    /// Физический размер цели, px.
    pub physical_size: [f32; 2],
    /// Размещение фона: x, y, ширина, высота.
    pub placement: [f32; 4],
    /// Направление света.
    pub light: [f32; 2],
    /// Число уровней пирамиды.
    pub pyramid_levels: u32,
    /// Фон живой (иначе Absent).
    pub backdrop_live: bool,
    /// Заливка вместо фона.
    pub absent_fill: [f32; 4],
    /// Spread атласа SDF, текселей.
    pub atlas_spread_texels: f32,
    /// Четверти поворота цели.
    pub quarter_turns: u32,
    /// Номер первого экземпляра пакета.
    pub first_instance: u32,
}

impl FramePush {
    /// Байты блока в раскладке `FrameParams`.
    pub(crate) fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FRAME_PUSH_BYTES as usize);
        put_floats(
            &mut bytes,
            &[
                self.logical_size[0],
                self.logical_size[1],
                self.physical_size[0],
                self.physical_size[1],
            ],
        );
        put_floats(&mut bytes, &self.placement);
        let live = if self.backdrop_live { 1.0 } else { 0.0 };
        put_floats(
            &mut bytes,
            &[self.light[0], self.light[1], self.pyramid_levels as f32, live],
        );
        put_floats(&mut bytes, &self.absent_fill);
        put_floats(&mut bytes, &[self.atlas_spread_texels, 0.0, 0.0, 0.0]);
        for word in [self.quarter_turns, self.first_instance, 0, 0] {
            bytes.extend_from_slice(&word.to_ne_bytes());
        }
        bytes
    }
}

/// Блок ступени размытия.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlurPush {
    /// Размещение фона (для первой ступени).
    pub destination: [f32; 4],
    /// Сдвиг диагональных выборок, px.
    pub tap_offset_px: f32,
    /// px экрана на тексель цели.
    pub target_texel_px: f32,
    /// px экрана на тексель источника-уровня.
    pub source_texel_px: f32,
}

impl BlurPush {
    /// Байты блока в раскладке `BlurParams`.
    pub(crate) fn to_bytes(self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(BLUR_PUSH_BYTES as usize);
        put_floats(&mut bytes, &self.destination);
        put_floats(
            &mut bytes,
            &[self.tap_offset_px, self.target_texel_px, self.source_texel_px, 0.0],
        );
        bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_blocks_have_declared_sizes_and_field_order() {
        let frame = FramePush {
            logical_size: [1.0, 2.0],
            physical_size: [3.0, 4.0],
            placement: [5.0, 6.0, 7.0, 8.0],
            light: [9.0, 10.0],
            pyramid_levels: 4,
            backdrop_live: true,
            absent_fill: [11.0, 12.0, 13.0, 14.0],
            atlas_spread_texels: 15.0,
            quarter_turns: 3,
            first_instance: 21,
        };
        let bytes = frame.to_bytes();
        assert_eq!(bytes.len(), FRAME_PUSH_BYTES as usize);
        let word = |index: usize| {
            u32::from_ne_bytes([
                bytes[index * 4],
                bytes[index * 4 + 1],
                bytes[index * 4 + 2],
                bytes[index * 4 + 3],
            ])
        };
        assert_eq!(f32::from_bits(word(2)), 3.0);
        assert_eq!(f32::from_bits(word(10)), 4.0, "число уровней");
        assert_eq!(f32::from_bits(word(11)), 1.0, "фон живой");
        assert_eq!(f32::from_bits(word(16)), 15.0);
        assert_eq!((word(20), word(21)), (3, 21));
        let blur = BlurPush {
            destination: [1.0; 4],
            tap_offset_px: 2.0,
            target_texel_px: 4.0,
            source_texel_px: 2.0,
        };
        assert_eq!(blur.to_bytes().len(), BLUR_PUSH_BYTES as usize);
    }
}
