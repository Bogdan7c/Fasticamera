//! Полоски Lift / Gamma / Gain / Offset по каналам R, G, B (решения 339–343).
//!
//! Здесь только словарь: 12 ручек и их адресация. Формулу (кодирование с «носком», Gamma, Lift/Gain, Offset) и
//! выбор основы каналов решает `isp-model` (`look::primaries`), экран — вид BARS в `camera-screen`.
//!
//! Зоны — те же [`WheelZone`], что у колёс: вкладки LIFT · GAMMA · GAIN · OFFSET общие (решение 343).
//! Каждая ручка — обычный [`LookKnob`] в [−1, 1]; полосок яркости Y нет (решение 339).

use serde::{Deserialize, Serialize};

use crate::knob::LookKnob;
use crate::wheels::WheelZone;

/// Канал полоски. Основные цвета Display P3 (решение 342) или рабочего пространства — это решает конфиг модели.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PrimaryChannel {
    /// Красный.
    Red,
    /// Зелёный.
    Green,
    /// Синий.
    Blue,
}

impl PrimaryChannel {
    /// Все каналы в порядке показа на экране и в отпечатке рецепта: R, G, B.
    pub const ALL: [PrimaryChannel; 3] = [PrimaryChannel::Red, PrimaryChannel::Green, PrimaryChannel::Blue];
}

/// Три ручки одной зоны: по одной на канал.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelTriple {
    /// Красный канал.
    pub red: LookKnob,
    /// Зелёный канал.
    pub green: LookKnob,
    /// Синий канал.
    pub blue: LookKnob,
}

impl ChannelTriple {
    /// Все три ручки в нуле.
    pub const NEUTRAL: ChannelTriple = ChannelTriple {
        red: LookKnob::NEUTRAL,
        green: LookKnob::NEUTRAL,
        blue: LookKnob::NEUTRAL,
    };

    /// Ручка канала.
    pub fn get(&self, channel: PrimaryChannel) -> LookKnob {
        match channel {
            PrimaryChannel::Red => self.red,
            PrimaryChannel::Green => self.green,
            PrimaryChannel::Blue => self.blue,
        }
    }

    /// Копия, в которой ручка канала заменена; остальные каналы не меняются.
    pub fn with(mut self, channel: PrimaryChannel, value: LookKnob) -> Self {
        match channel {
            PrimaryChannel::Red => self.red = value,
            PrimaryChannel::Green => self.green = value,
            PrimaryChannel::Blue => self.blue = value,
        }
        self
    }

    /// Все три ручки в нуле.
    pub fn is_neutral(&self) -> bool {
        PrimaryChannel::ALL
            .iter()
            .all(|&channel| self.get(channel).is_neutral())
    }
}

/// Все 12 полосок вида: четыре зоны по три канала.
///
/// В файле — вложенные объекты `{"lift":{"red":0,"green":0,"blue":0}, ...}`. Поле в
/// [`LookSettings`](crate::LookSettings) обязательное, как у колёс: файл схемы 3 переводит шаг миграции `look-presets`.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelPrimaries {
    /// Тени.
    pub lift: ChannelTriple,
    /// Середина.
    pub gamma: ChannelTriple,
    /// Света.
    pub gain: ChannelTriple,
    /// Весь канал.
    pub offset: ChannelTriple,
}

impl ChannelPrimaries {
    /// Ни одна полоска не тронута.
    pub const NEUTRAL: ChannelPrimaries = ChannelPrimaries {
        lift: ChannelTriple::NEUTRAL,
        gamma: ChannelTriple::NEUTRAL,
        gain: ChannelTriple::NEUTRAL,
        offset: ChannelTriple::NEUTRAL,
    };

    /// Все 12 полосок в нуле.
    pub fn is_neutral(&self) -> bool {
        WheelZone::ALL.iter().all(|&zone| self.zone(zone).is_neutral())
    }

    /// Три полоски зоны.
    pub fn zone(&self, zone: WheelZone) -> &ChannelTriple {
        match zone {
            WheelZone::Lift => &self.lift,
            WheelZone::Gamma => &self.gamma,
            WheelZone::Gain => &self.gain,
            WheelZone::Offset => &self.offset,
        }
    }

    /// Значение одной полоски.
    pub fn value(&self, zone: WheelZone, channel: PrimaryChannel) -> LookKnob {
        self.zone(zone).get(channel)
    }

    /// Копия, в которой одна полоска заменена; остальные 11 не меняются.
    pub fn with_value(mut self, zone: WheelZone, channel: PrimaryChannel, value: LookKnob) -> Self {
        let triple = match zone {
            WheelZone::Lift => &mut self.lift,
            WheelZone::Gamma => &mut self.gamma,
            WheelZone::Gain => &mut self.gain,
            WheelZone::Offset => &mut self.offset,
        };
        *triple = triple.with(channel, value);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Разные ненулевые значения по номеру полоски: перепутанные поля дают разные числа.
    fn distinct_value(index: usize) -> LookKnob {
        LookKnob::new(0.05 * (index as f32 + 1.0)).unwrap()
    }

    #[test]
    fn default_equals_neutral_and_is_neutral() {
        assert_eq!(ChannelPrimaries::default(), ChannelPrimaries::NEUTRAL);
        assert!(ChannelPrimaries::NEUTRAL.is_neutral());
    }

    #[test]
    fn every_single_bar_breaks_neutrality_and_touches_only_itself() {
        for zone in WheelZone::ALL {
            for channel in PrimaryChannel::ALL {
                let changed = ChannelPrimaries::NEUTRAL.with_value(zone, channel, distinct_value(0));
                assert!(!changed.is_neutral(), "{zone:?}/{channel:?}");
                for other_zone in WheelZone::ALL {
                    for other_channel in PrimaryChannel::ALL {
                        let expected = if (other_zone, other_channel) == (zone, channel) {
                            distinct_value(0)
                        } else {
                            LookKnob::NEUTRAL
                        };
                        assert_eq!(
                            changed.value(other_zone, other_channel),
                            expected,
                            "{zone:?}/{channel:?} задел {other_zone:?}/{other_channel:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn all_twelve_bars_roundtrip_through_get_and_set_with_distinct_values() {
        let mut primaries = ChannelPrimaries::NEUTRAL;
        let mut index = 0;
        for zone in WheelZone::ALL {
            for channel in PrimaryChannel::ALL {
                primaries = primaries.with_value(zone, channel, distinct_value(index));
                index += 1;
            }
        }
        let mut index = 0;
        for zone in WheelZone::ALL {
            for channel in PrimaryChannel::ALL {
                assert_eq!(
                    primaries.value(zone, channel),
                    distinct_value(index),
                    "{zone:?}/{channel:?}"
                );
                index += 1;
            }
        }
        // Соответствие именам полей: порядок ALL = порядок полей.
        assert_eq!(primaries.lift.red, distinct_value(0));
        assert_eq!(primaries.offset.blue, distinct_value(11));
    }

    #[test]
    fn serde_roundtrip_uses_named_channels_and_rejects_unknown_or_missing_fields() {
        let primaries = ChannelPrimaries::NEUTRAL.with_value(WheelZone::Gain, PrimaryChannel::Green, distinct_value(3));
        let json = serde_json::to_value(primaries).unwrap();
        assert_eq!(json["gain"]["green"], serde_json::json!(0.2f32));
        assert_eq!(json["lift"]["red"], serde_json::json!(0.0));
        let again: ChannelPrimaries = serde_json::from_value(json).unwrap();
        assert_eq!(again, primaries);

        let unknown = r#"{"lift":{"red":0,"green":0,"blue":0,"luma":0},"gamma":{"red":0,"green":0,"blue":0},
            "gain":{"red":0,"green":0,"blue":0},"offset":{"red":0,"green":0,"blue":0}}"#;
        assert!(serde_json::from_str::<ChannelPrimaries>(unknown).is_err());
        let missing_zone = r#"{"lift":{"red":0,"green":0,"blue":0}}"#;
        assert!(serde_json::from_str::<ChannelPrimaries>(missing_zone).is_err());
        let out_of_range = r#"{"lift":{"red":1.5,"green":0,"blue":0},"gamma":{"red":0,"green":0,"blue":0},
            "gain":{"red":0,"green":0,"blue":0},"offset":{"red":0,"green":0,"blue":0}}"#;
        assert!(serde_json::from_str::<ChannelPrimaries>(out_of_range).is_err());
    }
}
