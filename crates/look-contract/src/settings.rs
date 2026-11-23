//! Полный набор ручек вида: то, что экран крутит, а `isp-model` переводит в коэффициенты.

use serde::{Deserialize, Serialize};

use crate::bands::BandAdjustments;
use crate::knob::LookKnob;

/// Значения всех ручек вида. Снимок получает именно эти значения «на момент нажатия»
/// (решение 99), а не ссылку на пресет.
///
/// Нейтраль — все ручки в нуле и полосы не тронуты: это «Честный» (решение 98),
/// на нём картинка обязана совпасть бит в бит с основой без слоя вида.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookSettings {
    /// Контраст.
    pub contrast: LookKnob,
    /// Средние тона.
    pub midtones: LookKnob,
    /// Тени.
    pub shadows: LookKnob,
    /// Света.
    pub highlights: LookKnob,
    /// Насыщенность.
    pub saturation: LookKnob,
    /// Сочность: насыщенность с весом против уже насыщенного и против кожи.
    pub vibrance: LookKnob,
    /// Теплота: «+» — картинка теплее.
    pub warmth: LookKnob,
    /// Оттенок (зелёный ↔ пурпурный).
    pub tint: LookKnob,
    /// Ручки по полосам оттенка.
    #[serde(default)]
    pub bands: BandAdjustments,
}

impl LookSettings {
    /// Всё в нуле.
    pub const NEUTRAL: LookSettings = LookSettings {
        contrast: LookKnob::NEUTRAL,
        midtones: LookKnob::NEUTRAL,
        shadows: LookKnob::NEUTRAL,
        highlights: LookKnob::NEUTRAL,
        saturation: LookKnob::NEUTRAL,
        vibrance: LookKnob::NEUTRAL,
        warmth: LookKnob::NEUTRAL,
        tint: LookKnob::NEUTRAL,
        bands: BandAdjustments::NONE,
    };

    /// Слой вида ничего не меняет: каждая ручка и каждая полоса в нуле.
    pub fn is_neutral(&self) -> bool {
        self.contrast.is_neutral()
            && self.midtones.is_neutral()
            && self.shadows.is_neutral()
            && self.highlights.is_neutral()
            && self.saturation.is_neutral()
            && self.vibrance.is_neutral()
            && self.warmth.is_neutral()
            && self.tint.is_neutral()
            && self.bands.is_neutral()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bands::{BandAdjustment, HueBandId};

    #[test]
    fn default_equals_neutral_and_is_neutral() {
        assert_eq!(LookSettings::default(), LookSettings::NEUTRAL);
        assert!(LookSettings::NEUTRAL.is_neutral());
    }

    #[test]
    fn touching_any_single_knob_breaks_neutrality() {
        let one = LookKnob::new(0.1).unwrap();
        let touched = [
            LookSettings {
                contrast: one,
                ..LookSettings::NEUTRAL
            },
            LookSettings {
                midtones: one,
                ..LookSettings::NEUTRAL
            },
            LookSettings {
                shadows: one,
                ..LookSettings::NEUTRAL
            },
            LookSettings {
                highlights: one,
                ..LookSettings::NEUTRAL
            },
            LookSettings {
                saturation: one,
                ..LookSettings::NEUTRAL
            },
            LookSettings {
                vibrance: one,
                ..LookSettings::NEUTRAL
            },
            LookSettings {
                warmth: one,
                ..LookSettings::NEUTRAL
            },
            LookSettings {
                tint: one,
                ..LookSettings::NEUTRAL
            },
        ];
        for settings in touched {
            assert!(!settings.is_neutral(), "{settings:?}");
        }
    }

    #[test]
    fn touched_band_breaks_neutrality() {
        let band = BandAdjustment {
            hue: LookKnob::new(0.2).unwrap(),
            ..BandAdjustment::neutral(HueBandId::new("sky").unwrap())
        };
        let settings = LookSettings {
            bands: BandAdjustments::new(vec![band]).unwrap(),
            ..LookSettings::NEUTRAL
        };
        assert!(!settings.is_neutral());
    }

    #[test]
    fn serde_roundtrip_and_missing_bands_default_to_none() {
        let json = r#"{"contrast":0.2,"midtones":0,"shadows":0,"highlights":0,
            "saturation":0.1,"vibrance":0,"warmth":0,"tint":0}"#;
        let settings: LookSettings = serde_json::from_str(json).unwrap();
        assert_eq!(settings.contrast.get(), 0.2);
        assert!(settings.bands.is_neutral());
        let again: LookSettings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(again, settings);
    }

    #[test]
    fn serde_rejects_out_of_range_unknown_field_and_missing_knob() {
        let out_of_range = r#"{"contrast":1.5,"midtones":0,"shadows":0,"highlights":0,
            "saturation":0,"vibrance":0,"warmth":0,"tint":0}"#;
        assert!(serde_json::from_str::<LookSettings>(out_of_range).is_err());
        let unknown = r#"{"contrast":0,"midtones":0,"shadows":0,"highlights":0,
            "saturation":0,"vibrance":0,"warmth":0,"tint":0,"sharpness":0.3}"#;
        assert!(serde_json::from_str::<LookSettings>(unknown).is_err());
        assert!(serde_json::from_str::<LookSettings>(r#"{"contrast":0}"#).is_err());
    }
}
