//! Блок Exif для HEIF и JPEG (Exif 2.32, CIPA DC-008) поверх писателя [`crate::tiff`].
//!
//! Здесь же — общие для DNG и Exif описания камеры и момента съёмки: их заполняет приложение
//! (каталог камер, часы), а `photo-output` только записывает.
//!
//! Типы и сигнатуры — договор с модулем `service`.

use std::time::Duration;

use camera_contract::geometry::PixelSize;
use still_contract::shot::{StillFrameFacts, StillRotation};

use crate::tiff::{ByteOrder, Ifd, TiffFile, TiffValue, unsigned_rational};

mod calendar;

/// Объектив снятой камеры (из характеристик камеры).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LensDescription {
    /// Фокусное расстояние, мм (настоящее, не эквивалент).
    pub focal_length_mm: f32,
    /// Диафрагма f/N.
    pub f_number: f32,
}

/// Описание камеры для тегов файла.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraDescription {
    /// Производитель телефона (Exif `Make`).
    pub make: String,
    /// Модель телефона (Exif `Model`).
    pub model: String,
    /// Уникальное имя модели камеры для DNG (`UniqueCameraModel`): различает камеры одного телефона.
    pub unique_camera_model: String,
    /// Наша программа и версия (`Software`).
    pub software: String,
    /// Объектив, если характеристики его отдали.
    pub lens: Option<LensDescription>,
}

/// Момент съёмки по настенным часам (для `DateTimeOriginal` и имени файла).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureMoment {
    /// Миллисекунды Unix-времени (UTC).
    pub utc_millis: i64,
    /// Смещение местного времени от UTC, минуты (`OffsetTimeOriginal`).
    pub utc_offset_minutes: i16,
}

/// Где живёт поворот картинки. Читатель, увидевший поворот и в контейнере, и в Exif, применил бы
/// оба и повернул кадр дважды, поэтому поворот должен жить ровно в одном месте.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RotationCarrier {
    /// Поворот несёт тег Exif `Orientation` (JPEG: пиксели лежат как у датчика).
    ExifOrientationTag,
    /// Поворот несёт контейнер (HEIF: `irot` из `HeifWriter.setRotation`); тега `Orientation` в Exif НЕТ.
    ///
    /// Не `Orientation = 1`, а именно отсутствие тега: платформенный `ExifInterface` (через него
    /// MediaStore заполняет столбец `orientation`) ставит поворот из `irot`, а затем перезаписывает его
    /// тегом Exif, если тот есть. С `Orientation = 1` галерея считала снимок неповёрнутым (14c, телефон).
    ContainerTransform,
}

/// Что не так со сборкой блока Exif. Метаданные необязательны: служба на этой ошибке пишет файл БЕЗ
/// Exif, а не срывает снимок.
#[derive(Debug, thiserror::Error)]
pub enum ExifError {
    /// Писатель TIFF отказал (на практике невозможно: пишем в `Vec`, тексты очищены до ASCII).
    #[error("не удалось собрать блок Exif: {0}")]
    Layout(#[from] crate::tiff::TiffError),
}

/// Что нужно для блока Exif основной картинки.
#[derive(Debug, Clone, Copy)]
pub struct ExifInputs<'a> {
    /// Где живёт поворот картинки (от этого зависит тег `Orientation`).
    pub rotation_carrier: RotationCarrier,
    pub facts: &'a StillFrameFacts,
    pub camera: &'a CameraDescription,
    pub captured_at: &'a CaptureMoment,
    /// Размер основной картинки (`PixelXDimension`/`PixelYDimension`).
    pub pixel_size: PixelSize,
}

/// Теги IFD0 и Exif IFD (TIFF 6.0 / Exif 2.32, CIPA DC-008, таблицы 4–6).
mod tag {
    pub const ORIENTATION: u16 = 274;
    pub const MAKE: u16 = 271;
    pub const MODEL: u16 = 272;
    pub const SOFTWARE: u16 = 305;
    pub const DATE_TIME: u16 = 306;
    pub const X_RESOLUTION: u16 = 282;
    pub const Y_RESOLUTION: u16 = 283;
    pub const RESOLUTION_UNIT: u16 = 296;
    pub const YCBCR_POSITIONING: u16 = 531;
    pub const EXIF_IFD_POINTER: u16 = 34665;
    pub const EXPOSURE_TIME: u16 = 33434;
    pub const F_NUMBER: u16 = 33437;
    pub const PHOTOGRAPHIC_SENSITIVITY: u16 = 34855;
    pub const EXIF_VERSION: u16 = 36864;
    pub const DATE_TIME_ORIGINAL: u16 = 36867;
    pub const DATE_TIME_DIGITIZED: u16 = 36868;
    pub const OFFSET_TIME: u16 = 36880;
    pub const OFFSET_TIME_ORIGINAL: u16 = 36881;
    pub const OFFSET_TIME_DIGITIZED: u16 = 36882;
    pub const COMPONENTS_CONFIGURATION: u16 = 37121;
    pub const FOCAL_LENGTH: u16 = 37386;
    pub const SUB_SEC_TIME_ORIGINAL: u16 = 37521;
    pub const SUB_SEC_TIME_DIGITIZED: u16 = 37522;
    pub const FLASHPIX_VERSION: u16 = 40960;
    pub const COLOR_SPACE: u16 = 40961;
    pub const PIXEL_X_DIMENSION: u16 = 40962;
    pub const PIXEL_Y_DIMENSION: u16 = 40963;
}

/// Знаменатель дробей для вещественных величин (f/N, фокусное): три знака после запятой.
const REAL_VALUE_DENOMINATOR: u32 = 1000;
/// Максимум значения `PhotographicSensitivity` (SHORT); выше Exif велит читать `ISOSpeed`.
const MAX_SHORT_SENSITIVITY: u32 = u16::MAX as u32;
/// Exif 2.32: версия блока — четыре ASCII-символа без NUL.
const EXIF_VERSION_2_32: &[u8; 4] = b"0232";
/// FlashPix 1.0 — обязательное поле Exif для JPEG, значение фиксировано.
const FLASHPIX_VERSION_1_0: &[u8; 4] = b"0100";
/// ComponentsConfiguration для YCbCr-картинки: Y, Cb, Cr, нет (Exif, таблица 6).
const COMPONENTS_YCBCR: [u8; 4] = [1, 2, 3, 0];
/// `ColorSpace = 1` — sRGB.
const COLOR_SPACE_SRGB: u16 = 1;
/// Разрешение по умолчанию, 72 точки на дюйм: Exif 2.32 (таблица 17) требует `XResolution`,
/// `YResolution` и `ResolutionUnit` в IFD0 JPEG, хотя для телефонного снимка они ничего не значат.
const DEFAULT_RESOLUTION_DPI: u32 = 72;
/// `ResolutionUnit` = 2: дюймы.
const RESOLUTION_UNIT_INCH: u16 = 2;
/// `YCbCrPositioning` = 1: центрированный (обязателен в IFD0 JPEG по Exif 2.32).
const YCBCR_POSITIONING_CENTERED: u16 = 1;
/// Предел смещения зоны от UTC: ±24 часа (больше — значит битые данные часов).
const MAX_UTC_OFFSET_MINUTES: u16 = 24 * 60;

/// Строки времени Exif для одного момента съёмки.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExifTimestamps {
    /// `YYYY:MM:DD HH:MM:SS` по местным часам.
    pub date_time: String,
    /// Доли секунды, три цифры (миллисекунды) — `SubSecTime*`.
    pub sub_second: String,
    /// `±HH:MM` — `OffsetTime*`.
    pub utc_offset: String,
}

/// Строки времени Exif; `None`, если момент не выразить форматом (год вне 0000–9999, смещение вне ±24 ч).
pub(crate) fn exif_timestamps(moment: &CaptureMoment) -> Option<ExifTimestamps> {
    // `unsigned_abs`, а не `abs`: у `i16::MIN` обычный модуль переполняется.
    if moment.utc_offset_minutes.unsigned_abs() > MAX_UTC_OFFSET_MINUTES {
        return None;
    }
    let clock = calendar::wall_clock(moment.utc_millis, moment.utc_offset_minutes)?;
    if !(0..=9999).contains(&clock.year) {
        return None;
    }
    let sign = if moment.utc_offset_minutes < 0 { '-' } else { '+' };
    let offset_abs = moment.utc_offset_minutes.unsigned_abs();
    Some(ExifTimestamps {
        date_time: format!(
            "{:04}:{:02}:{:02} {:02}:{:02}:{:02}",
            clock.year, clock.month, clock.day, clock.hour, clock.minute, clock.second
        ),
        sub_second: format!("{:03}", clock.millisecond),
        utc_offset: format!("{sign}{:02}:{:02}", offset_abs / 60, offset_abs % 60),
    })
}

/// Значение тега `Orientation` (TIFF 6.0 / Exif): поворот по часовой стрелке, который нужно применить
/// к пикселям, чтобы картинка встала «как держали телефон». 1 — без поворота, 6 — 90°, 3 — 180°, 8 — 270°.
pub(crate) fn orientation_value(rotation: StillRotation) -> u16 {
    match rotation {
        StillRotation::None => 1,
        StillRotation::Clockwise90 => 6,
        StillRotation::Clockwise180 => 3,
        StillRotation::Clockwise270 => 8,
    }
}

/// Текст для ASCII-поля: всё вне 7-битной таблицы и NUL заменяется на `?` (поле не должно ронять запись).
pub(crate) fn ascii_text(text: &str) -> String {
    text.chars()
        .map(|symbol| {
            if symbol.is_ascii() && symbol != '\0' {
                symbol
            } else {
                '?'
            }
        })
        .collect()
}

/// Выдержка как дробь секунд; `None` — нулевая или не выразимая в LONG.
fn exposure_time_rational(exposure: Duration) -> Option<(u32, u32)> {
    let nanos = exposure.as_nanos();
    if nanos == 0 {
        return None;
    }
    // Наносекундная точность, пока числитель влезает в LONG (до ≈ 4,29 с); дольше — миллисекунды.
    let (numerator, denominator) = match u32::try_from(nanos) {
        Ok(numerator) => (numerator, 1_000_000_000u32),
        Err(_) => (u32::try_from(exposure.as_millis()).ok()?, 1000),
    };
    let divisor = greatest_common_divisor(numerator, denominator);
    Some((numerator / divisor, denominator / divisor))
}

fn greatest_common_divisor(mut first: u32, mut second: u32) -> u32 {
    while second != 0 {
        (first, second) = (second, first % second);
    }
    first.max(1)
}

/// Поля IFD0, общие для Exif-блока и DNG: производитель, модель, программа, время файла (`OffsetTime`
/// живёт в Exif IFD). Тег `Orientation` ставит вызывающий: где живёт поворот, знает только он.
pub(crate) fn fill_image_description_fields(
    ifd: &mut Ifd,
    camera: &CameraDescription,
    timestamps: Option<&ExifTimestamps>,
) {
    ifd.set(tag::MAKE, TiffValue::Ascii(ascii_text(&camera.make)));
    ifd.set(tag::MODEL, TiffValue::Ascii(ascii_text(&camera.model)));
    ifd.set(tag::SOFTWARE, TiffValue::Ascii(ascii_text(&camera.software)));
    if let Some(timestamps) = timestamps {
        ifd.set(tag::DATE_TIME, TiffValue::Ascii(timestamps.date_time.clone()));
    }
}

/// Exif IFD со съёмочными полями, общими для Exif-блока и DNG: выдержка, чувствительность, диафрагма,
/// фокусное расстояние, время съёмки. Поля, которых у кадра нет (HAL не отдал), не пишутся.
pub(crate) fn capture_exif_ifd(
    facts: &StillFrameFacts,
    camera: &CameraDescription,
    timestamps: Option<&ExifTimestamps>,
) -> Ifd {
    let mut exif = Ifd::new();
    exif.set(tag::EXIF_VERSION, TiffValue::Undefined(EXIF_VERSION_2_32.to_vec()));

    if let Some(fraction) = facts.capture.result.exposure_time.and_then(exposure_time_rational) {
        exif.set(tag::EXPOSURE_TIME, TiffValue::Rational(vec![fraction]));
    }
    if let Some(iso) = facts.capture.result.sensitivity_iso {
        let clamped = u16::try_from(iso.min(MAX_SHORT_SENSITIVITY)).unwrap_or(u16::MAX);
        exif.set(tag::PHOTOGRAPHIC_SENSITIVITY, TiffValue::short(clamped));
    }
    if let Some(lens) = camera.lens {
        if let Some(fraction) = unsigned_rational(f64::from(lens.f_number), REAL_VALUE_DENOMINATOR) {
            exif.set(tag::F_NUMBER, TiffValue::Rational(vec![fraction]));
        }
        if let Some(fraction) = unsigned_rational(f64::from(lens.focal_length_mm), REAL_VALUE_DENOMINATOR) {
            exif.set(tag::FOCAL_LENGTH, TiffValue::Rational(vec![fraction]));
        }
    }
    if let Some(timestamps) = timestamps {
        let date_time = TiffValue::Ascii(timestamps.date_time.clone());
        let sub_second = TiffValue::Ascii(timestamps.sub_second.clone());
        let utc_offset = TiffValue::Ascii(timestamps.utc_offset.clone());
        exif.set(tag::OFFSET_TIME, utc_offset.clone());
        // Кадр снят и оцифрован в один момент: «оригинал» и «оцифровка» совпадают.
        exif.set(tag::DATE_TIME_ORIGINAL, date_time.clone());
        exif.set(tag::DATE_TIME_DIGITIZED, date_time);
        exif.set(tag::SUB_SEC_TIME_ORIGINAL, sub_second.clone());
        exif.set(tag::SUB_SEC_TIME_DIGITIZED, sub_second);
        exif.set(tag::OFFSET_TIME_ORIGINAL, utc_offset.clone());
        exif.set(tag::OFFSET_TIME_DIGITIZED, utc_offset);
    }
    exif
}

/// Собирает блок Exif как поток TIFF (без заголовка `Exif\0\0`): IFD0 + Exif IFD.
///
/// Порядок байт — big-endian (`MM`), а не `II`, как у DNG. Причина — платформенный `ExifInterface`
/// Android 16 (им MediaStore читает размеры и поворот HEIF): размеры и поворот из контейнера он
/// кладёт числами в порядке `MM`, а читает их потом в порядке байт блока Exif. С блоком `II` MediaStore
/// записывал снимок 4096×3072 как 16×12 (0x1000 → 0x0010), а поворот терял. Проверено на телефоне в
/// 14c: тот же HEIF с блоком `MM` индексируется верно. `MM` — и самый распространённый порядок у камер.
///
/// Ошибка раскладки возвращается вызывающему: он решает, что делать с файлом (служба пишет снимок
/// без Exif, считает и логирует сбой).
pub fn build_exif_tiff(inputs: &ExifInputs<'_>) -> Result<Vec<u8>, ExifError> {
    let timestamps = exif_timestamps(inputs.captured_at);
    if timestamps.is_none() {
        tracing::warn!(?inputs.captured_at, "момент съёмки не выражается в Exif: времена не пишутся");
    }

    let mut exif_ifd = capture_exif_ifd(inputs.facts, inputs.camera, timestamps.as_ref());
    exif_ifd.set(
        tag::COMPONENTS_CONFIGURATION,
        TiffValue::Undefined(COMPONENTS_YCBCR.to_vec()),
    );
    exif_ifd.set(
        tag::FLASHPIX_VERSION,
        TiffValue::Undefined(FLASHPIX_VERSION_1_0.to_vec()),
    );
    exif_ifd.set(tag::COLOR_SPACE, TiffValue::short(COLOR_SPACE_SRGB));
    exif_ifd.set(tag::PIXEL_X_DIMENSION, TiffValue::long(inputs.pixel_size.width));
    exif_ifd.set(tag::PIXEL_Y_DIMENSION, TiffValue::long(inputs.pixel_size.height));

    let mut ifd0 = Ifd::new();
    match inputs.rotation_carrier {
        RotationCarrier::ExifOrientationTag => {
            ifd0.set(
                tag::ORIENTATION,
                TiffValue::short(orientation_value(inputs.facts.rotation)),
            );
        }
        // Поворот уже несёт контейнер: тега нет вовсе (см. `RotationCarrier::ContainerTransform`).
        RotationCarrier::ContainerTransform => {}
    }
    fill_image_description_fields(&mut ifd0, inputs.camera, timestamps.as_ref());
    // Обязательные для Exif-JPEG поля IFD0, не относящиеся к DNG.
    let resolution = TiffValue::Rational(vec![(DEFAULT_RESOLUTION_DPI, 1)]);
    ifd0.set(tag::X_RESOLUTION, resolution.clone());
    ifd0.set(tag::Y_RESOLUTION, resolution);
    ifd0.set(tag::RESOLUTION_UNIT, TiffValue::short(RESOLUTION_UNIT_INCH));
    ifd0.set(tag::YCBCR_POSITIONING, TiffValue::short(YCBCR_POSITIONING_CENTERED));
    ifd0.set(tag::EXIF_IFD_POINTER, TiffValue::ifd(exif_ifd));

    let mut bytes = Vec::new();
    TiffFile::with_byte_order(ifd0, ByteOrder::BigEndian).write(&mut bytes)?;
    Ok(bytes)
}
