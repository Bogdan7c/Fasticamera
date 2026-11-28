//! Рецепт вида: основа тона + значения ручек (решение 195).

use serde::{Deserialize, Serialize};

use crate::control::{LookControl, WheelPart};
use crate::settings::LookSettings;
use crate::tone_base::ToneBaseModel;
use crate::wheels::ColorWheel;

/// Полный рецепт пресета: модель основы и значения ручек. Это то, что хранит пресет и что правится
/// «поверх» пресета. Основа не входит в [`LookSettings`] намеренно: нейтраль настроек («чистый» вид)
/// должна означать «ручки в нуле на текущей модели», а не «ручки в нуле на какой-то модели».
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LookRecipe {
    /// Модель основы тона.
    pub base: ToneBaseModel,
    /// Значения ручек, полос и колёс.
    pub settings: LookSettings,
}

impl LookRecipe {
    /// Отличается ли значение ровно этого органа управления от `other`. Остальные органы не сравниваются.
    pub fn differs_from(&self, other: &LookRecipe, control: &LookControl) -> bool {
        // «Скопировать у другого и ничего не изменилось» ⇔ значения одинаковы: одно правило
        // для сравнения и для сброса, расхождения между ними быть не может.
        self.with_control_from(control, other) != *self
    }

    /// Копия рецепта, в которой значение ровно этого органа взято у `source`; остальные органы остаются
    /// как у `self`. Для оси полосы копируется только эта ось этой полосы (набор полос остаётся
    /// каноническим: полоса, ставшая нейтральной, пропадает).
    pub fn with_control_from(&self, control: &LookControl, source: &LookRecipe) -> LookRecipe {
        let mut result = self.clone();
        match control {
            LookControl::ToneBase => result.base = source.base,
            LookControl::Knob(knob) => {
                result.settings = result.settings.with_knob(*knob, source.settings.knob(*knob));
            }
            LookControl::Band { band, axis } => {
                let value = source.settings.bands.for_band(band).axis(*axis);
                let adjustment = result.settings.bands.for_band(band).with_axis(*axis, value);
                result.settings.bands = result.settings.bands.with_adjustment(adjustment);
            }
            LookControl::Wheel { zone, part } => {
                let own = result.settings.wheels.wheel(*zone);
                let theirs = source.settings.wheels.wheel(*zone);
                let wheel = match part {
                    WheelPart::Push => ColorWheel {
                        push: theirs.push,
                        ..own
                    },
                    WheelPart::Ring => ColorWheel {
                        ring: theirs.ring,
                        ..own
                    },
                    WheelPart::Whole => theirs,
                };
                result.settings.wheels = result.settings.wheels.with_wheel(*zone, wheel);
            }
            LookControl::Primary { zone, channel } => {
                let value = source.settings.primaries.value(*zone, *channel);
                result.settings.primaries = result.settings.primaries.with_value(*zone, *channel, value);
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bands::{BandAdjustment, BandAdjustments, HueBandId};
    use crate::control::{BandAxis, GlobalKnob};
    use crate::knob::LookKnob;
    use crate::primaries::PrimaryChannel;
    use crate::wheels::{WheelPush, WheelZone};

    fn knob(value: f32) -> LookKnob {
        LookKnob::new(value).unwrap()
    }

    fn band_id(raw: &str) -> HueBandId {
        HueBandId::new(raw).unwrap()
    }

    fn recipe(base: ToneBaseModel, settings: LookSettings) -> LookRecipe {
        LookRecipe { base, settings }
    }

    /// Рецепт, у которого ВСЕ органы управления ненейтральны; разный `seed` даёт разные значения.
    fn rich_recipe(seed: f32, base: ToneBaseModel) -> LookRecipe {
        let mut settings = LookSettings::NEUTRAL;
        for (index, knob_id) in GlobalKnob::ALL.into_iter().enumerate() {
            settings = settings.with_knob(knob_id, knob(seed + 0.01 * index as f32));
        }
        let bands = ["sky", "green"].map(|raw| BandAdjustment {
            band: band_id(raw),
            hue: knob(seed),
            saturation: knob(seed + 0.1),
            luminance: knob(seed + 0.2),
        });
        settings.bands = BandAdjustments::new(bands.to_vec()).unwrap();
        for (index, zone) in WheelZone::ALL.into_iter().enumerate() {
            let wheel = ColorWheel {
                push: WheelPush::new(seed, 0.1 * index as f32).unwrap(),
                ring: knob(seed - 0.05),
            };
            settings.wheels = settings.wheels.with_wheel(zone, wheel);
        }
        // Каждая из 12 полосок — своё значение, зависящее от seed: перепутанная зона или канал не пройдёт.
        for (zone_index, zone) in WheelZone::ALL.into_iter().enumerate() {
            for (channel_index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
                let value = knob(seed + 0.02 * (3 * zone_index + channel_index) as f32);
                settings.primaries = settings.primaries.with_value(zone, channel, value);
            }
        }
        recipe(base, settings)
    }

    /// Все органы управления, которые знает рецепт (две полосы: sky, green).
    fn all_controls() -> Vec<LookControl> {
        let mut controls = vec![LookControl::ToneBase];
        controls.extend(GlobalKnob::ALL.map(LookControl::Knob));
        for band in ["sky", "green"] {
            for axis in BandAxis::ALL {
                controls.push(LookControl::Band {
                    band: band_id(band),
                    axis,
                });
            }
        }
        for zone in WheelZone::ALL {
            for part in [WheelPart::Push, WheelPart::Ring, WheelPart::Whole] {
                controls.push(LookControl::Wheel { zone, part });
            }
            for channel in PrimaryChannel::ALL {
                controls.push(LookControl::Primary { zone, channel });
            }
        }
        controls
    }

    /// Каждый орган управления в отдельности: `with_control_from` меняет его и только его.
    #[test]
    fn with_control_from_changes_exactly_the_named_control() {
        let own = rich_recipe(0.3, ToneBaseModel::Agx);
        let source = rich_recipe(-0.4, ToneBaseModel::Aces);
        for control in all_controls() {
            let result = own.with_control_from(&control, &source);
            assert!(
                !result.differs_from(&source, &control),
                "{control:?}: значение не стало как у источника"
            );
            assert!(
                result.differs_from(&own, &control),
                "{control:?}: значение не изменилось"
            );
            for other in all_controls() {
                // Колесо целиком пересекается со своими частями: их не сравниваем как «чужие».
                if overlaps(&control, &other) {
                    continue;
                }
                assert!(
                    !result.differs_from(&own, &other),
                    "{control:?} задел чужой орган {other:?}"
                );
            }
        }
    }

    /// Совпадают ли два адреса или один из них — часть другого (колесо целиком и его части).
    fn overlaps(left: &LookControl, right: &LookControl) -> bool {
        match (left, right) {
            (
                LookControl::Wheel {
                    zone: left_zone,
                    part: left_part,
                },
                LookControl::Wheel {
                    zone: right_zone,
                    part: right_part,
                },
            ) => {
                left_zone == right_zone
                    && (left_part == right_part || *left_part == WheelPart::Whole || *right_part == WheelPart::Whole)
            }
            _ => left == right,
        }
    }

    #[test]
    fn band_axis_copy_keeps_other_axes_of_the_same_band() {
        let own = rich_recipe(0.3, ToneBaseModel::Agx);
        let source = rich_recipe(-0.4, ToneBaseModel::Agx);
        let control = LookControl::Band {
            band: band_id("sky"),
            axis: BandAxis::Hue,
        };
        let result = own.with_control_from(&control, &source);
        let sky = result.settings.bands.for_band(&band_id("sky"));
        assert_eq!(sky.hue, knob(-0.4));
        assert_eq!(sky.saturation, knob(0.4));
        assert_eq!(sky.luminance, knob(0.5));
    }

    #[test]
    fn band_axis_copy_that_makes_band_neutral_drops_the_entry() {
        let only_hue = BandAdjustment {
            hue: knob(0.5),
            ..BandAdjustment::neutral(band_id("sky"))
        };
        let own = recipe(
            ToneBaseModel::Agx,
            LookSettings {
                bands: BandAdjustments::new(vec![only_hue]).unwrap(),
                ..LookSettings::NEUTRAL
            },
        );
        let clean = recipe(ToneBaseModel::Agx, LookSettings::NEUTRAL);
        let control = LookControl::Band {
            band: band_id("sky"),
            axis: BandAxis::Hue,
        };
        let result = own.with_control_from(&control, &clean);
        assert_eq!(result, clean);
        assert!(result.settings.bands.is_neutral());
        // И наоборот: у источника есть правка, у нас её нет — полоса появляется.
        let restored = clean.with_control_from(&control, &own);
        assert_eq!(restored, own);
    }

    #[test]
    fn wheel_parts_copy_push_ring_or_both() {
        let own = rich_recipe(0.3, ToneBaseModel::Agx);
        let source = rich_recipe(-0.4, ToneBaseModel::Agx);
        let source_wheel = source.settings.wheels.wheel(WheelZone::Gain);
        let own_wheel = own.settings.wheels.wheel(WheelZone::Gain);
        let pick = |part| {
            own.with_control_from(
                &LookControl::Wheel {
                    zone: WheelZone::Gain,
                    part,
                },
                &source,
            )
            .settings
            .wheels
            .wheel(WheelZone::Gain)
        };
        assert_eq!(
            pick(WheelPart::Push),
            ColorWheel {
                push: source_wheel.push,
                ring: own_wheel.ring
            }
        );
        assert_eq!(
            pick(WheelPart::Ring),
            ColorWheel {
                push: own_wheel.push,
                ring: source_wheel.ring
            }
        );
        assert_eq!(pick(WheelPart::Whole), source_wheel);
    }

    #[test]
    fn tone_base_copy_keeps_all_settings() {
        let own = rich_recipe(0.3, ToneBaseModel::Agx);
        let source = rich_recipe(-0.4, ToneBaseModel::Legacy);
        let result = own.with_control_from(&LookControl::ToneBase, &source);
        assert_eq!(result.base, ToneBaseModel::Legacy);
        assert_eq!(result.settings, own.settings);
    }

    #[test]
    fn differs_from_is_false_for_equal_recipes_and_true_only_for_the_changed_control() {
        let base = rich_recipe(0.3, ToneBaseModel::Agx);
        for control in all_controls() {
            assert!(!base.differs_from(&base.clone(), &control), "{control:?}");
        }
        let changed = LookRecipe {
            settings: base.settings.clone().with_knob(GlobalKnob::Warmth, knob(-0.9)),
            ..base.clone()
        };
        assert!(changed.differs_from(&base, &LookControl::Knob(GlobalKnob::Warmth)));
        assert!(!changed.differs_from(&base, &LookControl::Knob(GlobalKnob::Tint)));
        assert!(!changed.differs_from(&base, &LookControl::ToneBase));
        let other_base = LookRecipe {
            base: ToneBaseModel::Filmic,
            ..base.clone()
        };
        assert!(other_base.differs_from(&base, &LookControl::ToneBase));
        assert!(!other_base.differs_from(&base, &LookControl::Knob(GlobalKnob::Warmth)));
    }

    #[test]
    fn unknown_band_reads_as_neutral_so_differs_from_is_false() {
        let own = recipe(ToneBaseModel::Agx, LookSettings::NEUTRAL);
        let control = LookControl::Band {
            band: band_id("ghost"),
            axis: BandAxis::Hue,
        };
        assert!(!own.differs_from(&own.clone(), &control));
    }

    #[test]
    fn serde_roundtrip_keeps_base_and_settings() {
        let original = rich_recipe(0.3, ToneBaseModel::Filmic);
        let text = serde_json::to_string(&original).unwrap();
        assert!(text.contains("\"base\":\"filmic\""), "{text}");
        assert_eq!(serde_json::from_str::<LookRecipe>(&text).unwrap(), original);
    }

    #[test]
    fn serde_rejects_unknown_field_and_missing_base_or_settings() {
        let settings = serde_json::to_string(&LookSettings::NEUTRAL).unwrap();
        let unknown = format!(r#"{{"base":"agx","settings":{settings},"extra":1}}"#);
        assert!(serde_json::from_str::<LookRecipe>(&unknown).is_err());
        let no_base = format!(r#"{{"settings":{settings}}}"#);
        assert!(serde_json::from_str::<LookRecipe>(&no_base).is_err());
        assert!(serde_json::from_str::<LookRecipe>(r#"{"base":"agx"}"#).is_err());
        let ok = format!(r#"{{"base":"agx","settings":{settings}}}"#);
        assert!(serde_json::from_str::<LookRecipe>(&ok).is_ok());
    }
}
