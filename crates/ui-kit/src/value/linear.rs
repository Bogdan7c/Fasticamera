//! Квантованная линейная модель: диапазон, шаг, «ноль» (EV ±2 по 1/3).

use super::{
    DEFAULT_MAX_MARKS, Emphasis, INDEX_EPSILON, MAX_EXACT_INDEX, Mark, Quantization, ValueError, ValueModel,
    ensure_finite, ensure_mark_budget, nearest_index,
};

/// Опорное значение линейной шкалы (крупный упор).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReferenceValue {
    /// Опорного значения нет.
    None,
    /// Опорное значение (обычно «ноль»); обязано лежать на сетке шага.
    At(f64),
}

/// Пределы линейной модели, включительно. Границы, не попадающие на сетку, сужаются к ближайшему
/// шагу внутрь: предел 0,5 при шаге 1/3 даёт верхний шаг 1/3.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LinearLimits {
    pub min: f64,
    pub max: f64,
}

/// Линейная модель со ступенями. Положение = индекс шага, значение = `индекс × шаг`.
#[derive(Debug, Clone, PartialEq)]
pub struct QuantizedLinear {
    step: f64,
    reference_index: Option<i64>,
    limits: LinearLimits,
    min_index: i64,
    max_index: i64,
    /// Предел числа рисок (инженерная защита, см. [`DEFAULT_MAX_MARKS`]).
    max_marks: u32,
}

impl QuantizedLinear {
    /// Создать модель с пределом рисок [`DEFAULT_MAX_MARKS`]. Ошибки: шаг не конечный или <= 0;
    /// опорное значение не на сетке; пределы пусты или дают слишком много шагов.
    pub fn new(step: f64, reference: ReferenceValue, limits: LinearLimits) -> Result<Self, ValueError> {
        Self::with_max_marks(step, reference, limits, DEFAULT_MAX_MARKS)
    }

    /// То же со своим пределом числа рисок: пределы, дающие больше шагов, — ошибка
    /// [`ValueError::TooManyMarks`], шкала не создаётся.
    pub fn with_max_marks(
        step: f64,
        reference: ReferenceValue,
        limits: LinearLimits,
        max_marks: u32,
    ) -> Result<Self, ValueError> {
        ensure_finite(step)?;
        if step <= 0.0 {
            return Err(ValueError::InvalidSpec {
                name: "step",
                reason: "шаг должен быть > 0",
            });
        }
        let reference_index = match reference {
            ReferenceValue::None => None,
            ReferenceValue::At(value) => {
                ensure_finite(value)?;
                let scaled = value / step;
                if scaled.abs() > MAX_EXACT_INDEX {
                    return Err(ValueError::InvalidSpec {
                        name: "reference",
                        reason: "опорное значение слишком велико для шага",
                    });
                }
                if (scaled - scaled.round()).abs() > INDEX_EPSILON {
                    return Err(ValueError::InvalidSpec {
                        name: "reference",
                        reason: "опорное значение не лежит на сетке шага",
                    });
                }
                Some(scaled.round() as i64)
            }
        };
        let (min_index, max_index) = Self::index_range(step, limits, max_marks)?;
        Ok(Self {
            step,
            reference_index,
            limits,
            min_index,
            max_index,
            max_marks,
        })
    }

    /// Шаг модели.
    pub fn step(&self) -> f64 {
        self.step
    }

    /// Индекс ближайшего шага к значению (после зажима в пределы).
    pub fn index_of(&self, value: f64) -> Result<i64, ValueError> {
        ensure_finite(value)?;
        Ok(nearest_index(value / self.step, self.min_index, self.max_index))
    }

    /// Значение шага по индексу. Всегда `индекс × шаг`: сумма дробей нигде не накапливается.
    pub fn value_of_index(&self, index: i64) -> f64 {
        index as f64 * self.step
    }

    /// Индексы крайних шагов. Всё проверяется в `f64` **до** перевода в `i64`: огромные пределы
    /// не насыщают `as i64` молча, а дают ошибку.
    fn index_range(step: f64, limits: LinearLimits, max_marks: u32) -> Result<(i64, i64), ValueError> {
        ensure_finite(limits.min)?;
        ensure_finite(limits.max)?;
        // Допуск: предел 2,0 при шаге 1/3 — это ровно шаг 6, а не 5.999…
        let first = (limits.min / step - INDEX_EPSILON).ceil();
        let last = (limits.max / step + INDEX_EPSILON).floor();
        if first > last {
            return Err(ValueError::InvalidLimits {
                reason: "между min и max нет ни одного шага",
            });
        }
        ensure_mark_budget(last - first + 1.0, max_marks)?;
        if first.abs() > MAX_EXACT_INDEX || last.abs() > MAX_EXACT_INDEX {
            return Err(ValueError::InvalidLimits {
                reason: "пределы слишком велики для шага",
            });
        }
        Ok((first as i64, last as i64))
    }
}

/// Шаг риски квантованной модели — одна единица положения (одна ступень).
impl super::RulerAxis for QuantizedLinear {
    fn tick_step_units(&self) -> f64 {
        1.0
    }
}

impl ValueModel for QuantizedLinear {
    type Value = f64;
    type Limits = LinearLimits;

    fn limits(&self) -> &LinearLimits {
        &self.limits
    }

    fn set_limits(&mut self, limits: LinearLimits) -> Result<(), ValueError> {
        let (min_index, max_index) = Self::index_range(self.step, limits, self.max_marks)?;
        self.limits = limits;
        self.min_index = min_index;
        self.max_index = max_index;
        Ok(())
    }

    fn quantization(&self) -> Quantization {
        Quantization::Stepped
    }

    fn position_range(&self) -> (f64, f64) {
        (self.min_index as f64, self.max_index as f64)
    }

    fn clamp_value(&self, value: f64) -> Result<f64, ValueError> {
        Ok(self.value_of_index(self.index_of(value)?))
    }

    fn position_of(&self, value: f64) -> Result<f64, ValueError> {
        Ok(self.index_of(value)? as f64)
    }

    fn value_at(&self, position: f64) -> f64 {
        self.value_of_index(nearest_index(position, self.min_index, self.max_index))
    }

    fn marks(&self) -> Vec<Mark> {
        (self.min_index..=self.max_index)
            .map(|index| Mark {
                position: index as f64,
                emphasis: if self.reference_index == Some(index) {
                    Emphasis::Reference
                } else {
                    Emphasis::Tick
                },
            })
            .collect()
    }
}
