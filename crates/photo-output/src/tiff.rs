//! Минимальный писатель TIFF 6.0 (Adobe, 1992) в любом из двух порядков байт TIFF ([`ByteOrder`]).
//!
//! Нужен трём потребителям: DNG (`crate::dng`), блок Exif для HEIF/JPEG (`crate::exif`) и их тестам.
//! Порядок байт выбирает потребитель: DNG — little-endian `II` (отсчёты кадра уже лежат в нём, как у
//! `DngCreator`), Exif — big-endian `MM` (причина — в `crate::exif`).
//! Писатель умеет ровно то, что нужно им, и ничего лишнего:
//! * IFD с полями, отсортированными по возрастанию тега (TIFF 6.0, раздел 2: «Image File Directory»);
//! * типы BYTE / ASCII / SHORT / LONG / RATIONAL / UNDEFINED / SRATIONAL / DOUBLE;
//! * значение до 4 байт лежит прямо в поле, длиннее — по смещению (на чётном адресе, раздел 2);
//! * вложенные IFD (Exif IFD, указатель-LONG) и массив `SubIFDs`;
//! * цепочка «следующий IFD»;
//! * полосы изображения (strips): их байты НЕ копируются — писатель принимает срезы и пишет их
//!   потоком после метаданных (DNG ≈ 25 МБ в память целиком не собирается).
//!
//! Раскладка файла: заголовок (8 байт) → все IFD с их значениями (метаданные, единицы килобайт) →
//! полосы. Метаданные собираются в небольшой буфер дважды: первый раз с нулевыми смещениями полос, чтобы
//! узнать размер, второй — с настоящими. Размер массива смещений от значений не зависит, поэтому раскладка
//! второго прохода совпадает с первой (это проверяется).

use std::collections::BTreeMap;
use std::io::Write;

/// Длина заголовка TIFF: `II`/`MM`, число 42, смещение первого IFD (TIFF 6.0, раздел 2).
const HEADER_LEN: usize = 8;
/// Максимум полей в IFD: счётчик — `SHORT`.
const MAX_ENTRIES_PER_IFD: usize = u16::MAX as usize;

/// Тег `StripOffsets` (TIFF 6.0, раздел 3): его заполняет писатель, вызывающий код его не задаёт.
pub const TAG_STRIP_OFFSETS: u16 = 273;
/// Тег `StripByteCounts` (TIFF 6.0, раздел 3): его заполняет писатель.
pub const TAG_STRIP_BYTE_COUNTS: u16 = 279;

/// Коды типов полей (TIFF 6.0, раздел 2, «Field»; DOUBLE — из TIFF Technical Note 1).
const TYPE_BYTE: u16 = 1;
const TYPE_ASCII: u16 = 2;
const TYPE_SHORT: u16 = 3;
const TYPE_LONG: u16 = 4;
const TYPE_RATIONAL: u16 = 5;
const TYPE_UNDEFINED: u16 = 7;
const TYPE_SRATIONAL: u16 = 10;
const TYPE_DOUBLE: u16 = 12;

/// Порядок байт всех многобайтовых чисел файла (TIFF 6.0, раздел 2, «Image File Header»).
///
/// Порядок один на весь файл: его объявляют два первых байта заголовка, и читатель по ним разбирает
/// и поля IFD, и значения. Байтовые типы (BYTE, ASCII, UNDEFINED) и полосы изображения писатель не
/// переставляет: полосы — чужие байты, их порядок обязан совпадать с объявленным (забота вызывающего).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteOrder {
    /// `II` («Intel»): младший байт первым.
    LittleEndian,
    /// `MM` («Motorola»): старший байт первым.
    BigEndian,
}

impl ByteOrder {
    /// Два байта заголовка, объявляющие порядок.
    fn header_marker(self) -> &'static [u8; 2] {
        match self {
            Self::LittleEndian => b"II",
            Self::BigEndian => b"MM",
        }
    }

    fn u16_bytes(self, value: u16) -> [u8; 2] {
        match self {
            Self::LittleEndian => value.to_le_bytes(),
            Self::BigEndian => value.to_be_bytes(),
        }
    }

    fn u32_bytes(self, value: u32) -> [u8; 4] {
        match self {
            Self::LittleEndian => value.to_le_bytes(),
            Self::BigEndian => value.to_be_bytes(),
        }
    }

    fn i32_bytes(self, value: i32) -> [u8; 4] {
        match self {
            Self::LittleEndian => value.to_le_bytes(),
            Self::BigEndian => value.to_be_bytes(),
        }
    }

    fn f64_bytes(self, value: f64) -> [u8; 8] {
        match self {
            Self::LittleEndian => value.to_le_bytes(),
            Self::BigEndian => value.to_be_bytes(),
        }
    }
}

/// Почему TIFF не записан.
#[derive(Debug, thiserror::Error)]
pub enum TiffError {
    /// У поля нет значений (счётчик 0 недопустим).
    #[error("у тега {0} пустое значение")]
    EmptyValue(u16),
    /// В ASCII-поле попал символ вне 7-битной таблицы или нулевой байт.
    #[error("в ASCII-поле тега {0} недопустимый символ")]
    InvalidAscii(u16),
    /// В IFD нет ни одного поля или их больше, чем вмещает счётчик.
    #[error("в IFD {0} полей: допустимо от 1 до 65535")]
    BadEntryCount(usize),
    /// Теги полос заняты писателем.
    #[error("тег {0} задаёт писатель (полосы изображения), его нельзя ставить вручную")]
    ReservedTag(u16),
    /// Файл не помещается в классический TIFF (смещения 32 бита, максимум 4 ГиБ).
    #[error("файл не помещается в классический TIFF (смещения 32 бита)")]
    TooLarge,
    /// Внутренняя ошибка: раскладка второго прохода разошлась с первой.
    #[error("внутренняя ошибка раскладки TIFF: метаданные заняли {second} байт вместо {first}")]
    LayoutMismatch { first: usize, second: usize },
    /// Ошибка записи в приёмник.
    #[error("ошибка записи TIFF: {0}")]
    Io(#[from] std::io::Error),
}

/// Значение поля IFD.
#[derive(Debug, Clone, PartialEq)]
pub enum TiffValue {
    Byte(Vec<u8>),
    /// Текст в 7-битной ASCII; завершающий нулевой байт добавляет писатель.
    Ascii(String),
    Short(Vec<u16>),
    Long(Vec<u32>),
    /// Пары (числитель, знаменатель).
    Rational(Vec<(u32, u32)>),
    /// Пары (числитель, знаменатель) со знаком.
    SRational(Vec<(i32, i32)>),
    Double(Vec<f64>),
    Undefined(Vec<u8>),
    /// Вложенный IFD (например Exif IFD): в поле пишется LONG со смещением вложенного каталога.
    Ifd(Box<Ifd>),
    /// Массив вложенных IFD (тег `SubIFDs`, 330): LONG-массив смещений.
    SubIfds(Vec<Ifd>),
}

impl TiffValue {
    /// Один SHORT.
    pub fn short(value: u16) -> Self {
        Self::Short(vec![value])
    }

    /// Один LONG.
    pub fn long(value: u32) -> Self {
        Self::Long(vec![value])
    }

    /// Текстовое поле.
    pub fn ascii(text: &str) -> Self {
        Self::Ascii(text.to_owned())
    }

    /// Вложенный каталог.
    pub fn ifd(ifd: Ifd) -> Self {
        Self::Ifd(Box::new(ifd))
    }

    /// Кодирует значение: (тип, счётчик, байты в порядке `order`). Вложенные IFD здесь не кодируются.
    fn encode_plain(&self, tag: u16, order: ByteOrder) -> Result<(u16, u32, Vec<u8>), TiffError> {
        let (type_code, count, bytes): (u16, usize, Vec<u8>) = match self {
            Self::Byte(values) => (TYPE_BYTE, values.len(), values.clone()),
            Self::Ascii(text) => {
                // TIFF 6.0: ASCII — 7-битные символы, строка заканчивается NUL, который входит в счётчик.
                if !text.is_ascii() || text.contains('\0') {
                    return Err(TiffError::InvalidAscii(tag));
                }
                let mut bytes = text.as_bytes().to_vec();
                bytes.push(0);
                (TYPE_ASCII, bytes.len(), bytes)
            }
            Self::Short(values) => (
                TYPE_SHORT,
                values.len(),
                values.iter().flat_map(|value| order.u16_bytes(*value)).collect(),
            ),
            Self::Long(values) => (
                TYPE_LONG,
                values.len(),
                values.iter().flat_map(|value| order.u32_bytes(*value)).collect(),
            ),
            Self::Rational(values) => (
                TYPE_RATIONAL,
                values.len(),
                values
                    .iter()
                    .flat_map(|(numerator, denominator)| {
                        order
                            .u32_bytes(*numerator)
                            .into_iter()
                            .chain(order.u32_bytes(*denominator))
                    })
                    .collect(),
            ),
            Self::SRational(values) => (
                TYPE_SRATIONAL,
                values.len(),
                values
                    .iter()
                    .flat_map(|(numerator, denominator)| {
                        order
                            .i32_bytes(*numerator)
                            .into_iter()
                            .chain(order.i32_bytes(*denominator))
                    })
                    .collect(),
            ),
            Self::Double(values) => (
                TYPE_DOUBLE,
                values.len(),
                values.iter().flat_map(|value| order.f64_bytes(*value)).collect(),
            ),
            Self::Undefined(values) => (TYPE_UNDEFINED, values.len(), values.clone()),
            Self::Ifd(_) | Self::SubIfds(_) => return Err(TiffError::EmptyValue(tag)),
        };
        if count == 0 {
            return Err(TiffError::EmptyValue(tag));
        }
        let count = u32::try_from(count).map_err(|_| TiffError::TooLarge)?;
        Ok((type_code, count, bytes))
    }
}

/// Каталог полей (IFD). Поля хранятся отсортированными по тегу — так их и пишет TIFF.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ifd {
    entries: BTreeMap<u16, TiffValue>,
    next: Option<Box<Ifd>>,
}

impl Ifd {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ставит поле; повторная установка того же тега заменяет значение (дубликатов в файле не бывает).
    pub fn set(&mut self, tag: u16, value: TiffValue) -> &mut Self {
        self.entries.insert(tag, value);
        self
    }

    /// Есть ли поле с таким тегом.
    pub fn contains(&self, tag: u16) -> bool {
        self.entries.contains_key(&tag)
    }

    /// Сколько полей в каталоге (без вложенных).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Задаёт «следующий IFD» (цепочка каталогов, TIFF 6.0 раздел 2).
    pub fn set_next(&mut self, next: Ifd) -> &mut Self {
        self.next = Some(Box::new(next));
        self
    }
}

/// Файл TIFF: корневой IFD, порядок байт и полосы изображения (срезы чужих байт, не копируются).
#[derive(Debug, Clone)]
pub struct TiffFile<'a> {
    root: Ifd,
    byte_order: ByteOrder,
    strips: Vec<&'a [u8]>,
}

impl<'a> TiffFile<'a> {
    /// Файл в порядке little-endian (`II`) — так пишется DNG.
    pub fn new(root: Ifd) -> Self {
        Self::with_byte_order(root, ByteOrder::LittleEndian)
    }

    /// Файл в явно выбранном порядке байт.
    pub fn with_byte_order(root: Ifd, byte_order: ByteOrder) -> Self {
        Self {
            root,
            byte_order,
            strips: Vec::new(),
        }
    }

    /// Добавляет полосу изображения; порядок вызовов = порядок полос. Теги `StripOffsets` и
    /// `StripByteCounts` корневого IFD заполнит писатель.
    pub fn push_strip(&mut self, bytes: &'a [u8]) {
        self.strips.push(bytes);
    }

    /// Пишет файл в `out` потоком; возвращает число записанных байт.
    pub fn write(&self, out: &mut dyn Write) -> Result<u64, TiffError> {
        if self.root.contains(TAG_STRIP_OFFSETS) {
            return Err(TiffError::ReservedTag(TAG_STRIP_OFFSETS));
        }
        if self.root.contains(TAG_STRIP_BYTE_COUNTS) {
            return Err(TiffError::ReservedTag(TAG_STRIP_BYTE_COUNTS));
        }

        // Проход 1: узнаём размер метаданных (смещения полос пока нули).
        let probe = self.encode_metadata(&vec![0; self.strips.len()])?;
        let strips_start = align_even(HEADER_LEN + probe.len());

        // Смещения полос: каждая начинается на чётном адресе.
        let mut offsets = Vec::with_capacity(self.strips.len());
        let mut cursor = strips_start;
        for strip in &self.strips {
            offsets.push(u32::try_from(cursor).map_err(|_| TiffError::TooLarge)?);
            cursor = align_even(cursor.checked_add(strip.len()).ok_or(TiffError::TooLarge)?);
        }
        u32::try_from(cursor).map_err(|_| TiffError::TooLarge)?;

        // Проход 2: настоящие смещения.
        let metadata = self.encode_metadata(&offsets)?;
        if metadata.len() != probe.len() {
            return Err(TiffError::LayoutMismatch {
                first: probe.len(),
                second: metadata.len(),
            });
        }

        let order = self.byte_order;
        out.write_all(order.header_marker())?;
        out.write_all(&order.u16_bytes(42))?;
        out.write_all(&order.u32_bytes(HEADER_LEN as u32))?;
        out.write_all(&metadata)?;
        write_padding(out, strips_start - (HEADER_LEN + metadata.len()))?;
        for strip in &self.strips {
            out.write_all(strip)?;
            write_padding(out, strip.len() % 2)?;
        }
        Ok(cursor as u64)
    }

    /// Метаданные (всё после заголовка до начала полос) с заданными смещениями полос.
    fn encode_metadata(&self, strip_offsets: &[u32]) -> Result<Vec<u8>, TiffError> {
        let mut root = self.root.clone();
        if !self.strips.is_empty() {
            let counts = self
                .strips
                .iter()
                .map(|strip| u32::try_from(strip.len()).map_err(|_| TiffError::TooLarge))
                .collect::<Result<Vec<u32>, TiffError>>()?;
            root.set(TAG_STRIP_OFFSETS, TiffValue::Long(strip_offsets.to_vec()));
            root.set(TAG_STRIP_BYTE_COUNTS, TiffValue::Long(counts));
        }
        let mut encoder = Encoder {
            buffer: Vec::new(),
            order: self.byte_order,
        };
        encoder.encode_ifd(&root)?;
        Ok(encoder.buffer)
    }
}

/// Кодировщик метаданных: буфер лежит сразу после заголовка, поэтому абсолютное смещение =
/// `HEADER_LEN + индекс в буфере`. Первый IFD начинается ровно по смещению 8 (его адрес в заголовке).
struct Encoder {
    buffer: Vec<u8>,
    /// Порядок байт файла: в нём кодируются счётчики, теги, типы, смещения и значения.
    order: ByteOrder,
}

impl Encoder {
    /// Абсолютное смещение текущего конца буфера.
    fn position(&self) -> Result<u32, TiffError> {
        u32::try_from(HEADER_LEN + self.buffer.len()).map_err(|_| TiffError::TooLarge)
    }

    /// Добивает буфер нулём до чётного абсолютного адреса.
    fn align(&mut self) {
        if (HEADER_LEN + self.buffer.len()) % 2 == 1 {
            self.buffer.push(0);
        }
    }

    /// Кодирует IFD с его значениями и вложенными каталогами; возвращает смещение каталога.
    fn encode_ifd(&mut self, ifd: &Ifd) -> Result<u32, TiffError> {
        let count = ifd.entries.len();
        if count == 0 || count > MAX_ENTRIES_PER_IFD {
            return Err(TiffError::BadEntryCount(count));
        }
        self.align();
        let ifd_offset = self.position()?;
        let block_start = self.buffer.len();
        // Блок каталога: счётчик (2) + поля (12 каждое) + указатель на следующий IFD (4).
        self.buffer.resize(block_start + 2 + 12 * count + 4, 0);
        self.buffer[block_start..block_start + 2].copy_from_slice(&self.order.u16_bytes(count as u16));

        for (index, (tag, value)) in ifd.entries.iter().enumerate() {
            let (type_code, value_count, payload) = self.encode_value(*tag, value)?;
            let entry_start = block_start + 2 + 12 * index;
            // Значение до 4 байт лежит в самом поле (выровнено влево), иначе — по смещению.
            let value_field: [u8; 4] = if payload.len() <= 4 {
                let mut inline = [0u8; 4];
                inline[..payload.len()].copy_from_slice(&payload);
                inline
            } else {
                self.align();
                let offset = self.position()?;
                self.buffer.extend_from_slice(&payload);
                self.order.u32_bytes(offset)
            };
            let entry = &mut self.buffer[entry_start..entry_start + 12];
            entry[0..2].copy_from_slice(&self.order.u16_bytes(*tag));
            entry[2..4].copy_from_slice(&self.order.u16_bytes(type_code));
            entry[4..8].copy_from_slice(&self.order.u32_bytes(value_count));
            entry[8..12].copy_from_slice(&value_field);
        }

        if let Some(next) = &ifd.next {
            let next_offset = self.encode_ifd(next)?;
            let pointer_at = block_start + 2 + 12 * count;
            self.buffer[pointer_at..pointer_at + 4].copy_from_slice(&self.order.u32_bytes(next_offset));
        }
        Ok(ifd_offset)
    }

    /// Тип, счётчик и байты значения; вложенные каталоги кодируются здесь же и превращаются в LONG.
    fn encode_value(&mut self, tag: u16, value: &TiffValue) -> Result<(u16, u32, Vec<u8>), TiffError> {
        match value {
            TiffValue::Ifd(child) => {
                let offset = self.encode_ifd(child)?;
                Ok((TYPE_LONG, 1, self.order.u32_bytes(offset).to_vec()))
            }
            TiffValue::SubIfds(children) => {
                if children.is_empty() {
                    return Err(TiffError::EmptyValue(tag));
                }
                let mut bytes = Vec::with_capacity(children.len() * 4);
                for child in children {
                    let child_offset = self.encode_ifd(child)?;
                    bytes.extend_from_slice(&self.order.u32_bytes(child_offset));
                }
                let count = u32::try_from(children.len()).map_err(|_| TiffError::TooLarge)?;
                Ok((TYPE_LONG, count, bytes))
            }
            plain => plain.encode_plain(tag, self.order),
        }
    }
}

fn align_even(position: usize) -> usize {
    position + position % 2
}

fn write_padding(out: &mut dyn Write, bytes: usize) -> Result<(), TiffError> {
    // Не больше одного байта: выравнивание на чётный адрес; в проходе метаданных — тоже мелочь.
    const ZEROS: [u8; 8] = [0; 8];
    let mut remaining = bytes;
    while remaining > 0 {
        let chunk = remaining.min(ZEROS.len());
        out.write_all(&ZEROS[..chunk])?;
        remaining -= chunk;
    }
    Ok(())
}

/// Переводит неотрицательное число в беззнаковую дробь с заданным знаменателем (округление к ближайшему).
/// `None` — число отрицательное, не конечное или не помещается в LONG.
pub fn unsigned_rational(value: f64, denominator: u32) -> Option<(u32, u32)> {
    if !value.is_finite() || value < 0.0 || denominator == 0 {
        return None;
    }
    let numerator = (value * f64::from(denominator)).round();
    if numerator > f64::from(u32::MAX) {
        return None;
    }
    Some((numerator as u32, denominator))
}

/// То же со знаком (SRATIONAL).
pub fn signed_rational(value: f64, denominator: u32) -> Option<(i32, i32)> {
    let denominator_signed = i32::try_from(denominator).ok()?;
    if !value.is_finite() || denominator == 0 {
        return None;
    }
    let numerator = (value * f64::from(denominator)).round();
    if numerator > f64::from(i32::MAX) || numerator < f64::from(i32::MIN) {
        return None;
    }
    Some((numerator as i32, denominator_signed))
}
