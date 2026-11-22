//! Писатель TIFF: раскладка файла проверяется читателем из `tests/support` (запись → чтение).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "тесты")]

mod support;

use photo_output::tiff::{ByteOrder, Ifd, TiffError, TiffFile, TiffValue, signed_rational, unsigned_rational};
use support::tiff_reader::{Order, ParsedTiff};

/// Оба порядка байт писателя и что читатель обязан увидеть в заголовке.
const BOTH_ORDERS: [(ByteOrder, Order); 2] = [
    (ByteOrder::LittleEndian, Order::Little),
    (ByteOrder::BigEndian, Order::Big),
];

fn write(file: &TiffFile<'_>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let written = file.write(&mut bytes).unwrap();
    assert_eq!(written, bytes.len() as u64, "возвращённая длина = записанным байтам");
    bytes
}

#[test]
fn every_value_type_survives_a_round_trip_inline_and_out_of_line() {
    for (byte_order, expected_order) in BOTH_ORDERS {
        every_value_type_round_trip(byte_order, expected_order);
    }
}

fn every_value_type_round_trip(byte_order: ByteOrder, expected_order: Order) {
    let mut ifd = Ifd::new();
    // Порядок вставки нарочно не по возрастанию тегов.
    ifd.set(500, TiffValue::Double(vec![1.5, -2.25]));
    ifd.set(300, TiffValue::Rational(vec![(1, 3)]));
    ifd.set(100, TiffValue::short(7)); // 2 байта — в поле
    ifd.set(101, TiffValue::Short(vec![1, 2])); // ровно 4 байта — ещё в поле
    ifd.set(102, TiffValue::Short(vec![1, 2, 3])); // 6 байт — по смещению
    ifd.set(200, TiffValue::ascii("Fasticamera")); // 12 байт с NUL — по смещению
    ifd.set(201, TiffValue::ascii("ab")); // 3 байта с NUL — в поле
    ifd.set(400, TiffValue::SRational(vec![(-1, 2), (3, -4)]));
    ifd.set(401, TiffValue::Byte(vec![9, 8, 7, 6, 5]));
    ifd.set(402, TiffValue::Undefined(b"0232".to_vec()));
    ifd.set(403, TiffValue::Long(vec![u32::MAX, 0]));

    let bytes = write(&TiffFile::with_byte_order(ifd, byte_order));
    let tiff = ParsedTiff::open(&bytes);
    assert_eq!(tiff.order, expected_order, "заголовок объявляет выбранный порядок");
    let parsed = tiff.first_ifd();

    assert_eq!(
        parsed.tags(),
        vec![100, 101, 102, 200, 201, 300, 400, 401, 402, 403, 500]
    );
    assert_eq!(parsed.entry(100).shorts(), vec![7]);
    assert_eq!(parsed.entry(100).out_of_line_offset, None);
    assert_eq!(parsed.entry(101).shorts(), vec![1, 2]);
    assert_eq!(parsed.entry(101).out_of_line_offset, None);
    assert_eq!(parsed.entry(102).shorts(), vec![1, 2, 3]);
    assert!(parsed.entry(102).out_of_line_offset.is_some());
    assert_eq!(parsed.entry(200).ascii(), "Fasticamera");
    assert_eq!(parsed.entry(201).ascii(), "ab");
    assert_eq!(parsed.entry(300).rationals(), vec![(1, 3)]);
    assert_eq!(parsed.entry(400).srationals(), vec![(-1, 2), (3, -4)]);
    assert_eq!(parsed.entry(401).bytes(), &[9, 8, 7, 6, 5]);
    assert_eq!(parsed.entry(402).bytes(), b"0232");
    assert_eq!(parsed.entry(403).longs(), vec![u32::MAX, 0]);
    assert_eq!(parsed.entry(500).doubles(), vec![1.5, -2.25]);
    assert_eq!(parsed.next_offset, 0, "следующего IFD нет");
}

#[test]
fn out_of_line_values_are_word_aligned_even_after_odd_sized_values() {
    let mut ifd = Ifd::new();
    ifd.set(10, TiffValue::Byte(vec![1; 5])); // 5 байт — нечётная длина по смещению
    ifd.set(11, TiffValue::Byte(vec![2; 7]));
    ifd.set(12, TiffValue::Rational(vec![(5, 6)]));
    let bytes = write(&TiffFile::new(ifd));
    let parsed = ParsedTiff::open(&bytes).first_ifd();
    for tag in [10, 11, 12] {
        let offset = parsed.entry(tag).out_of_line_offset.expect("значение вне поля");
        assert_eq!(offset % 2, 0, "тег {tag} на нечётном адресе");
    }
    assert_eq!(parsed.entry(10).bytes(), &[1; 5]);
    assert_eq!(parsed.entry(11).bytes(), &[2; 7]);
    assert_eq!(parsed.entry(12).rationals(), vec![(5, 6)]);
}

#[test]
fn nested_ifd_sub_ifds_and_next_ifd_are_reachable_from_their_pointers() {
    for (byte_order, _) in BOTH_ORDERS {
        nested_ifds_round_trip(byte_order);
    }
}

fn nested_ifds_round_trip(byte_order: ByteOrder) {
    let mut exif = Ifd::new();
    exif.set(33434, TiffValue::Rational(vec![(1, 100)]));
    let mut sub_a = Ifd::new();
    sub_a.set(256, TiffValue::long(111));
    let mut sub_b = Ifd::new();
    sub_b.set(256, TiffValue::long(222));
    let mut second = Ifd::new();
    second.set(256, TiffValue::long(333));

    let mut root = Ifd::new();
    root.set(34665, TiffValue::ifd(exif));
    root.set(330, TiffValue::SubIfds(vec![sub_a, sub_b]));
    root.set(256, TiffValue::long(1));
    root.set_next(second);

    let bytes = write(&TiffFile::with_byte_order(root, byte_order));
    let tiff = ParsedTiff::open(&bytes);
    let first = tiff.first_ifd();

    assert_eq!(first.entry(256).longs(), vec![1]);
    let exif_parsed = tiff.sub_ifd(first.entry(34665));
    assert_eq!(exif_parsed.entry(33434).rationals(), vec![(1, 100)]);
    let sub_offsets = first.entry(330).longs();
    assert_eq!(sub_offsets.len(), 2);
    assert_eq!(tiff.ifd_at(sub_offsets[0]).entry(256).longs(), vec![111]);
    assert_eq!(tiff.ifd_at(sub_offsets[1]).entry(256).longs(), vec![222]);
    let next = tiff.next_ifd(&first).expect("есть следующий IFD");
    assert_eq!(next.entry(256).longs(), vec![333]);
    assert!(tiff.next_ifd(&next).is_none());
}

#[test]
fn strips_are_streamed_after_metadata_with_correct_offsets_even_for_odd_lengths() {
    let strip_a = vec![0xA1u8; 5]; // нечётная длина: писатель добьёт до чётного адреса
    let strip_b = vec![0xB2u8; 8];
    let strip_c = vec![0xC3u8; 3];
    let mut root = Ifd::new();
    root.set(256, TiffValue::long(4));
    let mut file = TiffFile::new(root);
    file.push_strip(&strip_a);
    file.push_strip(&strip_b);
    file.push_strip(&strip_c);
    let bytes = write(&file);

    let tiff = ParsedTiff::open(&bytes);
    let parsed = tiff.first_ifd();
    assert_eq!(parsed.entry(279).longs(), vec![5, 8, 3]);
    let offsets = parsed.entry(273).longs();
    assert_eq!(&bytes[offsets[0] as usize..][..5], &strip_a[..]);
    assert_eq!(&bytes[offsets[1] as usize..][..8], &strip_b[..]);
    assert_eq!(&bytes[offsets[2] as usize..][..3], &strip_c[..]);
    assert!(offsets.iter().all(|offset| offset % 2 == 0));
    assert_eq!(tiff.strip_data(&parsed), [strip_a, strip_b, strip_c].concat());
}

#[test]
fn default_file_is_little_endian_like_dng() {
    let bytes = write(&TiffFile::new(Ifd::new().set(256, TiffValue::long(1)).clone()));
    assert_eq!(&bytes[..4], b"II*\0");
}

#[test]
fn big_endian_file_stores_every_number_most_significant_byte_first() {
    // Сырые байты, а не круговой тест: читатель с той же ошибкой порядка не заметил бы её.
    let mut ifd = Ifd::new();
    ifd.set(0x0102, TiffValue::long(0x0A0B_0C0D));
    let bytes = write(&TiffFile::with_byte_order(ifd, ByteOrder::BigEndian));

    assert_eq!(&bytes[..4], b"MM\0*", "заголовок MM и число 42 старшим байтом вперёд");
    assert_eq!(&bytes[4..8], &[0, 0, 0, 8], "первый IFD сразу после заголовка");
    assert_eq!(&bytes[8..10], &[0, 1], "в IFD одно поле");
    assert_eq!(&bytes[10..12], &[0x01, 0x02], "тег");
    assert_eq!(&bytes[12..14], &[0, 4], "тип LONG");
    assert_eq!(&bytes[14..18], &[0, 0, 0, 1], "счётчик");
    assert_eq!(&bytes[18..22], &[0x0A, 0x0B, 0x0C, 0x0D], "значение в поле");
    assert_eq!(&bytes[22..26], &[0, 0, 0, 0], "следующего IFD нет");
}

#[test]
fn single_strip_offset_lives_inside_the_field() {
    let strip = vec![1u8, 2, 3, 4];
    let mut file = TiffFile::new(Ifd::new().set(256, TiffValue::long(2)).clone());
    file.push_strip(&strip);
    let bytes = write(&file);
    let parsed = ParsedTiff::open(&bytes).first_ifd();
    assert_eq!(parsed.entry(273).out_of_line_offset, None);
    assert_eq!(parsed.entry(273).count, 1);
}

#[test]
fn setting_a_tag_twice_keeps_one_field_with_the_last_value() {
    let mut ifd = Ifd::new();
    ifd.set(256, TiffValue::long(1));
    ifd.set(256, TiffValue::long(2));
    let bytes = write(&TiffFile::new(ifd));
    let parsed = ParsedTiff::open(&bytes).first_ifd();
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entry(256).longs(), vec![2]);
}

#[test]
fn invalid_input_is_reported_not_written() {
    let mut out = Vec::new();

    let mut empty_value = Ifd::new();
    empty_value.set(256, TiffValue::Short(vec![]));
    let error = TiffFile::new(empty_value).write(&mut out).unwrap_err();
    assert!(matches!(error, TiffError::EmptyValue(256)), "{error:?}");

    let mut non_ascii = Ifd::new();
    non_ascii.set(271, TiffValue::ascii("Ёлка"));
    let error = TiffFile::new(non_ascii).write(&mut out).unwrap_err();
    assert!(matches!(error, TiffError::InvalidAscii(271)), "{error:?}");

    let error = TiffFile::new(Ifd::new()).write(&mut out).unwrap_err();
    assert!(matches!(error, TiffError::BadEntryCount(0)), "{error:?}");

    let mut reserved = Ifd::new();
    reserved.set(273, TiffValue::long(0));
    let error = TiffFile::new(reserved).write(&mut out).unwrap_err();
    assert!(matches!(error, TiffError::ReservedTag(273)), "{error:?}");
    assert!(out.is_empty(), "при ошибке разметки в приёмник ничего не пишется");
}

#[test]
fn sink_failure_is_surfaced_as_io_error() {
    struct FailingSink;
    impl std::io::Write for FailingSink {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("диск полон"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut ifd = Ifd::new();
    ifd.set(256, TiffValue::long(1));
    let error = TiffFile::new(ifd).write(&mut FailingSink).unwrap_err();
    assert!(matches!(error, TiffError::Io(_)), "{error:?}");
}

#[test]
fn rational_helpers_round_and_reject_unrepresentable_numbers() {
    assert_eq!(unsigned_rational(1.8, 1000), Some((1800, 1000)));
    assert_eq!(unsigned_rational(-0.1, 1000), None);
    assert_eq!(unsigned_rational(f64::NAN, 1000), None);
    assert_eq!(unsigned_rational(5e9, 1000), None);
    assert_eq!(signed_rational(-0.25, 1_000_000), Some((-250_000, 1_000_000)));
    assert_eq!(signed_rational(f64::INFINITY, 1000), None);
    assert_eq!(signed_rational(1e9, 1_000_000), None);
}
