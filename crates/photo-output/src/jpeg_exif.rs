//! Вставка блока Exif (APP1) в поток JPEG от платформы (`Bitmap.compress` Exif не пишет).
//!
//! Формат JPEG (ITU T.81, раздел B.1): `SOI (FF D8)`, затем сегменты `FF <маркер> <длина u16 big-endian,
//! включая эти 2 байта> <данные>`, затем `SOS (FF DA)` и сжатые данные до `EOI`. Мы разбираем только
//! сегменты ДО `SOS`: всё от `SOS` и дальше копируется как есть, поэтому сжатые данные не трогаются.
//!
//! Где лежит Exif. Exif 2.32 (CIPA DC-008, 4.5.4) требует APP1 сразу за `SOI`, а JFIF 1.02 требует APP0
//! `JFIF` сразу за `SOI`; оба условия сразу невыполнимы. Мы ставим APP1 ПОСЛЕ APP0 JFIF, если он первый:
//! так делают exiftool и libexif, а читатели Exif (Android `ExifInterface`, Photos, exiftool) ищут APP1
//! перебором сегментов. Если APP0 JFIF нет — APP1 идёт прямо за `SOI`.

/// Почему Exif не вставлен.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JpegExifError {
    /// Поток не начинается с SOI (`FF D8`) или маркеры испорчены.
    #[error("это не JPEG: {0}")]
    NotJpeg(String),
    /// Блок Exif больше предела сегмента APP1: длина сегмента — `u16`, на данные остаётся 65533 байта,
    /// из них 6 занимает сигнатура `Exif\0\0`. Число — размер данных сегмента (сигнатура + TIFF).
    #[error("блок Exif {0} байт не помещается в APP1")]
    ExifTooLarge(usize),
    /// Блок Exif пуст: вставлять нечего (так выглядит неудачная сборка блока).
    #[error("блок Exif пуст")]
    EmptyExif,
}

const MARKER_PREFIX: u8 = 0xFF;
const MARKER_SOI: u8 = 0xD8;
const MARKER_EOI: u8 = 0xD9;
const MARKER_SOS: u8 = 0xDA;
const MARKER_APP0: u8 = 0xE0;
const MARKER_APP1: u8 = 0xE1;
/// Сигнатура данных APP1 с Exif (Exif 2.32, 4.7.2).
const EXIF_SIGNATURE: &[u8; 6] = b"Exif\0\0";
/// Сигнатура данных APP0 с JFIF (JFIF 1.02).
const JFIF_SIGNATURE: &[u8; 5] = b"JFIF\0";
/// Предел данных одного сегмента: `u16::MAX` минус 2 байта самого поля длины.
const MAX_SEGMENT_DATA: usize = 65_533;

/// Сегмент JPEG до `SOS`: границы в исходном потоке и разбор.
struct Segment {
    /// Включая `FF <маркер>` и поле длины.
    start: usize,
    end: usize,
    marker: u8,
}

impl Segment {
    /// Данные сегмента (после поля длины).
    fn data<'a>(&self, jpeg: &'a [u8]) -> &'a [u8] {
        &jpeg[self.start + 4..self.end]
    }
}

/// Возвращает JPEG с блоком Exif (`exif_tiff` — поток TIFF без заголовка `Exif\0\0`) сразу после SOI
/// (и после APP0 JFIF, если он первый). Прежний APP1 Exif, если был, заменяется; остальные сегменты
/// (JFIF, ICC в APP2, XMP в другом APP1) сохраняются в прежнем порядке.
pub fn insert_exif_app1(jpeg: &[u8], exif_tiff: &[u8]) -> Result<Vec<u8>, JpegExifError> {
    if exif_tiff.is_empty() {
        return Err(JpegExifError::EmptyExif);
    }
    let segment_data_len = EXIF_SIGNATURE.len() + exif_tiff.len();
    if segment_data_len > MAX_SEGMENT_DATA {
        return Err(JpegExifError::ExifTooLarge(segment_data_len));
    }
    let (segments, entropy_start) = split_header_segments(jpeg)?;

    let mut output = Vec::with_capacity(jpeg.len() + segment_data_len + 4);
    output.extend_from_slice(&jpeg[..2]); // SOI
    let mut exif_written = false;
    for (index, segment) in segments.iter().enumerate() {
        let is_old_exif = segment.marker == MARKER_APP1 && segment.data(jpeg).starts_with(EXIF_SIGNATURE);
        if is_old_exif {
            continue;
        }
        // APP1 ставим перед первым сегментом, который не является ведущим JFIF.
        let is_leading_jfif =
            index == 0 && segment.marker == MARKER_APP0 && segment.data(jpeg).starts_with(JFIF_SIGNATURE);
        if !exif_written && !is_leading_jfif {
            push_exif_segment(&mut output, exif_tiff, segment_data_len);
            exif_written = true;
        }
        output.extend_from_slice(&jpeg[segment.start..segment.end]);
    }
    if !exif_written {
        push_exif_segment(&mut output, exif_tiff, segment_data_len);
    }
    output.extend_from_slice(&jpeg[entropy_start..]);
    Ok(output)
}

fn push_exif_segment(output: &mut Vec<u8>, exif_tiff: &[u8], segment_data_len: usize) {
    // Длина проверена выше (≤ 65533), поэтому +2 влезает в u16.
    let length_field = u16::try_from(segment_data_len + 2).unwrap_or(u16::MAX);
    output.extend_from_slice(&[MARKER_PREFIX, MARKER_APP1]);
    output.extend_from_slice(&length_field.to_be_bytes());
    output.extend_from_slice(EXIF_SIGNATURE);
    output.extend_from_slice(exif_tiff);
}

/// Делит заголовок JPEG на сегменты до `SOS`; возвращает их и смещение, с которого начинается остаток
/// потока (`SOS` с его данными или, у JPEG без сжатых данных, `EOI`).
fn split_header_segments(jpeg: &[u8]) -> Result<(Vec<Segment>, usize), JpegExifError> {
    if jpeg.len() < 2 || jpeg[0] != MARKER_PREFIX || jpeg[1] != MARKER_SOI {
        return Err(JpegExifError::NotJpeg("нет маркера SOI в начале".to_owned()));
    }
    let mut segments = Vec::new();
    let mut position = 2;
    loop {
        let marker_start = position;
        if jpeg.get(position) != Some(&MARKER_PREFIX) {
            return Err(JpegExifError::NotJpeg(format!("на смещении {position} нет маркера")));
        }
        // Перед маркером допускаются добивочные байты FF (T.81, B.1.1.2).
        while jpeg.get(position + 1) == Some(&MARKER_PREFIX) {
            position += 1;
        }
        let marker = *jpeg
            .get(position + 1)
            .ok_or_else(|| JpegExifError::NotJpeg("поток оборван на маркере".to_owned()))?;
        if marker == MARKER_SOS || marker == MARKER_EOI {
            return Ok((segments, marker_start));
        }
        // Маркеры без длины (SOI, TEM 01, RSTn D0–D7) до SOS встречаться не должны (T.81, B.2.1): поток
        // испорчен, и молча сдвигать позицию (теряя маркер) мы не станем.
        if marker == MARKER_SOI || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            return Err(JpegExifError::NotJpeg(format!(
                "маркер {marker:#04X} без длины до SOS на смещении {position}"
            )));
        }
        let length_bytes = jpeg
            .get(position + 2..position + 4)
            .ok_or_else(|| JpegExifError::NotJpeg("поток оборван в длине сегмента".to_owned()))?;
        let length = usize::from(u16::from_be_bytes([length_bytes[0], length_bytes[1]]));
        if length < 2 {
            return Err(JpegExifError::NotJpeg(format!("длина сегмента {length} меньше 2")));
        }
        let end = position + 2 + length;
        if end > jpeg.len() {
            return Err(JpegExifError::NotJpeg(format!(
                "сегмент {marker:#04X} выходит за конец потока"
            )));
        }
        segments.push(Segment {
            start: position,
            end,
            marker,
        });
        position = end;
    }
}
