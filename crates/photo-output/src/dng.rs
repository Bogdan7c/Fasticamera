//! Своя clean-room запись DNG по спецификации DNG 1.7 (Adobe, 2023): номера разделов — в комментариях.
//!
//! Файл — один IFD0 с сырыми отсчётами CFA без миниатюры (как у `DngCreator`) и вложенный Exif IFD.
//! Версия: `DNGVersion` = 1.4.0.0 (используем `ForwardMatrix` — 1.2, `NoiseProfile` — 1.3; ничего из 1.5+),
//! `DNGBackwardVersion` = 1.1.0.0: приложение А спецификации требует поднимать его только для
//! `ActiveArea` с маскированными пикселями, 1.2+ опций, нестандартных `CFALayout` и т. п. — мы ничего
//! из этого не пишем, а лишнее повышение запрещено («writers should not increase … higher than
//! otherwise required»). Так же пишет и платформенный `DngCreator`.
//!
//! Договор с модулем `service` — типы и сигнатуры ниже.

use std::io::Write;

use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::PixelRect;
use still_contract::shot::{RawSamples16, StillFrameFacts};

use crate::exif::{
    CameraDescription, CaptureMoment, ascii_text, capture_exif_ifd, exif_timestamps, fill_image_description_fields,
    orientation_value,
};
use crate::tiff::{Ifd, TiffError, TiffFile, TiffValue, signed_rational, unsigned_rational};

mod cfa;
mod profile;
mod tags;

/// Всё, что нужно для одного DNG.
#[derive(Debug, Clone, Copy)]
pub struct DngInputs<'a> {
    pub facts: &'a StillFrameFacts,
    /// Сырые отсчёты полезной области (16 бит, little-endian).
    pub raw: &'a RawSamples16,
    pub camera: &'a CameraDescription,
    pub captured_at: &'a CaptureMoment,
}

/// Почему DNG не записан.
#[derive(Debug, thiserror::Error)]
pub enum DngError {
    /// Данные кадра не годятся для DNG (размер, мозаика, калибровка) — с объяснением.
    #[error("кадр не годится для DNG: {0}")]
    InvalidInput(String),
    /// Ошибка записи в приёмник.
    #[error("ошибка записи DNG: {0}")]
    Io(#[from] std::io::Error),
    /// Файл не удалось разложить в TIFF (не ошибка ввода-вывода): слишком большой или внутренний сбой.
    #[error("не удалось разложить DNG: {0}")]
    Layout(String),
}

impl From<TiffError> for DngError {
    fn from(error: TiffError) -> Self {
        match error {
            TiffError::Io(io_error) => Self::Io(io_error),
            other => Self::Layout(other.to_string()),
        }
    }
}

/// Настройки записи, не меняющие смысл файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DngWriteOptions {
    /// К какому размеру (байт) стремимся в одной полосе изображения. TIFF 6.0 рекомендует полосы
    /// около 8 КиБ; платформенный `DngCreator` пишет по строке (3072 полосы на кадр). Мы берём
    /// целое число строк около 64 КиБ: у кадра 4096×3072 это 384 полосы, массив смещений — 1,5 КиБ,
    /// читатели не делают лишних мелких чтений.
    pub target_strip_bytes: usize,
}

impl Default for DngWriteOptions {
    fn default() -> Self {
        Self {
            target_strip_bytes: 64 * 1024,
        }
    }
}

/// Знаменатель дробей нейтрали и экспозиции: шесть знаков — точность лучше любого шума измерения.
const REAL_DENOMINATOR: u32 = 1_000_000;
/// Знаменатель уровней чёрного: 1/1024 отсчёта точно представляет двоичные дроби float (64,25; 64,375).
const BLACK_LEVEL_DENOMINATOR: u32 = 1024;
/// `DNGVersion` 1.4.0.0 (см. шапку модуля).
const DNG_VERSION: [u8; 4] = [1, 4, 0, 0];
/// `DNGBackwardVersion` 1.1.0.0 (см. шапку модуля).
const DNG_BACKWARD_VERSION: [u8; 4] = [1, 1, 0, 0];
/// `CFALayout` = 1: прямоугольная сетка (DNG 1.7.1.0, CFALayout).
const CFA_LAYOUT_RECTANGULAR: u16 = 1;
/// Сжатия нет (TIFF 6.0, `Compression` = 1).
const COMPRESSION_NONE: u16 = 1;
/// Тип основного изображения `NewSubFileType` = 0 (полноразмерное).
const MAIN_IMAGE_SUBFILE_TYPE: u32 = 0;

/// Пишет DNG целиком в `out` потоком (файл ≈ 25 МБ — в память не собирается).
pub fn write_dng(inputs: &DngInputs<'_>, out: &mut dyn Write) -> Result<(), DngError> {
    write_dng_with_options(inputs, DngWriteOptions::default(), out)
}

/// То же с настройками раскладки.
pub fn write_dng_with_options(
    inputs: &DngInputs<'_>,
    options: DngWriteOptions,
    out: &mut dyn Write,
) -> Result<(), DngError> {
    let size = inputs.raw.size();
    validate_inputs(inputs)?;
    let row_bytes = usize::try_from(size.width)
        .ok()
        .and_then(|width| width.checked_mul(2))
        .ok_or_else(|| DngError::Layout("строка не помещается в память".to_owned()))?;
    let rows_per_strip = (options.target_strip_bytes / row_bytes).clamp(1, size.height as usize);

    let ifd0 = build_ifd0(inputs, u32::try_from(rows_per_strip).unwrap_or(size.height))?;
    let mut file = TiffFile::new(ifd0);
    // Отсчёты уходят в файл как есть: срезы исходного буфера, без копии.
    for strip in inputs.raw.le_bytes().chunks(rows_per_strip * row_bytes) {
        file.push_strip(strip);
    }
    file.write(out)?;
    Ok(())
}

/// Проверки, после которых DNG гарантированно собирается; каждый отказ объясняет причину.
fn validate_inputs(inputs: &DngInputs<'_>) -> Result<(), DngError> {
    let stream = &inputs.facts.stream;
    let valid_area = stream.valid_area;
    let size = inputs.raw.size();
    if size != valid_area.size {
        return Err(DngError::InvalidInput(format!(
            "размер отсчётов {size:?} не равен полезной области {:?}: DNG хранит ровно полезную область",
            valid_area.size
        )));
    }
    if size.width < 2 || size.height < 2 {
        return Err(DngError::InvalidInput(format!(
            "кадр {size:?} меньше одного квадрата CFA 2×2"
        )));
    }
    let buffer = stream.buffer_size;
    if u64::from(valid_area.left) + u64::from(size.width) > u64::from(buffer.width)
        || u64::from(valid_area.top) + u64::from(size.height) > u64::from(buffer.height)
    {
        return Err(DngError::InvalidInput(format!(
            "полезная область {valid_area:?} выходит за буфер {buffer:?}"
        )));
    }
    if cfa::buffer_origin_quad(stream.mosaic).is_none() {
        return Err(DngError::InvalidInput(format!(
            "мозаика {:?} не Bayer: сырой DNG пишется только для Bayer",
            stream.mosaic
        )));
    }
    let crop = inputs.facts.crop;
    let crop_fits = crop.size.width > 0
        && crop.size.height > 0
        && crop.left >= valid_area.left
        && crop.top >= valid_area.top
        && u64::from(crop.left) + u64::from(crop.size.width) <= u64::from(valid_area.left) + u64::from(size.width)
        && u64::from(crop.top) + u64::from(crop.size.height) <= u64::from(valid_area.top) + u64::from(size.height);
    if !crop_fits {
        return Err(DngError::InvalidInput(format!(
            "кадрирование {crop:?} выходит за полезную область {valid_area:?}"
        )));
    }
    let neutral = inputs.facts.neutral.0;
    if !neutral.iter().all(|channel| channel.is_finite() && *channel > 0.0) {
        return Err(DngError::InvalidInput(format!(
            "нейтраль {neutral:?}: нужны конечные положительные числа"
        )));
    }
    if !inputs.facts.digital_gain.0.is_finite() {
        return Err(DngError::InvalidInput(
            "цифровая подтяжка экспозиции не число".to_owned(),
        ));
    }
    Ok(())
}

/// Уровень белого: динамический из результата кадра, если HAL его дал (Camera2 рекомендует его для DNG),
/// иначе статический из характеристик.
fn white_level(inputs: &DngInputs<'_>) -> u32 {
    inputs
        .facts
        .capture
        .color
        .dynamic_white_level
        .filter(|level| *level > 0)
        .unwrap_or(inputs.facts.stream.static_levels.white_level)
}

/// Уровни чёрного по позициям квадрата 2×2 буфера: динамические этого кадра или статические паспорта.
fn black_levels_at_buffer_origin(inputs: &DngInputs<'_>) -> [f64; 4] {
    match inputs.facts.capture.color.dynamic_black_level {
        Some(levels) => levels.map(f64::from),
        None => inputs.facts.stream.static_levels.black_level_pattern.map(f64::from),
    }
}

fn build_ifd0(inputs: &DngInputs<'_>, rows_per_strip: u32) -> Result<Ifd, DngError> {
    let facts = inputs.facts;
    let stream: &StreamPassport = &facts.stream;
    let size = inputs.raw.size();
    let timestamps = exif_timestamps(inputs.captured_at);

    let buffer_quad =
        cfa::buffer_origin_quad(stream.mosaic).ok_or_else(|| DngError::InvalidInput("мозаика не Bayer".to_owned()))?;
    let positions = cfa::shifted_positions(stream.valid_area);
    let image_quad = cfa::apply_shift(buffer_quad, positions);

    let mut ifd = Ifd::new();
    // Базовые поля TIFF и вид изображения.
    ifd.set(tags::NEW_SUBFILE_TYPE, TiffValue::long(MAIN_IMAGE_SUBFILE_TYPE));
    ifd.set(tags::IMAGE_WIDTH, TiffValue::long(size.width));
    ifd.set(tags::IMAGE_LENGTH, TiffValue::long(size.height));
    ifd.set(tags::BITS_PER_SAMPLE, TiffValue::short(16));
    ifd.set(tags::COMPRESSION, TiffValue::short(COMPRESSION_NONE));
    ifd.set(
        tags::PHOTOMETRIC_INTERPRETATION,
        TiffValue::short(tags::PHOTOMETRIC_CFA),
    );
    ifd.set(tags::SAMPLES_PER_PIXEL, TiffValue::short(1));
    ifd.set(tags::ROWS_PER_STRIP, TiffValue::long(rows_per_strip));
    ifd.set(tags::PLANAR_CONFIGURATION, TiffValue::short(1));
    // Для DNG поворот всегда несёт тег `Orientation` (контейнера с поворотом у DNG нет).
    let orientation = orientation_value(facts.rotation);
    fill_image_description_fields(&mut ifd, inputs.camera, orientation, timestamps.as_ref());
    ifd.set(
        tags::EXIF_IFD_POINTER,
        TiffValue::ifd(capture_exif_ifd(facts, inputs.camera, timestamps.as_ref())),
    );

    // Версии DNG и модель камеры.
    ifd.set(tags::DNG_VERSION, TiffValue::Byte(DNG_VERSION.to_vec()));
    ifd.set(
        tags::DNG_BACKWARD_VERSION,
        TiffValue::Byte(DNG_BACKWARD_VERSION.to_vec()),
    );
    ifd.set(
        tags::UNIQUE_CAMERA_MODEL,
        TiffValue::Ascii(ascii_text(&inputs.camera.unique_camera_model)),
    );

    // CFA: узор записанного изображения (с учётом чётности начала полезной области).
    ifd.set(tags::CFA_REPEAT_PATTERN_DIM, TiffValue::Short(vec![2, 2]));
    ifd.set(
        tags::CFA_PATTERN,
        TiffValue::Byte(image_quad.iter().map(|color| *color as u8).collect()),
    );
    ifd.set(tags::CFA_PLANE_COLOR, TiffValue::Byte(vec![0, 1, 2]));
    ifd.set(tags::CFA_LAYOUT, TiffValue::short(CFA_LAYOUT_RECTANGULAR));

    // Уровни: чёрный по позициям записанного изображения, белый.
    let black_levels = cfa::apply_shift(black_levels_at_buffer_origin(inputs), positions);
    let white = white_level(inputs);
    let mut black_pairs = Vec::with_capacity(4);
    for level in black_levels {
        let pair = unsigned_rational(level, BLACK_LEVEL_DENOMINATOR)
            .ok_or_else(|| DngError::InvalidInput(format!("уровень чёрного {level} недопустим")))?;
        black_pairs.push(pair);
    }
    if black_levels.iter().any(|level| *level >= f64::from(white)) {
        return Err(DngError::InvalidInput(format!(
            "уровень чёрного {black_levels:?} не ниже уровня белого {white}"
        )));
    }
    ifd.set(tags::BLACK_LEVEL_REPEAT_DIM, TiffValue::Short(vec![2, 2]));
    ifd.set(tags::BLACK_LEVEL, TiffValue::Rational(black_pairs));
    ifd.set(tags::WHITE_LEVEL, TiffValue::long(white));

    // Кадрирование: в координатах записанного изображения (от начала полезной области).
    let (crop_origin, crop_size) = default_crop(facts.crop, stream.valid_area);
    ifd.set(tags::DEFAULT_CROP_ORIGIN, TiffValue::Long(crop_origin.to_vec()));
    ifd.set(tags::DEFAULT_CROP_SIZE, TiffValue::Long(crop_size.to_vec()));

    profile::fill_color_calibration(&mut ifd, &stream.color_calibration)?;

    // Белый кадра — НАША нейтраль (решение 9), а не `neutral_color_point` от HAL.
    let neutral = facts
        .neutral
        .0
        .iter()
        .map(|channel| unsigned_rational(*channel, REAL_DENOMINATOR))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| DngError::InvalidInput("нейтраль не выражается дробью LONG".to_owned()))?;
    ifd.set(tags::AS_SHOT_NEUTRAL, TiffValue::Rational(neutral));

    // BaselineExposure (DNG 1.7.1.0): «на сколько EV сдвинуть нулевую точку; положительное — ярче
    // по умолчанию». Видоискатель показал кадр со подтяжкой `k` EV (решение 72), снимок повторяет её в
    // HEIF/JPEG; сами отсчёты DNG не масштабируются, поэтому подтяжку несёт этот тег — редактор откроет
    // кадр той же яркости, а в EV-ползунке останется запас.
    let baseline = signed_rational(f64::from(facts.digital_gain.0), REAL_DENOMINATOR)
        .ok_or_else(|| DngError::InvalidInput("цифровая подтяжка не выражается дробью".to_owned()))?;
    ifd.set(tags::BASELINE_EXPOSURE, TiffValue::SRational(vec![baseline]));

    if let Some(profile) = facts.capture.color.noise_profile {
        match cfa::noise_profile_per_plane(&profile, buffer_quad) {
            Some(pairs) => {
                ifd.set(tags::NOISE_PROFILE, TiffValue::Double(pairs.to_vec()));
            }
            None => tracing::warn!(
                ?profile,
                "профиль шума HAL недопустим для DNG: тег NoiseProfile не пишется"
            ),
        }
    }
    Ok(ifd)
}

/// Начало и размер кадрирования относительно полезной области: `[x, y]`, `[ширина, высота]`.
/// Вызывать только после `validate_inputs`: кадрирование внутри полезной области.
fn default_crop(crop: PixelRect, valid_area: PixelRect) -> ([u32; 2], [u32; 2]) {
    (
        [crop.left - valid_area.left, crop.top - valid_area.top],
        [crop.size.width, crop.size.height],
    )
}
