//! Настройки аниматора: когда считать свойство успокоившимся и нижняя граница масштаба.

use serde::{Deserialize, Serialize};

use super::error::AnimatorError;
use super::property::AnimatedProperty;
use crate::spring::RestCriterion;

/// Настройки [`super::PropertyAnimator`].
///
/// Критерий покоя `RestCriterion::default()` рассчитан на pt (пол допуска 0,01). Прозрачность и
/// масштаб — безразмерные величины порядка единицы, там 0,01 был бы заметной ступенькой, поэтому
/// у каждого свойства свой критерий.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AnimatorConfig {
    /// Покой прозрачности. `[estimate]` пол допуска 0,002 ≈ полшага 8-битного канала (1/255 / 2).
    /// Подгонка, не Apple; относительный допуск и множитель скорости — как у `RestCriterion::default()`
    /// (`motion.md` 1.2, `[doc]` 0,001).
    pub opacity_rest: RestCriterion,
    /// Покой масштаба. `[estimate]` пол 0,002 (0,2 % размера). Подгонка, не Apple.
    pub scale_rest: RestCriterion,
    /// Покой поворота, радианы. `[estimate]` пол 0,002 рад ≈ 0,11°. Подгонка, не Apple.
    pub rotation_rest: RestCriterion,
    /// Покой сдвига, pt. Как у прокрутки по умолчанию (`RestCriterion::default()`: `[doc]` 0,001 от
    /// расстояния, `[estimate]` пол 0,01 pt ≈ 1/100 px при 3x — подгонка, не Apple).
    pub translation_rest: RestCriterion,
    /// Наименьший допустимый масштаб: обратное преобразование хит-теста для нулевого масштаба не
    /// существует, а пружина к цели 0 нулём не ограничена. `[estimate]` 0,001 — подгонка, не Apple.
    /// Отражение (отрицательный масштаб) аниматором не поддерживается.
    pub min_scale: f64,
}

impl Default for AnimatorConfig {
    fn default() -> Self {
        let unit_rest = RestCriterion {
            absolute_position_floor: 0.002,
            ..RestCriterion::default()
        };
        Self {
            opacity_rest: unit_rest,
            scale_rest: unit_rest,
            rotation_rest: unit_rest,
            translation_rest: RestCriterion::default(),
            min_scale: 0.001,
        }
    }
}

impl AnimatorConfig {
    /// Проверка настроек: критерии покоя корректны, минимальный масштаб конечен и положителен.
    pub fn validate(&self) -> Result<(), AnimatorError> {
        self.opacity_rest.validate()?;
        self.scale_rest.validate()?;
        self.rotation_rest.validate()?;
        self.translation_rest.validate()?;
        if !(self.min_scale.is_finite() && self.min_scale > 0.0) {
            return Err(AnimatorError::InvalidMinScale(self.min_scale));
        }
        Ok(())
    }

    /// Критерий покоя свойства.
    pub(super) fn rest_for(&self, property: AnimatedProperty) -> RestCriterion {
        match property {
            AnimatedProperty::Opacity => self.opacity_rest,
            AnimatedProperty::ScaleX | AnimatedProperty::ScaleY => self.scale_rest,
            AnimatedProperty::Rotation => self.rotation_rest,
            AnimatedProperty::TranslationX | AnimatedProperty::TranslationY => self.translation_rest,
        }
    }
}
