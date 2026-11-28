//! Упаковка полосок R, G, B (решения 339–343) в строки 39–50 блока `LookParameters`.

use isp_model::cfa::BayerPattern;
use isp_model::look::LookControls;
use isp_model::look::model::LookModelSettings;
use isp_model::look::primaries::PrimariesModelSettings;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use look_contract::{ChannelPrimaries, LookSettings, PrimaryChannel, WheelZone};

use super::tests::{knob, row_at};
use super::*;
use crate::quality::test_params::quality_params_with_look;

/// Первая строка полосок в блоке (зеркало `LOOK_ROW_PRIMARIES` в GLSL).
const FIRST_PRIMARIES_ROW: usize = 39;

fn params_with(settings: LookSettings) -> IspParams {
    let controls = LookControls::new(settings, Arc::new(LookModelSettings::default()));
    quality_params_with_look(BayerPattern::RGGB, ToneBase::Agx(AgxSettings::default()), controls)
}

/// Вид, в котором тронуты ВСЕ 12 полосок разными значениями (перестановка зон или каналов даёт другие числа).
fn all_bars_touched() -> ChannelPrimaries {
    let mut bars = ChannelPrimaries::NEUTRAL;
    let mut step = 0.0;
    for zone in WheelZone::ALL {
        for channel in PrimaryChannel::ALL {
            step += 1.0;
            bars = bars.with_value(
                zone,
                channel,
                knob(0.07 * step * if (step as u32).is_multiple_of(2) { -1.0 } else { 1.0 }),
            );
        }
    }
    bars
}

#[test]
fn primaries_rows_are_the_tail_of_the_block_after_the_old_rows() {
    // Раскладка: 39 прежних строк (цвет 0–35 + тон 36–38) и 12 строк полосок.
    assert_eq!(FIRST_PRIMARIES_ROW + 12, LOOK_ROWS);
    assert_eq!(LOOK_UNIFORM_BYTES, LOOK_ROWS * 16);
}

#[test]
fn bars_leave_rows_zero_to_thirty_eight_byte_for_byte_as_without_bars() {
    // Вид с тоном, цветом и колёсами: строки 0–38 заняты. Полоски поверх не имеют права их тронуть.
    let base_settings = LookSettings {
        contrast: knob(0.4),
        saturation: knob(0.3),
        ..LookSettings::NEUTRAL
    };
    let without = pack_look_uniforms(&params_with(base_settings.clone()));
    let with = pack_look_uniforms(&params_with(LookSettings {
        primaries: all_bars_touched(),
        ..base_settings
    }));
    assert_eq!(without[..FIRST_PRIMARIES_ROW * 16], with[..FIRST_PRIMARIES_ROW * 16]);
    assert!(
        without[FIRST_PRIMARIES_ROW * 16..].iter().all(|byte| *byte == 0),
        "без полосок хвост — нули"
    );
    assert!(
        with[FIRST_PRIMARIES_ROW * 16..].iter().any(|byte| *byte != 0),
        "с полосками хвост занят"
    );
}

#[test]
fn only_bars_touched_pack_flag_constants_matrices_and_zone_rows_and_leave_old_rows_zero() {
    let model = PrimariesModelSettings::default();
    let params = params_with(LookSettings {
        primaries: all_bars_touched(),
        ..LookSettings::NEUTRAL
    });
    let resolved = params.look.primaries.expect("полоски тронуты");
    let bytes = pack_look_uniforms(&params);
    assert!(
        bytes[..FIRST_PRIMARIES_ROW * 16].iter().all(|byte| *byte == 0),
        "цвет и тон нейтральны — нули"
    );
    let row = |index: usize| row_at(&bytes, FIRST_PRIMARIES_ROW + index);
    let encoding = resolved.encoding;
    assert_eq!(row(0), [1.0, model.encoding_toe, encoding.toe_root, encoding.scale]);
    assert_eq!(row(1), [encoding.inverse_scale, 0.0, 0.0, 0.0]);
    for index in 0..3 {
        let [a, b, c] = resolved.working_to_basis[index];
        assert_eq!(row(2 + index), [a, b, c, 0.0], "W → основа, строка {index}");
        let [a, b, c] = resolved.basis_to_working[index];
        assert_eq!(row(5 + index), [a, b, c, 0.0], "основа → W, строка {index}");
    }
    // Зоны по строкам Lift, Gamma, Gain, Offset; в строке — каналы R, G, B.
    for (zone_row, zone) in [WheelZone::Lift, WheelZone::Gamma, WheelZone::Gain, WheelZone::Offset]
        .into_iter()
        .enumerate()
    {
        let packed = row(8 + zone_row);
        for (channel_index, channel) in PrimaryChannel::ALL.into_iter().enumerate() {
            let knob_value = all_bars_touched().value(zone, channel).get();
            let expected = match zone {
                WheelZone::Lift => model.lift_range * knob_value,
                WheelZone::Gamma => (model.gamma_slope_stops * knob_value).exp2(),
                WheelZone::Gain => (model.gain_stops_in_sqrt * knob_value).exp2(),
                WheelZone::Offset => model.offset_range * knob_value,
            };
            assert_eq!(packed[channel_index], expected, "{zone:?}/{channel:?}");
        }
        assert_eq!(packed[3], 0.0);
    }
}

#[test]
fn neutral_bars_after_touching_pack_the_same_block_as_never_touched_and_do_not_stick() {
    let never = pack_look_uniforms(&params_with(LookSettings::NEUTRAL));
    let touched = pack_look_uniforms(&params_with(LookSettings {
        primaries: all_bars_touched(),
        ..LookSettings::NEUTRAL
    }));
    assert_ne!(never, touched);
    let returned = pack_look_uniforms(&params_with(LookSettings {
        primaries: ChannelPrimaries::NEUTRAL,
        ..LookSettings::NEUTRAL
    }));
    assert_eq!(never, returned);
    assert!(never.iter().all(|byte| *byte == 0));
}

#[test]
fn shader_row_table_and_files_agree_with_the_rust_layout() {
    let stage = include_str!("../../shaders/look_stage.glsl");
    assert!(stage.contains(&format!("const uint LOOK_ROW_PRIMARIES = {FIRST_PRIMARIES_ROW}u;")));
    assert!(stage.contains("#include \"look_primaries.glsl\""));
    assert!(
        stage.contains("look_apply_lut(look_apply_color(look_apply_primaries(look_apply_tone(working_rgb))))"),
        "порядок тон → полоски → цвет → LUT"
    );
    let viewfinder = include_str!("../../shaders/viewfinder_isp.glsl");
    assert!(viewfinder.contains("look_apply_color(look_apply_primaries(look_apply_tone(display_working)))"));
}
