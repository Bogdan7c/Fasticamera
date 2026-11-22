//! Минимальный ЧИТАТЕЛЬ TIFF для тестов: разбирает то, что записал `photo_output::tiff`, и заодно
//! проверяет структурные правила TIFF 6.0 (поля по возрастанию тега, значения на чётных адресах,
//! смещения внутри файла). Panic — способ тестового бинаря сказать «файл испорчен».
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "тестовый читатель"
)]

use std::collections::BTreeMap;

/// Разобранное поле IFD.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEntry {
    pub tag: u16,
    pub type_code: u16,
    pub count: u32,
    /// Байты значения (из самого поля или по смещению), ровно `count * размер_типа`.
    pub bytes: Vec<u8>,
    /// Смещение значения, если оно лежало вне поля.
    pub out_of_line_offset: Option<u32>,
}

impl ParsedEntry {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn shorts(&self) -> Vec<u16> {
        assert_eq!(self.type_code, 3, "тег {} не SHORT", self.tag);
        self.bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
    }

    pub fn longs(&self) -> Vec<u32> {
        assert_eq!(self.type_code, 4, "тег {} не LONG", self.tag);
        self.bytes
            .chunks(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    pub fn ascii(&self) -> String {
        assert_eq!(self.type_code, 2, "тег {} не ASCII", self.tag);
        let (last, text) = self.bytes.split_last().expect("ASCII не пуст");
        assert_eq!(*last, 0, "ASCII должна заканчиваться NUL");
        String::from_utf8(text.to_vec()).expect("ASCII")
    }

    pub fn rationals(&self) -> Vec<(u32, u32)> {
        assert_eq!(self.type_code, 5, "тег {} не RATIONAL", self.tag);
        self.bytes
            .chunks(8)
            .map(|c| {
                (
                    u32::from_le_bytes(c[..4].try_into().unwrap()),
                    u32::from_le_bytes(c[4..].try_into().unwrap()),
                )
            })
            .collect()
    }

    pub fn rational_values(&self) -> Vec<f64> {
        self.rationals()
            .into_iter()
            .map(|(n, d)| f64::from(n) / f64::from(d))
            .collect()
    }

    pub fn srationals(&self) -> Vec<(i32, i32)> {
        assert_eq!(self.type_code, 10, "тег {} не SRATIONAL", self.tag);
        self.bytes
            .chunks(8)
            .map(|c| {
                (
                    i32::from_le_bytes(c[..4].try_into().unwrap()),
                    i32::from_le_bytes(c[4..].try_into().unwrap()),
                )
            })
            .collect()
    }

    pub fn srational_values(&self) -> Vec<f64> {
        self.srationals()
            .into_iter()
            .map(|(n, d)| f64::from(n) / f64::from(d))
            .collect()
    }

    pub fn doubles(&self) -> Vec<f64> {
        assert_eq!(self.type_code, 12, "тег {} не DOUBLE", self.tag);
        self.bytes
            .chunks(8)
            .map(|c| f64::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }
}

/// Разобранный каталог.
#[derive(Debug, Clone)]
pub struct ParsedIfd {
    pub offset: u32,
    pub entries: BTreeMap<u16, ParsedEntry>,
    pub next_offset: u32,
}

impl ParsedIfd {
    pub fn entry(&self, tag: u16) -> &ParsedEntry {
        self.entries.get(&tag).unwrap_or_else(|| panic!("нет тега {tag}"))
    }

    pub fn maybe(&self, tag: u16) -> Option<&ParsedEntry> {
        self.entries.get(&tag)
    }

    pub fn tags(&self) -> Vec<u16> {
        self.entries.keys().copied().collect()
    }
}

/// Разобранный файл.
pub struct ParsedTiff<'a> {
    pub bytes: &'a [u8],
}

fn type_size(type_code: u16) -> usize {
    match type_code {
        1 | 2 | 7 => 1,
        3 => 2,
        4 => 4,
        5 | 10 | 12 => 8,
        other => panic!("неизвестный тип поля {other}"),
    }
}

impl<'a> ParsedTiff<'a> {
    /// Проверяет заголовок `II*\0`.
    pub fn open(bytes: &'a [u8]) -> Self {
        assert_eq!(&bytes[..4], b"II*\0", "не TIFF little-endian");
        Self { bytes }
    }

    pub fn first_ifd(&self) -> ParsedIfd {
        self.ifd_at(u32::from_le_bytes(self.bytes[4..8].try_into().unwrap()))
    }

    /// Каталог, на который указывает LONG-поле (Exif IFD и подобные).
    pub fn sub_ifd(&self, entry: &ParsedEntry) -> ParsedIfd {
        self.ifd_at(entry.longs()[0])
    }

    pub fn next_ifd(&self, ifd: &ParsedIfd) -> Option<ParsedIfd> {
        (ifd.next_offset != 0).then(|| self.ifd_at(ifd.next_offset))
    }

    pub fn ifd_at(&self, offset: u32) -> ParsedIfd {
        assert_eq!(offset % 2, 0, "IFD на нечётном адресе {offset}");
        let start = offset as usize;
        let count = u16::from_le_bytes(self.bytes[start..start + 2].try_into().unwrap()) as usize;
        assert!(count > 0, "пустой IFD");
        let mut entries = BTreeMap::new();
        let mut previous_tag = None;
        for index in 0..count {
            let at = start + 2 + index * 12;
            let raw = &self.bytes[at..at + 12];
            let tag = u16::from_le_bytes([raw[0], raw[1]]);
            if let Some(previous) = previous_tag {
                assert!(tag > previous, "теги не по возрастанию: {previous} затем {tag}");
            }
            previous_tag = Some(tag);
            let type_code = u16::from_le_bytes([raw[2], raw[3]]);
            let value_count = u32::from_le_bytes(raw[4..8].try_into().unwrap());
            let byte_len = type_size(type_code) * value_count as usize;
            let (value_bytes, out_of_line_offset) = if byte_len <= 4 {
                (raw[8..8 + byte_len].to_vec(), None)
            } else {
                let value_offset = u32::from_le_bytes(raw[8..12].try_into().unwrap());
                assert_eq!(value_offset % 2, 0, "значение тега {tag} на нечётном адресе");
                let from = value_offset as usize;
                (self.bytes[from..from + byte_len].to_vec(), Some(value_offset))
            };
            entries.insert(
                tag,
                ParsedEntry {
                    tag,
                    type_code,
                    count: value_count,
                    bytes: value_bytes,
                    out_of_line_offset,
                },
            );
        }
        let next_at = start + 2 + count * 12;
        ParsedIfd {
            offset,
            entries,
            next_offset: u32::from_le_bytes(self.bytes[next_at..next_at + 4].try_into().unwrap()),
        }
    }

    /// Байты изображения, склеенные из полос по `StripOffsets`/`StripByteCounts`.
    pub fn strip_data(&self, ifd: &ParsedIfd) -> Vec<u8> {
        let offsets = ifd.entry(273).longs();
        let counts = ifd.entry(279).longs();
        assert_eq!(offsets.len(), counts.len());
        let mut data = Vec::new();
        for (offset, count) in offsets.into_iter().zip(counts) {
            assert_eq!(offset % 2, 0, "полоса на нечётном адресе");
            data.extend_from_slice(&self.bytes[offset as usize..(offset + count) as usize]);
        }
        data
    }
}
