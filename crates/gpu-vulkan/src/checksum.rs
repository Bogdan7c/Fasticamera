//! Контрольная сумма FNV-1a 64 для файлов, которые мы пишем сами (кэш пайплайнов).
//!
//! Тот же алгоритм, что в дайджесте стенда насыщения (`saturation::digest`), но отдельный маленький модуль: файл кэша не
//! должен зависеть от стенда. Сумма НЕ криптографическая: она нужна, чтобы заметить случайную порчу (обрезок, перевёрнутый
//! бит), а не подделку.

/// Начальное значение FNV-1a 64.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
/// Простое число FNV-1a 64.
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a 64 байт: побайтово «xor, затем умножение».
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(FNV_OFFSET_BASIS, |state, byte| {
        (state ^ u64::from(*byte)).wrapping_mul(FNV_PRIME)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_the_published_fnv1a_64_test_vectors() {
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
        assert_eq!(fnv1a64(b"foobar"), 0x8594_4171_f739_67e8);
    }
}
