//! Исполнитель замера: ответ приходит с меткой кадра и областью, очередь ограничена.

#![allow(clippy::unwrap_used, reason = "в тестах падение — сигнал провала")]

use camera_contract::auto_exposure::MeasurementQuality;
use camera_contract::three_a::PointRequestId;

use super::*;
use crate::exposure::measurement::MeasurementPhase;
use crate::exposure::metering_weights::{MeteringArea, TilePlacement};
use crate::exposure::test_support::{GRID, plain_luminance, product, target, uniform_gray};

fn tag(sequence: u64) -> FrameTag {
    FrameTag {
        stream_generation: 1,
        sequence,
        sensor_timestamp_nanos: 1_000 * sequence as i64,
    }
}

#[test]
fn submitted_frame_comes_back_measured_with_its_tag_and_area() {
    let mut meter = CpuExposureMeter::new(MeteringConfig::default()).unwrap();
    let weights = MeteringWeights::build(
        GRID,
        TilePlacement::full_frame(4.0 / 3.0).unwrap(),
        MeteringArea::Matrix,
        &MeteringConfig::default(),
    )
    .unwrap();
    let stats = uniform_gray(0.15);
    let area = MeteredArea::Point(PointRequestId(7));
    meter
        .submit(ExposureMeteringRequest {
            frame: tag(5),
            area,
            statistics: &stats,
            weights: &weights,
            luminance: plain_luminance(),
            frame_product: product(0.01),
            target: target(),
            phase: MeasurementPhase::Established,
        })
        .unwrap();
    let done = meter.poll().unwrap();
    assert_eq!(done.frame, tag(5));
    assert_eq!(done.area, area);
    let measurement = done.outcome.unwrap();
    assert_eq!(measurement.quality, MeasurementQuality::Measured);
    // Вдвое ярче цели: нужно вдвое меньше света.
    assert!((measurement.required_product.seconds_iso() - 0.005).abs() < 1e-4);
    assert!(meter.poll().is_none(), "ответ отдаётся один раз");
}

#[test]
fn unclaimed_measurements_are_capped_and_the_oldest_is_dropped_with_a_count() {
    let mut meter = CpuExposureMeter::new(MeteringConfig::default()).unwrap();
    let weights = MeteringWeights::build(
        GRID,
        TilePlacement::full_frame(4.0 / 3.0).unwrap(),
        MeteringArea::Matrix,
        &MeteringConfig::default(),
    )
    .unwrap();
    let stats = uniform_gray(0.075);
    for sequence in 0..6 {
        meter
            .submit(ExposureMeteringRequest {
                frame: tag(sequence),
                area: MeteredArea::WholeFrame,
                statistics: &stats,
                weights: &weights,
                luminance: plain_luminance(),
                frame_product: product(0.01),
                target: target(),
                phase: MeasurementPhase::Established,
            })
            .unwrap();
    }
    assert_eq!(meter.overwritten(), 2);
    assert_eq!(meter.poll().unwrap().frame, tag(2), "самые старые вытеснены");
}

#[test]
fn failed_measurement_is_reported_not_dropped() {
    let mut meter = CpuExposureMeter::new(MeteringConfig::default()).unwrap();
    let weights = MeteringWeights::build(
        GRID,
        TilePlacement::full_frame(4.0 / 3.0).unwrap(),
        MeteringArea::Matrix,
        &MeteringConfig::default(),
    )
    .unwrap();
    let empty = crate::exposure::test_support::stats_from_tiles(|_, _| isp_model::statistics::AwbTile::default());
    meter
        .submit(ExposureMeteringRequest {
            frame: tag(1),
            area: MeteredArea::WholeFrame,
            statistics: &empty,
            weights: &weights,
            luminance: plain_luminance(),
            frame_product: product(0.01),
            target: target(),
            phase: MeasurementPhase::Established,
        })
        .unwrap();
    assert!(meter.poll().unwrap().outcome.is_err());
    assert!(
        CpuExposureMeter::new(MeteringConfig {
            dark_floor: -1.0,
            ..Default::default()
        })
        .is_err()
    );
}
