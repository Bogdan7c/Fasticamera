//! Разбор `OpcodeList2/3` по DNG 1.7, раздел «Opcode Lists»: список ВСЕГДА big-endian, независимо от порядка
//! байт TIFF. Независимая от писателя реализация: тесты сверяют не «что мы записали тем же кодом», а то, что
//! прочитает любой читатель по спецификации.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "тестовый помощник"
)]

/// Один разобранный опкод `GainMap` (ID 9).
#[derive(Debug, Clone, PartialEq)]
pub struct GainMapOpcode {
    pub version: [u8; 4],
    pub flags: u32,
    pub top: u32,
    pub left: u32,
    pub bottom: u32,
    pub right: u32,
    pub plane: u32,
    pub planes: u32,
    pub row_pitch: u32,
    pub col_pitch: u32,
    pub points_vertical: u32,
    pub points_horizontal: u32,
    pub spacing_vertical: f64,
    pub spacing_horizontal: f64,
    pub origin_vertical: f64,
    pub origin_horizontal: f64,
    pub map_planes: u32,
    /// Усиления по строкам (`points_vertical × points_horizontal × map_planes`).
    pub gains: Vec<f32>,
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> [u8; N] {
        let chunk: [u8; N] = self.bytes[self.position..self.position + N].try_into().unwrap();
        self.position += N;
        chunk
    }
    fn u32(&mut self) -> u32 {
        u32::from_be_bytes(self.take())
    }
    fn f64(&mut self) -> f64 {
        f64::from_be_bytes(self.take())
    }
    fn f32(&mut self) -> f32 {
        f32::from_be_bytes(self.take())
    }
}

/// Разбирает список из одних `GainMap`; любой другой ID или лишние байты — паника теста.
pub fn parse_gain_map_list(blob: &[u8]) -> Vec<GainMapOpcode> {
    let mut cursor = Cursor {
        bytes: blob,
        position: 0,
    };
    let count = cursor.u32();
    let mut opcodes = Vec::new();
    for _ in 0..count {
        let id = cursor.u32();
        assert_eq!(id, 9, "ожидался GainMap (ID 9)");
        let version = cursor.take::<4>();
        let flags = cursor.u32();
        let parameters_length = cursor.u32() as usize;
        let parameters_start = cursor.position;
        let mut opcode = GainMapOpcode {
            version,
            flags,
            top: cursor.u32(),
            left: cursor.u32(),
            bottom: cursor.u32(),
            right: cursor.u32(),
            plane: cursor.u32(),
            planes: cursor.u32(),
            row_pitch: cursor.u32(),
            col_pitch: cursor.u32(),
            points_vertical: cursor.u32(),
            points_horizontal: cursor.u32(),
            spacing_vertical: cursor.f64(),
            spacing_horizontal: cursor.f64(),
            origin_vertical: cursor.f64(),
            origin_horizontal: cursor.f64(),
            map_planes: cursor.u32(),
            gains: Vec::new(),
        };
        let gain_count = (opcode.points_vertical * opcode.points_horizontal * opcode.map_planes) as usize;
        opcode.gains = (0..gain_count).map(|_| cursor.f32()).collect();
        assert_eq!(
            cursor.position - parameters_start,
            parameters_length,
            "поле длины параметров не совпало с разобранным"
        );
        opcodes.push(opcode);
    }
    assert_eq!(cursor.position, blob.len(), "после последнего опкода остались байты");
    opcodes
}
