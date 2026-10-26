//! Формат RAW-буфера и мозаика цветного фильтра.

use serde::Serialize;

/// Формат RAW-потока (решение 21: RAW10 по умолчанию, RAW16 на выбор).
///
/// `RAW_PRIVATE` сюда не входит: его раскладка непрозрачна (00 C4), наш ISP его читать не может.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RawFormat {
    /// Упакованный 10-битный (4 пикселя в 5 байтах), `AIMAGE_FORMAT_RAW10`.
    Raw10,
    /// 16 бит на пиксель, значащие младшие биты, `AIMAGE_FORMAT_RAW16`.
    Raw16,
}

impl RawFormat {
    pub fn name(self) -> &'static str {
        match self {
            RawFormat::Raw10 => "raw10",
            RawFormat::Raw16 => "raw16",
        }
    }
}

/// Раскладка цветного фильтра в верхнем левом квадрате 2×2
/// (`ACAMERA_SENSOR_INFO_COLOR_FILTER_ARRANGEMENT`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CfaLayout {
    Rggb,
    Grbg,
    Gbrg,
    Bggr,
    /// Сенсор без мозаики (все три канала в каждом пикселе).
    Rgb,
    /// Монохромный сенсор.
    Mono,
    /// Ближний инфракрасный.
    Nir,
}

/// Вид мозаики: как повторяется цветной фильтр.
///
/// Решение 10: тип мозаики — часть паспорта кадра с первого дня, чтобы полное разрешение
/// (сырой Quad Bayer) и RGBW-сенсоры не ломали архитектуру.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MosaicKind {
    /// Классический Bayer: период 2×2.
    Bayer,
    /// Quad Bayer (QCFA): каждый цвет — блок 2×2, период 4×4.
    QuadBayer,
    /// Сенсор с белыми пикселями.
    Rgbw,
}

/// Мозаика конкретного буфера: вид и раскладка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct MosaicPattern {
    pub kind: MosaicKind,
    pub layout: CfaLayout,
}

/// Родная мозаика сенсора (до биннинга), если её удалось узнать.
///
/// Публичный RAW на OnePlus 11 — уже биннинг до Bayer, а сами сенсоры QCFA (вендорский тег
/// `is_qcfa_sensor`). Без вендорского знания честно говорим «неизвестно», а не угадываем.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case", tag = "state", content = "kind")]
pub enum NativeMosaic {
    Known(MosaicKind),
    Unknown,
}
