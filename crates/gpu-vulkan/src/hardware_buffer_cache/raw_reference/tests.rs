use super::*;

fn raw16_geometry(width: u32, height: u32, padding_bytes: u64) -> RowGeometry {
    RowGeometry {
        rows: height,
        row_valid_bytes: u64::from(width) * 2,
        row_stride_bytes: u64::from(width) * 2 + padding_bytes,
    }
}

fn filled(format: RawPixelFormat, width: u32, geometry: &RowGeometry) -> Vec<u8> {
    let _ = width;
    let mut bytes = vec![0_u8; (geometry.row_stride_bytes * u64::from(geometry.rows)) as usize];
    assert!(fill_pattern(&mut bytes, format, geometry));
    bytes
}

#[test]
fn format_codes_and_row_bytes_match_phone_facts() {
    assert_eq!(RawPixelFormat::Raw16.ahb_format_code(), 0x20);
    assert_eq!(RawPixelFormat::Raw10.ahb_format_code(), 0x25);
    assert_eq!(RawPixelFormat::Raw16.valid_row_bytes(4096), 8192);
    assert_eq!(RawPixelFormat::Raw10.valid_row_bytes(4096), 5120);
}

#[test]
fn raw16_stride_is_pixels_times_two() {
    assert_eq!(
        resolve_row_stride_bytes(RawPixelFormat::Raw16, 4096, 3072, 4096, 25_165_824),
        Ok(8192)
    );
}

#[test]
fn raw10_stride_in_bytes_matches_phone_allocation() {
    // Реальные числа OnePlus 11: RAW10 4096×3072, AHB stride 5120, аллокация 15 728 640.
    assert_eq!(
        resolve_row_stride_bytes(RawPixelFormat::Raw10, 4096, 3072, 5120, 15_728_640),
        Ok(5120)
    );
}

#[test]
fn raw10_stride_falls_back_to_pixels_when_bytes_cannot_hold_the_row() {
    // stride=100 как «байты» не вмещает 100 пикселей RAW10 (нужно 125) → это пиксели: 125 байт.
    assert_eq!(
        resolve_row_stride_bytes(RawPixelFormat::Raw10, 100, 2, 100, 250),
        Ok(125)
    );
}

#[test]
fn raw10_stride_inconsistent_with_allocation_is_an_error_not_a_guess() {
    assert_eq!(
        resolve_row_stride_bytes(RawPixelFormat::Raw10, 4096, 3072, 5120, 1000),
        Err(StrideError::Inconsistent {
            ahb_stride: 5120,
            allocation_size: 1000
        })
    );
}

#[test]
fn required_bytes_leaves_last_row_without_padding() {
    assert_eq!(raw16_geometry(4, 3, 8).required_bytes(), 2 * 16 + 8);
    assert_eq!(raw16_geometry(4, 0, 8).required_bytes(), 0);
}

#[test]
fn pattern_fills_valid_part_by_formula_and_padding_with_sentinel() {
    let geometry = raw16_geometry(5, 3, 6);
    let bytes = filled(RawPixelFormat::Raw16, 5, &geometry);
    for row in 0..3_u32 {
        for column in 0..5_u32 {
            let offset = (u64::from(row) * geometry.row_stride_bytes) as usize + column as usize * 2;
            assert_eq!(
                u16::from_le_bytes([bytes[offset], bytes[offset + 1]]),
                raw16_pattern_value(column, row)
            );
        }
        let padding_start = (u64::from(row) * geometry.row_stride_bytes + geometry.row_valid_bytes) as usize;
        assert!(
            bytes[padding_start..padding_start + 6]
                .iter()
                .all(|byte| *byte == PADDING_SENTINEL)
        );
    }
}

#[test]
fn pattern_refuses_a_buffer_shorter_than_geometry() {
    let geometry = raw16_geometry(5, 3, 6);
    let mut too_short = vec![0_u8; (geometry.required_bytes() - 1) as usize];
    assert!(!fill_pattern(&mut too_short, RawPixelFormat::Raw16, &geometry));
}

#[test]
fn pattern_values_are_not_degenerate() {
    // Образец должен различать позиции, иначе проверка «GPU видит те же байты» ничего не докажет.
    let values: std::collections::HashSet<u16> = (0..16)
        .flat_map(|row| (0..16).map(move |column| raw16_pattern_value(column, row)))
        .collect();
    assert!(values.len() > 200, "всего {} разных значений из 256", values.len());
}

#[test]
fn checksum_matches_hand_computed_vector() {
    // Два текселя 1 и 2: sum = 3; mix = (rotl(1,0) ^ 0) ^ (rotl(2,1) ^ 1*0x9E3779B1) = 1 ^ 4 ^ 0x9E3779B1.
    let geometry = raw16_geometry(2, 1, 0);
    let bytes = [1_u8, 0, 2, 0];
    let checksum = checksum_u16_texels(&bytes, 2, &geometry).expect("геометрия вмещается");
    assert_eq!(
        checksum,
        ChecksumPair {
            sum: 3,
            mix: 1 ^ 4 ^ 0x9E37_79B1
        }
    );
}

#[test]
fn texel_checksum_equals_independent_formula_over_the_pattern() {
    let (width, height) = (37_u32, 11_u32);
    let geometry = raw16_geometry(width, height, 10);
    let bytes = filled(RawPixelFormat::Raw16, width, &geometry);
    let mut expected_sum = 0_u32;
    for row in 0..height {
        for column in 0..width {
            expected_sum = expected_sum.wrapping_add(u32::from(raw16_pattern_value(column, row)));
        }
    }
    let checksum = checksum_u16_texels(&bytes, width, &geometry).expect("геометрия вмещается");
    assert_eq!(checksum.sum, expected_sum);
}

#[test]
fn texel_checksum_ignores_padding_but_sees_every_valid_byte() {
    let (width, height) = (8_u32, 4_u32);
    let geometry = raw16_geometry(width, height, 6);
    let bytes = filled(RawPixelFormat::Raw16, width, &geometry);
    let baseline = checksum_u16_texels(&bytes, width, &geometry).expect("геометрия вмещается");

    let mut padding_changed = bytes.clone();
    padding_changed[geometry.row_valid_bytes as usize] ^= 0xFF; // первый байт паддинга строки 0
    assert_eq!(checksum_u16_texels(&padding_changed, width, &geometry), Ok(baseline));

    let mut texel_changed = bytes.clone();
    texel_changed[3] ^= 0x01;
    let changed = checksum_u16_texels(&texel_changed, width, &geometry).expect("геометрия вмещается");
    assert_ne!(changed.sum, baseline.sum);
    assert_ne!(changed.mix, baseline.mix);
}

#[test]
fn mix_detects_swapped_texels_that_the_plain_sum_cannot() {
    let geometry = raw16_geometry(4, 1, 0);
    let original = [1_u8, 0, 2, 0, 3, 0, 4, 0];
    let swapped = [2_u8, 0, 1, 0, 3, 0, 4, 0];
    let first = checksum_u16_texels(&original, 4, &geometry).expect("вмещается");
    let second = checksum_u16_texels(&swapped, 4, &geometry).expect("вмещается");
    assert_eq!(first.sum, second.sum);
    assert_ne!(first.mix, second.mix);
}

#[test]
fn sum_wraps_modulo_2_pow_32() {
    let geometry = RowGeometry {
        rows: 1,
        row_valid_bytes: 8,
        row_stride_bytes: 8,
    };
    let bytes = [0xFF_u8; 8]; // два слова 0xFFFFFFFF
    let checksum = checksum_strided_words(&bytes, &geometry).expect("вмещается");
    assert_eq!(checksum.sum, 0xFFFF_FFFE);
}

#[test]
fn word_checksum_skips_padding_and_partial_tail_word() {
    // 2 строки, stride 12, полезных 10 байт → 2 целых слова на строку, 2 байта хвоста не учитываются.
    let geometry = RowGeometry {
        rows: 2,
        row_valid_bytes: 10,
        row_stride_bytes: 12,
    };
    let mut bytes = Vec::new();
    for (first, second) in [(1_u32, 2_u32), (30, 40)] {
        bytes.extend(first.to_le_bytes());
        bytes.extend(second.to_le_bytes());
        bytes.extend([0xEE; 4]);
    }
    assert_eq!(checksum_strided_words(&bytes, &geometry).expect("вмещается").sum, 73);
}

#[test]
fn packed_raw10_pattern_checksum_depends_on_stride_layout() {
    let (width, height) = (16_u32, 6_u32);
    let valid = RawPixelFormat::Raw10.valid_row_bytes(width);
    let geometry = RowGeometry {
        rows: height,
        row_valid_bytes: valid,
        row_stride_bytes: valid + 12,
    };
    let bytes = filled(RawPixelFormat::Raw10, width, &geometry);
    let correct = checksum_strided_words(&bytes, &geometry).expect("вмещается");
    // Ошибочный шаг строки (как если бы забыли паддинг) даёт другой результат.
    let wrong_stride = RowGeometry {
        row_stride_bytes: valid,
        ..geometry
    };
    let wrong = checksum_strided_words(&bytes, &wrong_stride).expect("вмещается");
    assert_ne!(correct, wrong);
}

#[test]
fn short_buffer_is_reported_with_sizes() {
    let geometry = raw16_geometry(4, 2, 0);
    assert_eq!(
        checksum_u16_texels(&[0; 15], 4, &geometry),
        Err(BufferTooSmall {
            required: 16,
            actual: 15
        })
    );
    assert_eq!(
        checksum_strided_words(&[0; 15], &geometry),
        Err(BufferTooSmall {
            required: 16,
            actual: 15
        })
    );
}
