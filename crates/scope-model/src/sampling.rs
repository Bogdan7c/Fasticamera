//! Прореживание входа: приборам не нужен каждый пиксель, шаг — рычаг бюджета.

use serde::{Deserialize, Serialize};

/// Наибольший шаг прореживания: дальше отсчётов слишком мало для устойчивой картины.
pub const MAX_SAMPLE_STRIDE: u32 = 64;

/// Ошибка шага прореживания.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SampleStrideError {
    /// Шаг вне `1..=MAX_SAMPLE_STRIDE`.
    #[error("шаг прореживания {value} вне 1..={MAX_SAMPLE_STRIDE}")]
    OutOfRange {
        /// Недопустимое значение.
        value: u32,
    },
}

/// Шаг прореживания `≥ 1`: берётся каждый `stride`-й пиксель по обеим осям.
///
/// Берутся пиксели `x = offset + i·stride`, `y = offset + j·stride`, где `offset = (stride − 1) / 2`
/// (целочисленное деление), при условии `x < width` и `y < height`. Смещение ставит отсчёт в середину
/// блока `stride × stride`, а не в его угол. Если `offset ≥ размер` (картинка меньше половины шага),
/// отсчётов по этой оси нет: счётчики остаются нулевыми, это не ошибка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "u32", into = "u32")]
pub struct SampleStride(u32);

impl SampleStride {
    /// Каждый пиксель.
    pub const EVERY_PIXEL: Self = Self(1);

    /// Каждый второй пиксель по обеим осям (четверть отсчётов) — значение по умолчанию `[estimate]`.
    pub const EVERY_SECOND_PIXEL: Self = Self(2);

    /// Проверенный шаг.
    pub fn try_new(value: u32) -> Result<Self, SampleStrideError> {
        if (1..=MAX_SAMPLE_STRIDE).contains(&value) {
            Ok(Self(value))
        } else {
            Err(SampleStrideError::OutOfRange { value })
        }
    }

    /// Значение шага.
    pub fn get(self) -> u32 {
        self.0
    }

    /// Смещение первого отсчёта: `(stride − 1) / 2`.
    pub fn offset(self) -> u32 {
        (self.0 - 1) / 2
    }

    /// Сколько отсчётов по оси длиной `length`: число `i` с `offset + i·stride < length`.
    pub fn samples_along(self, length: u32) -> u32 {
        let offset = self.offset();
        if offset >= length {
            0
        } else {
            (length - 1 - offset) / self.0 + 1
        }
    }

    /// Координата `index`-го отсчёта по оси.
    pub fn coordinate(self, index: u32) -> u32 {
        self.offset() + index * self.0
    }
}

impl TryFrom<u32> for SampleStride {
    type Error = SampleStrideError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        Self::try_new(value)
    }
}

impl From<SampleStride> for u32 {
    fn from(stride: SampleStride) -> Self {
        stride.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stride_selects_block_centres() {
        let stride = SampleStride::try_new(4).unwrap();
        assert_eq!(stride.offset(), 1);
        let coordinates: Vec<u32> = (0..stride.samples_along(10)).map(|i| stride.coordinate(i)).collect();
        assert_eq!(coordinates, vec![1, 5, 9]);
        assert_eq!(stride.samples_along(9), 2);
        assert_eq!(stride.samples_along(1), 0);
        assert_eq!(SampleStride::EVERY_PIXEL.samples_along(7), 7);
    }

    #[test]
    fn invalid_strides_are_rejected() {
        assert_eq!(
            SampleStride::try_new(0),
            Err(SampleStrideError::OutOfRange { value: 0 })
        );
        assert!(SampleStride::try_new(MAX_SAMPLE_STRIDE + 1).is_err());
        assert!(serde_json::from_str::<SampleStride>("0").is_err());
        assert_eq!(serde_json::from_str::<SampleStride>("3").unwrap().get(), 3);
    }
}
