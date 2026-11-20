//! Логарифмическая модель: диапазон, опорные точки (кратность 0,5…2 с упорами 0,5 / 1 / 2).

use super::{
    DEFAULT_MAX_MARKS, Emphasis, INDEX_EPSILON, MAX_EXACT_INDEX, Mark, Quantization, ValueError, ValueModel,
    ensure_finite, ensure_mark_budget,
};

/// Расстояние (в единицах), ближе которого тик сливается с опорной точкой.
const MARK_MERGE_EPSILON: f64 = 1e-6;

/// Пределы логарифмической модели, включительно; `0 < min < max`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LogLimits {
    pub min: f64,
    pub max: f64,
}

/// Неизменяемое описание модели (то, что не зависит от пределов камеры).
#[derive(Debug, Clone, PartialEq)]
pub struct LogarithmicSpec {
    /// Единиц положения на октаву (удвоение значения). Вместе с токеном плотности задаёт длину шкалы.
    pub units_per_octave: f64,
    /// Шаг рисок в единицах положения.
    pub tick_spacing_units: f64,
    /// Опорные значения (крупные упоры). Вне пределов не показываются, но запоминаются: после
    /// смены камеры они снова появятся.
    pub anchors: Vec<f64>,
}

/// Логарифмическая модель. Положение = `log2(значение) × единиц на октаву` (абсолютное, `1 → 0`).
#[derive(Debug, Clone, PartialEq)]
pub struct Logarithmic {
    spec: LogarithmicSpec,
    limits: LogLimits,
    /// Предел числа рисок (инженерная защита, см. [`DEFAULT_MAX_MARKS`]).
    max_marks: u32,
    /// Риски в заданных значениях (а не равномерно по логарифму). `None` — равномерная сетка
    /// `tick_spacing_units`. Нужна колесу зума: риски через 0,1× — равномерны по значению, а не по октаве.
    tick_values: Option<Vec<f64>>,
}

impl Logarithmic {
    /// Создать модель с пределом рисок [`DEFAULT_MAX_MARKS`]. Ошибки: числа не конечны или <= 0;
    /// якорь <= 0; пределы не `0 < min < max`; пределы и шаг рисок дают слишком много рисок.
    pub fn new(spec: LogarithmicSpec, limits: LogLimits) -> Result<Self, ValueError> {
        Self::with_max_marks(spec, limits, DEFAULT_MAX_MARKS)
    }

    /// То же со своим пределом числа рисок ([`ValueError::TooManyMarks`] при превышении).
    pub fn with_max_marks(spec: LogarithmicSpec, limits: LogLimits, max_marks: u32) -> Result<Self, ValueError> {
        for (name, value) in [
            ("units_per_octave", spec.units_per_octave),
            ("tick_spacing_units", spec.tick_spacing_units),
        ] {
            ensure_finite(value)?;
            if value <= 0.0 {
                return Err(ValueError::InvalidSpec {
                    name,
                    reason: "должно быть > 0",
                });
            }
        }
        for anchor in &spec.anchors {
            ensure_finite(*anchor)?;
            if *anchor <= 0.0 {
                return Err(ValueError::InvalidSpec {
                    name: "anchors",
                    reason: "опорное значение логарифмической шкалы должно быть > 0",
                });
            }
        }
        Self::validate_limits(&spec, limits, max_marks)?;
        Ok(Self {
            spec,
            limits,
            max_marks,
            tick_values: None,
        })
    }

    /// Расставить обычные риски в заданных **значениях** (а не равномерно по логарифму): рисунок рисок и
    /// рубежи хаптики колеса берутся из одного `marks()`, поэтому правило «риски через 0,1×» живёт здесь.
    /// Значения вне пределов в `marks()` не попадают (но запоминаются: после смены камеры вернутся).
    /// Риски, совпавшие с опорной точкой, сливаются с ней (остаётся опорная).
    /// Ошибки: значение не конечно или <= 0; рисок больше предела.
    pub fn with_tick_values(mut self, values: Vec<f64>) -> Result<Self, ValueError> {
        for value in &values {
            ensure_finite(*value)?;
            if *value <= 0.0 {
                return Err(ValueError::InvalidSpec {
                    name: "tick_values",
                    reason: "риска логарифмической шкалы должна быть > 0",
                });
            }
        }
        ensure_mark_budget(values.len() as f64, self.max_marks)?;
        self.tick_values = Some(values);
        Ok(self)
    }

    fn validate_limits(spec: &LogarithmicSpec, limits: LogLimits, max_marks: u32) -> Result<(), ValueError> {
        ensure_finite(limits.min)?;
        ensure_finite(limits.max)?;
        if limits.min <= 0.0 || limits.min >= limits.max {
            return Err(ValueError::InvalidLimits {
                reason: "нужно 0 < min < max",
            });
        }
        // Число рисок = длина шкалы в единицах / шаг рисок; считается до генерации рисок.
        let span_units = (limits.max.log2() - limits.min.log2()) * spec.units_per_octave;
        ensure_mark_budget(span_units / spec.tick_spacing_units + 1.0, max_marks)?;
        if limits.max.log2().abs() * spec.units_per_octave / spec.tick_spacing_units > MAX_EXACT_INDEX
            || limits.min.log2().abs() * spec.units_per_octave / spec.tick_spacing_units > MAX_EXACT_INDEX
        {
            return Err(ValueError::InvalidLimits {
                reason: "пределы слишком велики для шага рисок",
            });
        }
        Ok(())
    }

    fn position_of_unclamped(&self, value: f64) -> f64 {
        value.log2() * self.spec.units_per_octave
    }
}

impl ValueModel for Logarithmic {
    type Value = f64;
    type Limits = LogLimits;

    fn limits(&self) -> &LogLimits {
        &self.limits
    }

    fn set_limits(&mut self, limits: LogLimits) -> Result<(), ValueError> {
        Self::validate_limits(&self.spec, limits, self.max_marks)?;
        self.limits = limits;
        Ok(())
    }

    fn quantization(&self) -> Quantization {
        Quantization::Continuous
    }

    fn position_range(&self) -> (f64, f64) {
        (
            self.position_of_unclamped(self.limits.min),
            self.position_of_unclamped(self.limits.max),
        )
    }

    fn clamp_value(&self, value: f64) -> Result<f64, ValueError> {
        ensure_finite(value)?;
        if value <= 0.0 {
            // Логарифм нуля и отрицательных не определён: такое значение — не «вне пределов», а ошибка.
            return Err(ValueError::InvalidSpec {
                name: "value",
                reason: "значение логарифмической шкалы должно быть > 0",
            });
        }
        Ok(value.clamp(self.limits.min, self.limits.max))
    }

    fn position_of(&self, value: f64) -> Result<f64, ValueError> {
        Ok(self.position_of_unclamped(self.clamp_value(value)?))
    }

    fn value_at(&self, position: f64) -> f64 {
        let (lowest, highest) = self.position_range();
        if !position.is_finite() || position <= lowest {
            return self.limits.min;
        }
        if position >= highest {
            return self.limits.max;
        }
        (position / self.spec.units_per_octave).exp2()
    }

    fn marks(&self) -> Vec<Mark> {
        let (lowest, highest) = self.position_range();
        let spacing = self.spec.tick_spacing_units;
        let anchors: Vec<f64> = self
            .spec
            .anchors
            .iter()
            .filter(|anchor| (self.limits.min..=self.limits.max).contains(*anchor))
            .map(|anchor| self.position_of_unclamped(*anchor))
            .collect();
        let tick_positions: Vec<f64> = match &self.tick_values {
            Some(values) => values
                .iter()
                .filter(|value| (self.limits.min..=self.limits.max).contains(*value))
                .map(|value| self.position_of_unclamped(*value))
                .collect(),
            None => {
                let first_tick = (lowest / spacing - INDEX_EPSILON).ceil() as i64;
                let last_tick = (highest / spacing + INDEX_EPSILON).floor() as i64;
                (first_tick..=last_tick).map(|index| index as f64 * spacing).collect()
            }
        };
        let mut marks: Vec<Mark> = tick_positions
            .into_iter()
            .filter(|position| {
                anchors
                    .iter()
                    .all(|anchor| (anchor - position).abs() > MARK_MERGE_EPSILON)
            })
            .map(|position| Mark {
                position,
                emphasis: Emphasis::Tick,
            })
            .collect();
        marks.extend(anchors.into_iter().map(|position| Mark {
            position,
            emphasis: Emphasis::Reference,
        }));
        marks.sort_by(|left, right| left.position.total_cmp(&right.position));
        marks
    }
}
