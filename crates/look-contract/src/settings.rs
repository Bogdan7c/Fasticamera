//! Полный набор ручек вида: то, что экран крутит, а `isp-model` переводит в коэффициенты.

use serde::{Deserialize, Serialize};

use crate::bands::BandAdjustments;
use crate::knob::LookKnob;
use crate::wheels::ColorWheels;

/// Значения всех ручек вида. Снимок получает именно эти значения «на момент нажатия»
/// (решение 99), а не ссылку на пресет.
///
/// Нейтраль — все ручки в нуле, полосы и колёса не тронуты: это «Честный» (решение 98),
/// на нём картинка обязана совпасть бит в бит с основой без слоя вида.
///
/// Ручки и колёса — один набор (решение 193): пресет хранит их вместе, на кадре они работают вместе.
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
    /// Колёса цвета по зонам (решения 186, 191). Без `#[serde(default)]` намеренно: файл пресетов
    /// схемы 1 (до колёс) переводит шаг миграции `look-presets`, а не молчаливое умолчание словаря —
    /// иначе сломанный шаг миграции прошёл бы незамеченным.
    pub wheels: ColorWheels,
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
        wheels: ColorWheels::NEUTRAL,
    };

    /// Слой вида ничего не меняет: каждая ручка, каждая полоса и каждое колесо в нуле.
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
            && self.wheels.is_neutral()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bands::{BandAdjustment, HueBandId};
    use crate::wheels::{ColorWheel, WheelPush, WheelZone};

    /// Нейтральные колёса в JSON — для строк файла, набранных в тестах руками.
    const NEUTRAL_WHEELS_JSON: &str = r#""wheels":{"lift":{"push":[0,0],"ring":0},"gamma":{"push":[0,0],"ring":0},
        "gain":{"push":[0,0],"ring":0},"offset":{"push":[0,0],"ring":0}}"#;

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
    fn touched_wheel_breaks_neutrality_for_every_zone_and_knob() {
        let pushed = ColorWheel {
            push: WheelPush::new(0.0, -0.1).unwrap(),
            ..ColorWheel::NEUTRAL
        };
        let ringed = ColorWheel {
            ring: LookKnob::new(0.1).unwrap(),
            ..ColorWheel::NEUTRAL
        };
        for zone in WheelZone::ALL {
            for wheel in [pushed, ringed] {
                let settings = LookSettings {
                    wheels: ColorWheels::NEUTRAL.with_wheel(zone, wheel),
                    ..LookSettings::NEUTRAL
                };
                assert!(!settings.is_neutral(), "{zone:?} {wheel:?}");
                // Колёса и ручки — одно значение: сравнение по `==` видит разницу (ключ кэша кадра).
                assert_ne!(settings, LookSettings::NEUTRAL);
            }
        }
    }

    #[test]
    fn missing_wheels_is_an_error_not_a_silent_default() {
        let json = r#"{"contrast":0.2,"midtones":0,"shadows":0,"highlights":0,
            "saturation":0.1,"vibrance":0,"warmth":0,"tint":0}"#;
        let error = serde_json::from_str::<LookSettings>(json).unwrap_err();
        assert!(error.to_string().contains("wheels"), "{error}");
    }

    #[test]
    fn serde_roundtrip_and_missing_bands_default_to_none() {
        let json = format!(
            r#"{{"contrast":0.2,"midtones":0,"shadows":0,"highlights":0,
            "saturation":0.1,"vibrance":0,"warmth":0,"tint":0,{NEUTRAL_WHEELS_JSON}}}"#
        );
        let settings: LookSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(settings.contrast.get(), 0.2);
        assert!(settings.bands.is_neutral());
        let again: LookSettings = serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
        assert_eq!(again, settings);
    }

    #[test]
    fn serde_rejects_out_of_range_unknown_field_and_missing_knob() {
        let out_of_range = format!(
            r#"{{"contrast":1.5,"midtones":0,"shadows":0,"highlights":0,
            "saturation":0,"vibrance":0,"warmth":0,"tint":0,{NEUTRAL_WHEELS_JSON}}}"#
        );
        assert!(serde_json::from_str::<LookSettings>(&out_of_range).is_err());
        let unknown = format!(
            r#"{{"contrast":0,"midtones":0,"shadows":0,"highlights":0,
            "saturation":0,"vibrance":0,"warmth":0,"tint":0,{NEUTRAL_WHEELS_JSON},"sharpness":0.3}}"#
        );
        assert!(serde_json::from_str::<LookSettings>(&unknown).is_err());
        assert!(serde_json::from_str::<LookSettings>(r#"{"contrast":0}"#).is_err());
    }
}
