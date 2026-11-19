//! Ряд значений: список допустимых значений (выдержки, ISO по 1/3 ступени, таймер).

use std::fmt::Debug;

use super::{Emphasis, Mark, Quantization, ValueError, ValueModel, nearest_index};

/// Пределы ряда: индексы первого и последнего допустимого значения, включительно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeriesLimits {
    pub first_index: usize,
    pub last_index: usize,
}

impl SeriesLimits {
    /// Весь ряд длиной `length`.
    pub fn whole(length: usize) -> Result<Self, ValueError> {
        match length.checked_sub(1) {
            Some(last_index) => Ok(Self {
                first_index: 0,
                last_index,
            }),
            None => Err(ValueError::InvalidLimits {
                reason: "ряд пуст"
            }),
        }
    }
}

/// Ряд значений. Положение = индекс в ряду (абсолютный).
#[derive(Debug, Clone, PartialEq)]
pub struct ValueSeries<T> {
    values: Vec<T>,
    reference_indices: Vec<usize>,
    limits: SeriesLimits,
}

impl<T: Copy + PartialEq + Debug> ValueSeries<T> {
    /// Создать ряд. Ошибки: ряд пуст; в нём повторы (значение должно находиться однозначно);
    /// опорный индекс или пределы выходят за ряд.
    pub fn new(values: Vec<T>, reference_indices: Vec<usize>, limits: SeriesLimits) -> Result<Self, ValueError> {
        if values.is_empty() {
            return Err(ValueError::InvalidSpec {
                name: "values",
                reason: "ряд пуст",
            });
        }
        let has_duplicates = values
            .iter()
            .enumerate()
            .any(|(index, value)| values[..index].contains(value));
        if has_duplicates {
            return Err(ValueError::InvalidSpec {
                name: "values",
                reason: "в ряду есть повторяющиеся значения",
            });
        }
        if reference_indices.iter().any(|index| *index >= values.len()) {
            return Err(ValueError::InvalidSpec {
                name: "reference_indices",
                reason: "опорный индекс вне ряда",
            });
        }
        Self::validate_limits(values.len(), limits)?;
        Ok(Self {
            values,
            reference_indices,
            limits,
        })
    }

    /// Пределы «от значения до значения» (оба обязаны быть в ряду; `lowest` стоит в ряду раньше `highest`).
    pub fn limits_between(&self, lowest: T, highest: T) -> Result<SeriesLimits, ValueError> {
        Ok(SeriesLimits {
            first_index: self.index_in_series(lowest)?,
            last_index: self.index_in_series(highest)?,
        })
    }

    /// Все значения ряда.
    pub fn values(&self) -> &[T] {
        &self.values
    }

    fn validate_limits(length: usize, limits: SeriesLimits) -> Result<(), ValueError> {
        if limits.first_index > limits.last_index {
            return Err(ValueError::InvalidLimits {
                reason: "первый индекс позже последнего",
            });
        }
        if limits.last_index >= length {
            return Err(ValueError::InvalidLimits {
                reason: "предел выходит за ряд",
            });
        }
        Ok(())
    }

    fn index_in_series(&self, value: T) -> Result<usize, ValueError> {
        self.values
            .iter()
            .position(|candidate| *candidate == value)
            .ok_or(ValueError::NotInSeries)
    }
}

impl<T: Copy + PartialEq + Debug> ValueModel for ValueSeries<T> {
    type Value = T;
    type Limits = SeriesLimits;

    fn limits(&self) -> &SeriesLimits {
        &self.limits
    }

    fn set_limits(&mut self, limits: SeriesLimits) -> Result<(), ValueError> {
        Self::validate_limits(self.values.len(), limits)?;
        self.limits = limits;
        Ok(())
    }

    fn quantization(&self) -> Quantization {
        Quantization::Stepped
    }

    fn position_range(&self) -> (f64, f64) {
        (self.limits.first_index as f64, self.limits.last_index as f64)
    }

    fn clamp_value(&self, value: T) -> Result<T, ValueError> {
        let index = self
            .index_in_series(value)?
            .clamp(self.limits.first_index, self.limits.last_index);
        Ok(self.values[index])
    }

    fn position_of(&self, value: T) -> Result<f64, ValueError> {
        let index = self
            .index_in_series(value)?
            .clamp(self.limits.first_index, self.limits.last_index);
        Ok(index as f64)
    }

    fn value_at(&self, position: f64) -> T {
        let index = nearest_index(position, self.limits.first_index as i64, self.limits.last_index as i64);
        // Индекс зажат в пределы, а пределы проверены против длины ряда.
        self.values[index as usize]
    }

    fn marks(&self) -> Vec<Mark> {
        (self.limits.first_index..=self.limits.last_index)
            .map(|index| Mark {
                position: index as f64,
                emphasis: if self.reference_indices.contains(&index) {
                    Emphasis::Reference
                } else {
                    Emphasis::Tick
                },
            })
            .collect()
    }
}
