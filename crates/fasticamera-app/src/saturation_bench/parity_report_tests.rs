//! Итог линейки паритета в строке: худшее по сценам, допуск, компактная строка результата.

use gpu_vulkan::storage_image::StorageImageFormat;
use isp_vulkan::saturation::parity::{ErrorAllowance, Parity, ParityMeasure};

use super::*;

fn parity(max_abs: f32, math_excess: f32) -> Parity {
    Parity {
        max_abs,
        over_one_lsb: 0.0,
        math_excess,
    }
}

fn measure(half_excess: f32, ten_bit_excess: f32, format: StorageImageFormat) -> ParityMeasure {
    ParityMeasure {
        half: parity(1.0e-4, half_excess),
        ten_bit: parity(4.0e-4, ten_bit_excess),
        ten_bit_format: format,
    }
}

fn record_of(outcome: ParityOutcome) -> ParityRecord {
    match outcome {
        ParityOutcome::Measured(record) => record,
        ParityOutcome::Unavailable { reason } => panic!("ожидались числа, получено: {reason}"),
    }
}

#[test]
fn worst_excess_scene_and_per_format_numbers_are_taken_across_scenes() {
    let mut accumulator = ParityAccumulator::new(ErrorAllowance::Standard);
    accumulator.add_scene(
        "gray_ramp",
        &measure(2.0e-6, 1.0e-6, StorageImageFormat::A2B10G10R10Unorm),
    );
    accumulator.add_scene(
        "hue_sweep",
        &measure(3.0e-6, 9.0e-6, StorageImageFormat::A2B10G10R10Unorm),
    );
    accumulator.add_scene(
        "near_black",
        &measure(0.0, 4.0e-6, StorageImageFormat::A2B10G10R10Unorm),
    );
    let record = record_of(accumulator.finish());
    assert_eq!(record.worst_math_excess, 9.0e-6);
    assert_eq!(record.worst_scene, "hue_sweep");
    assert_eq!(record.fp16_math_excess, 3.0e-6);
    assert_eq!(record.preferred_math_excess, 9.0e-6);
    assert_eq!(record.scenes_measured, 3);
    assert_eq!(record.preferred_format, "rgb10a2");
    assert!(record.passed(), "{record:?}");
}

#[test]
fn excess_over_the_allowance_fails_and_names_the_scene() {
    let mut accumulator = ParityAccumulator::new(ErrorAllowance::Standard);
    accumulator.add_scene(
        "gray_ramp",
        &measure(1.0e-6, 1.0e-6, StorageImageFormat::A2B10G10R10Unorm),
    );
    accumulator.add_scene(
        "hue_sweep",
        &measure(2.5e-5, 1.0e-6, StorageImageFormat::A2B10G10R10Unorm),
    );
    let outcome = accumulator.finish();
    let record = record_of(outcome.clone());
    assert!(!record.passed());
    assert_eq!(record.worst_scene, "hue_sweep");
    let violation = outcome.violation().expect("нарушение допуска");
    assert!(
        violation.contains("hue_sweep") && violation.contains("2.50e-5"),
        "{violation}"
    );
    assert!(
        outcome.compact_row("w").ends_with("|fail"),
        "{}",
        outcome.compact_row("w")
    );
}

#[test]
fn a_device_without_ten_bit_storage_is_reported_but_is_not_a_violation() {
    let mut accumulator = ParityAccumulator::new(ErrorAllowance::Standard);
    accumulator.add_scene("gray_ramp", &measure(1.0e-6, 2.0e-6, StorageImageFormat::Rgba16Float));
    let outcome = accumulator.finish();
    assert_eq!(record_of(outcome.clone()).preferred_format, "rgba16f");
    assert_eq!(outcome.violation(), None);
}

#[test]
fn no_scene_measured_is_unavailable_never_a_pass() {
    let outcome = ParityAccumulator::new(ErrorAllowance::Standard).finish();
    assert!(matches!(outcome, ParityOutcome::Unavailable { .. }));
    assert!(outcome.violation().is_some());
    assert_eq!(outcome.compact_row("w"), "w|-|-|-|unavailable");
}

#[test]
fn compact_row_has_five_cells_with_scientific_numbers_the_script_can_parse() {
    let mut accumulator = ParityAccumulator::new(ErrorAllowance::Standard);
    accumulator.add_scene(
        "hue_sweep",
        &measure(2.0e-6, 9.5e-6, StorageImageFormat::A2B10G10R10Unorm),
    );
    let row = accumulator.finish().compact_row("look_parity_agx_bands");
    let cells: Vec<&str> = row.split('|').collect();
    assert_eq!(cells.len(), 5, "{row}");
    assert_eq!(cells[0], "look_parity_agx_bands");
    assert_eq!(cells[1].parse::<f64>().unwrap(), 9.5e-6);
    assert_eq!(cells[2].parse::<f64>().unwrap(), 4.0e-4);
    assert_eq!(cells[3].parse::<f64>().unwrap(), 1.0e-5);
    assert_eq!(cells[4], "pass");
}
