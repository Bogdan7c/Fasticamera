//! Вставка Exif в JPEG: место вставки, замена старого блока, сохранение прочих сегментов и сжатых данных.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "тесты")]

mod support;

use photo_output::jpeg_exif::{JpegExifError, insert_exif_app1};
use support::tiff_reader::ParsedTiff;

fn segment(marker: u8, data: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0xFF, marker];
    bytes.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
    bytes.extend_from_slice(data);
    bytes
}

fn jfif() -> Vec<u8> {
    segment(0xE0, b"JFIF\0\x01\x01\x00\x00\x01\x00\x01\x00\x00")
}

fn icc() -> Vec<u8> {
    segment(0xE2, b"ICC_PROFILE\0\x01\x01profile-bytes")
}

fn quantization_table() -> Vec<u8> {
    segment(0xDB, &[0x00; 65])
}

/// Сжатые данные со «стаффингом» `FF 00` и маркером перезапуска внутри: вставка не должна их трогать.
fn scan_and_end() -> Vec<u8> {
    let mut bytes = segment(0xDA, &[0x01, 0x01, 0x00, 0x00, 0x3F, 0x00]);
    bytes.extend_from_slice(&[0x12, 0xFF, 0x00, 0x34, 0xFF, 0xD0, 0x56]);
    bytes.extend_from_slice(&[0xFF, 0xD9]);
    bytes
}

fn jpeg_from(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = vec![0xFF, 0xD8];
    for part in parts {
        bytes.extend_from_slice(part);
    }
    bytes
}

fn exif_app1(tiff: &[u8]) -> Vec<u8> {
    let mut data = b"Exif\0\0".to_vec();
    data.extend_from_slice(tiff);
    segment(0xE1, &data)
}

/// Небольшой настоящий TIFF для вставки (из писателя crate).
fn sample_tiff() -> Vec<u8> {
    use photo_output::tiff::{Ifd, TiffFile, TiffValue};
    let mut ifd = Ifd::new();
    ifd.set(271, TiffValue::ascii("OnePlus"));
    let mut bytes = Vec::new();
    TiffFile::new(ifd).write(&mut bytes).unwrap();
    bytes
}

#[test]
fn exif_goes_after_leading_jfif_and_keeps_every_other_segment_and_the_scan() {
    let tiff = sample_tiff();
    let jpeg = jpeg_from(&[jfif(), icc(), quantization_table(), scan_and_end()]);
    let result = insert_exif_app1(&jpeg, &tiff).unwrap();
    assert_eq!(
        result,
        jpeg_from(&[jfif(), exif_app1(&tiff), icc(), quantization_table(), scan_and_end()])
    );
}

#[test]
fn without_jfif_exif_goes_right_after_soi() {
    let tiff = sample_tiff();
    let jpeg = jpeg_from(&[icc(), scan_and_end()]);
    let result = insert_exif_app1(&jpeg, &tiff).unwrap();
    assert_eq!(result, jpeg_from(&[exif_app1(&tiff), icc(), scan_and_end()]));
}

#[test]
fn inserted_block_is_a_readable_tiff_with_the_exif_signature() {
    let tiff = sample_tiff();
    let result = insert_exif_app1(&jpeg_from(&[jfif(), scan_and_end()]), &tiff).unwrap();
    // SOI(2) + JFIF(2+2+14) → APP1 начинается на смещении 20.
    assert_eq!(&result[20..22], &[0xFF, 0xE1]);
    let length = u16::from_be_bytes([result[22], result[23]]) as usize;
    assert_eq!(length, 2 + 6 + tiff.len());
    assert_eq!(&result[24..30], b"Exif\0\0");
    let parsed = ParsedTiff::open(&result[30..30 + tiff.len()]).first_ifd();
    assert_eq!(parsed.entry(271).ascii(), "OnePlus");
}

#[test]
fn previous_exif_is_replaced_but_xmp_in_another_app1_is_kept() {
    let old_tiff = {
        use photo_output::tiff::{Ifd, TiffFile, TiffValue};
        let mut ifd = Ifd::new();
        ifd.set(271, TiffValue::ascii("OldMake"));
        let mut bytes = Vec::new();
        TiffFile::new(ifd).write(&mut bytes).unwrap();
        bytes
    };
    let xmp = segment(0xE1, b"http://ns.adobe.com/xap/1.0/\0<x:xmpmeta/>");
    let new_tiff = sample_tiff();
    let jpeg = jpeg_from(&[jfif(), exif_app1(&old_tiff), xmp.clone(), icc(), scan_and_end()]);
    let result = insert_exif_app1(&jpeg, &new_tiff).unwrap();
    assert_eq!(
        result,
        jpeg_from(&[jfif(), exif_app1(&new_tiff), xmp, icc(), scan_and_end()])
    );
    assert!(
        !result.windows(7).any(|window| window == b"OldMake"),
        "старый Exif не должен остаться в файле"
    );
}

#[test]
fn inserting_twice_gives_the_same_file_as_inserting_once() {
    let tiff = sample_tiff();
    let once = insert_exif_app1(&jpeg_from(&[jfif(), icc(), scan_and_end()]), &tiff).unwrap();
    let twice = insert_exif_app1(&once, &tiff).unwrap();
    assert_eq!(once, twice);
}

#[test]
fn non_jpeg_and_broken_streams_are_rejected() {
    let tiff = sample_tiff();
    for broken in [
        Vec::new(),
        vec![0xFF],
        b"\x89PNG\r\n\x1a\n".to_vec(),
        vec![0xFF, 0xD8, 0x00, 0x01],             // после SOI нет маркера
        vec![0xFF, 0xD8, 0xFF],                   // оборван на маркере
        vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00],       // оборван в длине
        vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x01], // длина меньше 2
        vec![0xFF, 0xD8, 0xFF, 0xD0, 0xFF, 0xDA], // RSTn до SOS
        vec![0xFF, 0xD8, 0xFF, 0x01, 0xFF, 0xDA], // TEM до SOS
        vec![0xFF, 0xD8, 0xFF, 0xD8, 0xFF, 0xDA], // второй SOI
        {
            let mut cut = jpeg_from(&[jfif(), scan_and_end()]);
            cut.truncate(10); // сегмент выходит за конец потока
            cut
        },
    ] {
        let error = insert_exif_app1(&broken, &tiff).unwrap_err();
        assert!(matches!(error, JpegExifError::NotJpeg(_)), "{broken:?} → {error:?}");
    }
}

#[test]
fn exif_that_does_not_fit_app1_is_rejected_and_the_limit_is_exact() {
    let jpeg = jpeg_from(&[jfif(), scan_and_end()]);
    // Данные сегмента = 6 байт сигнатуры + TIFF; предел 65533.
    let fits = vec![0u8; 65_533 - 6];
    let result = insert_exif_app1(&jpeg, &fits).unwrap();
    assert_eq!(result.len(), jpeg.len() + 4 + 65_533);

    let too_big = vec![0u8; 65_533 - 6 + 1];
    assert_eq!(
        insert_exif_app1(&jpeg, &too_big).unwrap_err(),
        JpegExifError::ExifTooLarge(65_534)
    );
}

#[test]
fn empty_exif_block_is_rejected_instead_of_writing_a_hollow_segment() {
    let jpeg = jpeg_from(&[jfif(), scan_and_end()]);
    assert_eq!(insert_exif_app1(&jpeg, &[]).unwrap_err(), JpegExifError::EmptyExif);
}
