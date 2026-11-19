//! Две оси: композиция двух одномерных моделей (кельвины + оттенок).
//!
//! Двумерной шкалы нет: пара — это две независимые одномерные шкалы (по одной [`crate::scale::SnapScale`]
//! на ось), а здесь — общее значение пары и атомарная смена пределов обеих осей.

use super::{ValueError, ValueModel};

/// Значение пары осей.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PairValue<A, B> {
    pub primary: A,
    pub secondary: B,
}

/// Пара независимых моделей.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisPair<P: ValueModel, S: ValueModel> {
    primary: P,
    secondary: S,
}

impl<P: ValueModel, S: ValueModel> AxisPair<P, S> {
    pub fn new(primary: P, secondary: S) -> Self {
        Self { primary, secondary }
    }

    pub fn primary(&self) -> &P {
        &self.primary
    }

    pub fn secondary(&self) -> &S {
        &self.secondary
    }

    /// Применить правило «вне пределов» к каждой оси отдельно.
    pub fn clamp_pair(
        &self,
        value: PairValue<P::Value, S::Value>,
    ) -> Result<PairValue<P::Value, S::Value>, ValueError> {
        Ok(PairValue {
            primary: self.primary.clamp_value(value.primary)?,
            secondary: self.secondary.clamp_value(value.secondary)?,
        })
    }

    /// Сменить пределы обеих осей **атомарно**: если вторая ось отвергла пределы, первая
    /// возвращается к прежним, пара остаётся как была.
    pub fn set_limits(&mut self, primary: P::Limits, secondary: S::Limits) -> Result<(), ValueError> {
        let previous_primary = self.primary.limits().clone();
        self.primary.set_limits(primary)?;
        if let Err(error) = self.secondary.set_limits(secondary) {
            // Прежние пределы уже были приняты этой моделью, поэтому откат не может не удаться.
            self.primary.set_limits(previous_primary)?;
            return Err(error);
        }
        Ok(())
    }

    /// Разобрать на две модели — по одной на шкалу.
    pub fn into_axes(self) -> (P, S) {
        (self.primary, self.secondary)
    }
}
