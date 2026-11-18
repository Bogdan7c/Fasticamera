//! Граница исполнителя: порядок, идентичность кадра, переполнение, использование через трейт-объект.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::capabilities::RawSupport;
use camera_contract::fixtures::main_wide_camera;
use isp_model::color::dng::DngColorProfile;
use isp_model::statistics::{AwbTile, TileGrid};

use super::*;
use crate::config::LocusTableConfig;

fn table() -> CameraLocusTable {
    let RawSupport::Supported(info) = main_wide_camera().capabilities.raw else {
        panic!("в образце основной камеры есть RAW");
    };
    let profile = DngColorProfile::from_calibration_sets(&info.color_calibration).expect("профиль цвета");
    CameraLocusTable::build(&profile, &LocusTableConfig::default(), 0.02).expect("таблица")
}

fn empty_statistics() -> AwbStatistics {
    AwbStatistics::from_tiles(TileGrid { columns: 2, rows: 1 }, vec![AwbTile::default(); 2]).expect("плитки")
}

fn tag(sequence: u64) -> FrameTag {
    FrameTag {
        stream_generation: 7,
        sequence,
        sensor_timestamp_nanos: 1_000 * i64::try_from(sequence).expect("номер кадра"),
    }
}

#[test]
fn poll_before_any_submit_is_empty() {
    let mut executor = CpuWhiteBalanceEstimator::new(EstimatorConfig::default()).expect("исполнитель");
    assert_eq!(executor.poll(), None);
}

#[test]
fn answers_come_back_in_order_with_the_identity_of_their_frame() {
    let (table, statistics) = (table(), empty_statistics());
    let mut executor: Box<dyn WhiteBalanceEstimator> =
        Box::new(CpuWhiteBalanceEstimator::new(EstimatorConfig::default()).expect("исполнитель"));
    for sequence in [3, 4] {
        executor
            .submit(EstimationRequest {
                frame: tag(sequence),
                statistics: &statistics,
                table: &table,
            })
            .expect("запрос принят");
    }
    let first = executor.poll().expect("первый ответ");
    let second = executor.poll().expect("второй ответ");
    assert_eq!((first.frame, second.frame), (tag(3), tag(4)));
    assert_eq!(first.outcome, Ok(EstimateOutcome::NoCandidates { eligible_tiles: 0 }));
    assert_eq!(executor.poll(), None, "ответы выданы по одному разу");
}

#[test]
fn unclaimed_answers_are_bounded_and_the_oldest_are_counted_as_overwritten() {
    let (table, statistics) = (table(), empty_statistics());
    let mut executor = CpuWhiteBalanceEstimator::new(EstimatorConfig::default()).expect("исполнитель");
    for sequence in 0..6 {
        executor
            .submit(EstimationRequest {
                frame: tag(sequence),
                statistics: &statistics,
                table: &table,
            })
            .expect("запрос принят");
    }
    assert_eq!(executor.overwritten(), 2, "6 запросов при ёмкости 4");
    assert_eq!(executor.poll().expect("ответ").frame, tag(2), "самые старые вытеснены");
}

#[test]
fn invalid_estimator_config_is_rejected_at_construction() {
    let config = EstimatorConfig {
        min_level: 0.99,
        ..EstimatorConfig::default()
    };
    assert!(CpuWhiteBalanceEstimator::new(config).is_err());
}
