//! Значения записей метаданных Camera2 в нейтральном виде и доступ к ним.
//!
//! `MetadataSource` — «дай запись по тегу» — единственное, что нужно разбору характеристик
//! и результатов. На Android его реализует обёртка над `ACameraMetadata`; на ПК — фейковая
//! карта тегов с числами OnePlus 11. Записи копируются в `Vec`: характеристики читаются раз
//! на камеру, а результат — пять скаляров на кадр, это не горячий путь.

use camera_contract::capabilities::Rational;
use serde::Serialize;

use crate::tags::Tag;

/// Тип записи метаданных (`ACAMERA_TYPE_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Byte,
    Int32,
    Int64,
    Float,
    Double,
    Rational,
}

/// Значение записи метаданных (массив: скаляр — массив из одного элемента).
#[derive(Debug, Clone, PartialEq)]
pub enum EntryValue {
    Bytes(Vec<u8>),
    Int32s(Vec<i32>),
    Int64s(Vec<i64>),
    Floats(Vec<f32>),
    Doubles(Vec<f64>),
    Rationals(Vec<Rational>),
}

impl EntryValue {
    pub fn kind(&self) -> EntryKind {
        match self {
            EntryValue::Bytes(_) => EntryKind::Byte,
            EntryValue::Int32s(_) => EntryKind::Int32,
            EntryValue::Int64s(_) => EntryKind::Int64,
            EntryValue::Floats(_) => EntryKind::Float,
            EntryValue::Doubles(_) => EntryKind::Double,
            EntryValue::Rationals(_) => EntryKind::Rational,
        }
    }

    /// Число элементов в записи.
    pub fn len(&self) -> usize {
        match self {
            EntryValue::Bytes(values) => values.len(),
            EntryValue::Int32s(values) => values.len(),
            EntryValue::Int64s(values) => values.len(),
            EntryValue::Floats(values) => values.len(),
            EntryValue::Doubles(values) => values.len(),
            EntryValue::Rationals(values) => values.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Итог чтения записи: отсутствие и сбой чтения — разные исходы.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryLookup {
    Present(EntryValue),
    /// HAL ключ не отдал (`ACAMERA_ERROR_METADATA_NOT_FOUND`).
    Absent,
    /// Чтение не удалось по другой причине; `status` — `camera_status_t`.
    Unreadable {
        status: i32,
    },
}

/// Источник метаданных: характеристики камеры или результат запроса.
pub trait MetadataSource {
    fn entry(&self, tag: Tag) -> EntryLookup;
}

/// Чем запись не подошла нужному разбору.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "problem")]
pub enum EntryProblem {
    #[error("тип записи {actual:?} вместо {expected:?}")]
    WrongType { expected: EntryKind, actual: EntryKind },
    #[error("в записи {actual} элементов, нужно не меньше {expected}")]
    TooFewElements { expected: usize, actual: usize },
    #[error("значение вне допустимого диапазона")]
    OutOfRange,
    #[error("чтение записи не удалось, код {status}")]
    Unreadable { status: i32 },
}

/// Прочитанное значение: `Ok(None)` — ключа нет, `Err` — ключ есть, но непригоден.
pub type Read<T> = Result<Option<T>, EntryProblem>;

/// Читает запись типа «байты» (не меньше `min_len` элементов).
pub fn read_bytes(source: &dyn MetadataSource, tag: Tag, min_len: usize) -> Read<Vec<u8>> {
    read_typed(source, tag, min_len, EntryKind::Byte, |value| match value {
        EntryValue::Bytes(values) => Some(values),
        _ => None,
    })
}

pub fn read_int32s(source: &dyn MetadataSource, tag: Tag, min_len: usize) -> Read<Vec<i32>> {
    read_typed(source, tag, min_len, EntryKind::Int32, |value| match value {
        EntryValue::Int32s(values) => Some(values),
        _ => None,
    })
}

pub fn read_int64s(source: &dyn MetadataSource, tag: Tag, min_len: usize) -> Read<Vec<i64>> {
    read_typed(source, tag, min_len, EntryKind::Int64, |value| match value {
        EntryValue::Int64s(values) => Some(values),
        _ => None,
    })
}

pub fn read_floats(source: &dyn MetadataSource, tag: Tag, min_len: usize) -> Read<Vec<f32>> {
    read_typed(source, tag, min_len, EntryKind::Float, |value| match value {
        EntryValue::Floats(values) => Some(values),
        _ => None,
    })
}

pub fn read_rationals(source: &dyn MetadataSource, tag: Tag, min_len: usize) -> Read<Vec<Rational>> {
    read_typed(source, tag, min_len, EntryKind::Rational, |value| match value {
        EntryValue::Rationals(values) => Some(values),
        _ => None,
    })
}

/// Первый элемент байтовой записи (скалярный байт).
pub fn read_byte(source: &dyn MetadataSource, tag: Tag) -> Read<u8> {
    Ok(read_bytes(source, tag, 1)?.and_then(|values| values.first().copied()))
}

pub fn read_int32(source: &dyn MetadataSource, tag: Tag) -> Read<i32> {
    Ok(read_int32s(source, tag, 1)?.and_then(|values| values.first().copied()))
}

pub fn read_int64(source: &dyn MetadataSource, tag: Tag) -> Read<i64> {
    Ok(read_int64s(source, tag, 1)?.and_then(|values| values.first().copied()))
}

pub fn read_float(source: &dyn MetadataSource, tag: Tag) -> Read<f32> {
    Ok(read_floats(source, tag, 1)?.and_then(|values| values.first().copied()))
}

fn read_typed<T>(
    source: &dyn MetadataSource,
    tag: Tag,
    min_len: usize,
    expected: EntryKind,
    extract: impl FnOnce(EntryValue) -> Option<Vec<T>>,
) -> Read<Vec<T>> {
    match source.entry(tag) {
        EntryLookup::Absent => Ok(None),
        EntryLookup::Unreadable { status } => Err(EntryProblem::Unreadable { status }),
        EntryLookup::Present(value) => {
            let actual = value.kind();
            let elements = extract(value).ok_or(EntryProblem::WrongType { expected, actual })?;
            if elements.len() < min_len {
                return Err(EntryProblem::TooFewElements {
                    expected: min_len,
                    actual: elements.len(),
                });
            }
            Ok(Some(elements))
        }
    }
}
