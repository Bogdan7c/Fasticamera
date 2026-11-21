//! Отсев предложений чужой камеры и перенос ожидаемой цели (13b): две камеры стримят одновременно, и их
//! метки времени перемешиваются на одной шкале, поэтому одного времени мало.

use std::sync::atomic::Ordering;

use camera_contract::auto_exposure::{
    CameraTag, ExposureProduct, ExposureProposal, ExposureProposalChannel, MeasurementQuality, MeteredArea,
    ProposalConvergence,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::fixtures::camera_id;
use camera_contract::three_a::{MainsFrequency, RequestOrigin, ThreeAResultState};

use super::config::OwnExposureConfig;
use super::exposure::RawStatsExposure;
use crate::exposure_transfer::ExposureTransfer;
use crate::three_a_control::{ExposureController, PlanRefresh};
use crate::three_a_status::AutoExposureProgress;

const OLD_CAMERA: &str = "0";
const NEW_CAMERA: &str = "3";

fn tag(camera: &str) -> CameraTag {
    CameraTag::of(&camera_id(camera))
}

fn proposal(measured_ns: i64, target: f64, camera: &str) -> ExposureProposal {
    ExposureProposal {
        measured_frame: SensorTimestamp::from_nanos(measured_ns),
        measured_on: tag(camera),
        target: ExposureProduct::from_seconds_iso(target).expect("цель"),
        metered_area: MeteredArea::WholeFrame,
        convergence: ProposalConvergence::Settled,
        quality: MeasurementQuality::Measured,
    }
}

fn repeating_result(timestamp_ns: i64) -> CaptureResultMeta {
    CaptureResultMeta {
        frame_number: FrameNumber(timestamp_ns / 1_000),
        sensor_timestamp: Some(SensorTimestamp::from_nanos(timestamp_ns)),
        exposure_time: None,
        sensitivity_iso: None,
        reported_frame_duration: None,
        rolling_shutter_skew: None,
        three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
    }
}

/// Исполнитель, чей поток начался на `camera`, с ящиком предложений.
fn exposure_on(camera: &str) -> (RawStatsExposure, ExposureProposalChannel) {
    let channel = ExposureProposalChannel::new();
    let mut exposure = RawStatsExposure::new(OwnExposureConfig::default(), channel.clone(), MainsFrequency::Hz50);
    exposure.begin_stream_on_camera(&camera_id(camera));
    (exposure, channel)
}

#[test]
fn a_proposal_measured_on_another_camera_is_rejected_even_when_its_timestamp_looks_fresh() {
    let (mut exposure, channel) = exposure_on(OLD_CAMERA);
    exposure.observe_result(&repeating_result(5_000));
    // Метка 9_000 позже начала потока: по времени предложение «свежее», но измерено на чужой камере.
    channel.publish(proposal(9_000, 0.05, NEW_CAMERA));
    assert_eq!(exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(exposure.foreign_camera_proposals_counter().load(Ordering::SeqCst), 1);
    assert_eq!(
        exposure.stale_proposals_counter().load(Ordering::SeqCst),
        0,
        "это не «прошлый поток»"
    );
    assert_eq!(exposure.progress(), Some(AutoExposureProgress::AwaitingMeasurement));
    // Предложение своей камеры принимается.
    channel.publish(proposal(9_500, 0.4, OLD_CAMERA));
    assert_eq!(exposure.refresh(), PlanRefresh::Changed);
    assert_eq!(exposure.progress(), Some(AutoExposureProgress::Settled));
}

#[test]
fn an_interleaved_late_proposal_from_the_old_camera_is_rejected_after_a_swap() {
    let (mut exposure, channel) = exposure_on(OLD_CAMERA);
    exposure.observe_result(&repeating_result(1_000));
    channel.publish(proposal(2_000, 0.4, OLD_CAMERA));
    assert_eq!(exposure.refresh(), PlanRefresh::Changed);

    // Обмен камер: как делает движок (конец потока, затем старт на новой камере).
    exposure.reset_for_new_stream();
    exposure.begin_stream_on_camera(&camera_id(NEW_CAMERA));
    // Первый результат нового потока на метке 3_000; запоздавшее предложение старой камеры по кадру 3_500
    // ПОЗЖЕ этой метки (метки двух сенсоров перемешаны) — по времени оно прошло бы.
    exposure.observe_result(&repeating_result(3_000));
    channel.publish(proposal(3_500, 0.02, OLD_CAMERA));
    assert_eq!(
        exposure.refresh(),
        PlanRefresh::Unchanged,
        "цель старой камеры не принята"
    );
    assert_eq!(exposure.foreign_camera_proposals_counter().load(Ordering::SeqCst), 1);
    assert_eq!(exposure.progress(), Some(AutoExposureProgress::AwaitingMeasurement));
    // Предложение новой камеры после этого принимается.
    channel.publish(proposal(4_000, 0.2, NEW_CAMERA));
    assert_eq!(exposure.refresh(), PlanRefresh::Changed);
}

#[test]
fn a_foreign_proposal_held_before_the_stream_start_is_known_is_dropped_at_once() {
    let (mut exposure, channel) = exposure_on(NEW_CAMERA);
    // Результата нового потока ещё нет, метки нет, но камеру проверить можно сразу.
    channel.publish(proposal(1_000, 0.4, OLD_CAMERA));
    assert_eq!(exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(exposure.foreign_camera_proposals_counter().load(Ordering::SeqCst), 1);
    // Придержанного предложения нет: метка потока ничего не принимает задним числом.
    exposure.observe_result(&repeating_result(500));
    assert_eq!(exposure.refresh(), PlanRefresh::Unchanged);
    assert_eq!(exposure.progress(), Some(AutoExposureProgress::AwaitingMeasurement));
}

#[test]
fn without_a_named_camera_only_the_timestamp_filter_applies() {
    let channel = ExposureProposalChannel::new();
    let mut exposure = RawStatsExposure::new(OwnExposureConfig::default(), channel.clone(), MainsFrequency::Hz50);
    exposure.observe_result(&repeating_result(1_000));
    channel.publish(proposal(2_000, 0.4, NEW_CAMERA));
    assert_eq!(exposure.refresh(), PlanRefresh::Changed);
    assert_eq!(exposure.foreign_camera_proposals_counter().load(Ordering::SeqCst), 0);
}

#[test]
fn the_carried_scene_target_is_the_remembered_target_recomputed_for_the_new_camera() {
    let (mut exposure, channel) = exposure_on(OLD_CAMERA);
    assert_eq!(exposure.carried_scene_target(), None, "первый поток: ожидать нечего");
    assert_eq!(exposure.scene_target_after_transfer(&ExposureTransfer::IDENTITY), None);

    exposure.observe_result(&repeating_result(1_000));
    channel.publish(proposal(2_000, 0.4, OLD_CAMERA));
    exposure.refresh();

    // До конца потока память ещё не перенесена, но заранее посчитать итог можно: ×2,5 при переносе.
    let transfer = ExposureTransfer {
        factor: 2.5,
        ..ExposureTransfer::IDENTITY
    };
    let predicted = exposure
        .scene_target_after_transfer(&transfer)
        .expect("есть предложение");
    assert!((predicted.seconds_iso() - 1.0).abs() < 1e-9);
    assert_eq!(
        exposure.carried_scene_target(),
        None,
        "заранее посчитанное не меняет состояние"
    );

    // Настоящий перенос даёт то же самое.
    exposure.reset_for_new_stream();
    exposure.carry_memory_to_new_camera(&transfer);
    assert_eq!(exposure.carried_scene_target(), Some(predicted));
}
