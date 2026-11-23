use isp_model::look::lut::LookLut;

use super::*;

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

fn f32_at(bytes: &[u8], at: usize) -> f32 {
    f32::from_ne_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

#[test]
fn absent_lut_packs_a_zero_size_header_only() {
    let bytes = pack_look_lut(None, LookLutLimits::default()).expect("заглушка всегда годится");
    assert_eq!(bytes.len(), HEADER_BYTES);
    assert_eq!(u32_at(&bytes, 0), 0, "размер 0 — таблицы нет");
}

#[test]
fn lut_packs_size_and_nodes_in_red_fastest_order() {
    // Таблица с узлами, по которым видно порядок: значение узла (r, g, b) = (r, g, b) / 2.
    let lut = LookLut::identity(3, 65).expect("таблица 3³");
    let bytes = pack_look_lut(Some(&lut), LookLutLimits::default()).expect("таблица помещается");
    assert_eq!(bytes.len(), HEADER_BYTES + 27 * NODE_BYTES);
    assert_eq!(u32_at(&bytes, 0), 3);
    // Узел (r = 1, g = 0, b = 2) лежит под номером 1 + 3·(0 + 3·2) = 19 → байты 16 + 19·12.
    let at = HEADER_BYTES + 19 * NODE_BYTES;
    assert_eq!(
        [f32_at(&bytes, at), f32_at(&bytes, at + 4), f32_at(&bytes, at + 8)],
        [0.5, 0.0, 1.0]
    );
    // Последний узел — белый.
    let last = HEADER_BYTES + 26 * NODE_BYTES;
    assert_eq!(
        [f32_at(&bytes, last), f32_at(&bytes, last + 4), f32_at(&bytes, last + 8)],
        [1.0; 3]
    );
}

#[test]
fn lut_larger_than_the_limit_is_an_explicit_error_not_a_silent_skip() {
    let lut = LookLut::identity(9, 65).expect("таблица 9³");
    let limits = LookLutLimits { max_size: 8 };
    assert_eq!(
        pack_look_lut(Some(&lut), limits),
        Err(LookLutError::TooLarge { size: 9, max: 8 })
    );
    // Ровно по лимиту — помещается.
    assert!(pack_look_lut(Some(&lut), LookLutLimits { max_size: 9 }).is_ok());
}

#[test]
fn limits_are_validated_and_size_the_buffer() {
    assert!(LookLutLimits { max_size: 1 }.validate().is_err());
    assert!(
        LookLutLimits {
            max_size: MAX_LUT_SIZE_LIMIT + 1
        }
        .validate()
        .is_err()
    );
    assert!(LookLutLimits::default().validate().is_ok());
    assert_eq!(
        LookLutLimits { max_size: 17 }.buffer_bytes(),
        16 + 17 * 17 * 17 * 12,
        "заголовок и 12 байт на узел"
    );
    assert_eq!(
        LookLutLimits::default().max_size,
        LookModelSettings::default().max_lut_size,
        "предел буфера и предел модели вида совпадают"
    );
}
