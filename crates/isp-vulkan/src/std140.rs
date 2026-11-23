//! Последовательная запись строк uniform-блока std140 по 16 байт (общий помощник упаковщиков).
//!
//! Все блоки `isp-vulkan` состоят только из строк `vec4`/`uvec4`, поэтому std140 не вставляет дыр, а
//! смещение строки — `16 · номер`. Писатель не знает ни одного блока: числа и порядок задают упаковщики.

/// Писатель строк `vec4`/`uvec4`.
#[derive(Debug, Default)]
pub(crate) struct Std140Writer {
    bytes: Vec<u8>,
}

impl Std140Writer {
    /// Строка `vec4`.
    pub(crate) fn vec4(&mut self, values: [f32; 4]) {
        self.bytes.extend(values.iter().flat_map(|value| value.to_ne_bytes()));
    }

    /// Строка `uvec4`.
    pub(crate) fn uvec4(&mut self, values: [u32; 4]) {
        self.bytes.extend(values.iter().flat_map(|value| value.to_ne_bytes()));
    }

    /// Три строки `vec4` — матрица 3×3 по строкам (четвёртое число строки — 0).
    pub(crate) fn matrix3(&mut self, rows: [[f32; 3]; 3]) {
        for row in rows {
            self.vec4([row[0], row[1], row[2], 0.0]);
        }
    }

    /// Готовый блок ровно `BYTES` байт. Число строк фиксирует упаковщик; несовпадение — ошибка в нём,
    /// которую ловит тест раскладки упаковщика (в проде лишние байты не пишутся, недостающие — нули).
    pub(crate) fn finish<const BYTES: usize>(self) -> [u8; BYTES] {
        let mut block = [0_u8; BYTES];
        let length = self.bytes.len().min(BYTES);
        block[..length].copy_from_slice(&self.bytes[..length]);
        block
    }
}
