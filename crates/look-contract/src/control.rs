//! Органы управления видом как данные: «какая именно ручка» без строк и без копирования кода на каждое поле.
//!
//! Нужны трём потребителям: сбросу одной ручки к значению пресета (решение 196), подсветке «эта ручка
//! отличается от пресета» и списку ручек, которые текущая основа не умеет. Все они адресуют ручку одним
//! значением [`LookControl`].

use crate::bands::{BandAdjustment, HueBandId};
use crate::knob::LookKnob;
use crate::settings::LookSettings;
use crate::wheels::WheelZone;

/// Одна из восьми глобальных ручек [`LookSettings`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlobalKnob {
    /// Контраст.
    Contrast,
    /// Средние тона.
    Midtones,
    /// Тени.
    Shadows,
    /// Света.
    Highlights,
    /// Насыщенность.
    Saturation,
    /// Сочность.
    Vibrance,
    /// Теплота.
    Warmth,
    /// Оттенок (зелёный ↔ пурпурный).
    Tint,
}

impl GlobalKnob {
    /// Все глобальные ручки в порядке полей [`LookSettings`].
    pub const ALL: [GlobalKnob; 8] = [
        GlobalKnob::Contrast,
        GlobalKnob::Midtones,
        GlobalKnob::Shadows,
        GlobalKnob::Highlights,
        GlobalKnob::Saturation,
        GlobalKnob::Vibrance,
        GlobalKnob::Warmth,
        GlobalKnob::Tint,
    ];
}

/// Одна из трёх ручек полосы оттенка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BandAxis {
    /// Сдвиг оттенка.
    Hue,
    /// Насыщенность полосы.
    Saturation,
    /// Яркость полосы.
    Luminance,
}

impl BandAxis {
    /// Все оси полосы.
    pub const ALL: [BandAxis; 3] = [BandAxis::Hue, BandAxis::Saturation, BandAxis::Luminance];
}

/// Какую часть колеса цвета имеем в виду.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WheelPart {
    /// Только толчок цвета (точка в круге).
    Push,
    /// Только кольцо яркости.
    Ring,
    /// Колесо целиком: и толчок, и кольцо.
    Whole,
}

/// Адрес одного органа управления рецепта. Полоса адресуется по идентификатору, поэтому тип не знает,
/// какие полосы существуют (это знает конфиг модели вида).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum LookControl {
    /// Модель основы тона.
    ToneBase,
    /// Глобальная ручка.
    Knob(GlobalKnob),
    /// Одна ось одной полосы оттенка.
    Band {
        /// Какая полоса.
        band: HueBandId,
        /// Какая ось полосы.
        axis: BandAxis,
    },
    /// Часть колеса цвета зоны.
    Wheel {
        /// Какое колесо.
        zone: WheelZone,
        /// Какая часть колеса.
        part: WheelPart,
    },
}

impl LookSettings {
    /// Значение глобальной ручки.
    pub fn knob(&self, knob: GlobalKnob) -> LookKnob {
        match knob {
            GlobalKnob::Contrast => self.contrast,
            GlobalKnob::Midtones => self.midtones,
            GlobalKnob::Shadows => self.shadows,
            GlobalKnob::Highlights => self.highlights,
            GlobalKnob::Saturation => self.saturation,
            GlobalKnob::Vibrance => self.vibrance,
            GlobalKnob::Warmth => self.warmth,
            GlobalKnob::Tint => self.tint,
        }
    }

    /// Копия, в которой одна глобальная ручка заменена; остальное не меняется.
    pub fn with_knob(mut self, knob: GlobalKnob, value: LookKnob) -> LookSettings {
        match knob {
            GlobalKnob::Contrast => self.contrast = value,
            GlobalKnob::Midtones => self.midtones = value,
            GlobalKnob::Shadows => self.shadows = value,
            GlobalKnob::Highlights => self.highlights = value,
            GlobalKnob::Saturation => self.saturation = value,
            GlobalKnob::Vibrance => self.vibrance = value,
            GlobalKnob::Warmth => self.warmth = value,
            GlobalKnob::Tint => self.tint = value,
        }
        self
    }
}

impl BandAdjustment {
    /// Значение одной оси полосы.
    pub fn axis(&self, axis: BandAxis) -> LookKnob {
        match axis {
            BandAxis::Hue => self.hue,
            BandAxis::Saturation => self.saturation,
            BandAxis::Luminance => self.luminance,
        }
    }

    /// Копия, в которой одна ось заменена; остальные оси не меняются.
    pub fn with_axis(mut self, axis: BandAxis, value: LookKnob) -> BandAdjustment {
        match axis {
            BandAxis::Hue => self.hue = value,
            BandAxis::Saturation => self.saturation = value,
            BandAxis::Luminance => self.luminance = value,
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Разные ненулевые значения по номеру ручки: перепутанные поля дают разные числа.
    fn distinct_value(index: usize) -> LookKnob {
        LookKnob::new(0.1 * (index as f32 + 1.0)).unwrap()
    }

    #[test]
    fn knob_get_and_set_roundtrip_for_all_eight_knobs() {
        let mut settings = LookSettings::NEUTRAL;
        for (index, knob) in GlobalKnob::ALL.into_iter().enumerate() {
            settings = settings.with_knob(knob, distinct_value(index));
        }
        for (index, knob) in GlobalKnob::ALL.into_iter().enumerate() {
            assert_eq!(settings.knob(knob), distinct_value(index), "{knob:?}");
        }
        // Соответствие полям: порядок ALL = порядок полей, без перепутанных ручек.
        assert_eq!(settings.contrast, distinct_value(0));
        assert_eq!(settings.tint, distinct_value(7));
    }

    #[test]
    fn with_knob_touches_only_that_knob() {
        for knob in GlobalKnob::ALL {
            let changed = LookSettings::NEUTRAL.with_knob(knob, distinct_value(3));
            for other in GlobalKnob::ALL {
                let expected = if other == knob {
                    distinct_value(3)
                } else {
                    LookKnob::NEUTRAL
                };
                assert_eq!(changed.knob(other), expected, "{knob:?} / {other:?}");
            }
            assert!(changed.bands.is_neutral() && changed.wheels.is_neutral());
        }
    }

    #[test]
    fn band_axis_get_and_set_roundtrip() {
        let sky = HueBandId::new("sky").unwrap();
        for (index, axis) in BandAxis::ALL.into_iter().enumerate() {
            let band = BandAdjustment::neutral(sky.clone()).with_axis(axis, distinct_value(index));
            for other in BandAxis::ALL {
                let expected = if other == axis {
                    distinct_value(index)
                } else {
                    LookKnob::NEUTRAL
                };
                assert_eq!(band.axis(other), expected, "{axis:?} / {other:?}");
            }
        }
    }
}
