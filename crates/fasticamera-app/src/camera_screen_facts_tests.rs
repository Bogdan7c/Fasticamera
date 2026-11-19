//! Тесты перевода статуса движка в факты экрана: отсутствие и недоступность камеры, точка, блокировка, фокус,
//! диапазон EV активной камеры, геометрия потока и «изменение только при реальной разнице».

use std::sync::Arc;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::capabilities::Rational;
use camera_contract::catalog::CameraCatalog;
use camera_contract::fixtures::{camera_id, front_camera, main_wide_camera, ultra_wide_camera};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{
    ConfiguredStream, FrameRateMechanism, ReadoutAspect, ResolvedStream, StreamMode, TargetFrameRate,
};
use camera_contract::three_a::{AeAfLock, MeteringRegion, PointOfInterest, PointRequestId, SensorPoint};
use camera_core::state::{ClosedReason, RecoveryPlan};
use camera_core::status::{AppPresence, CameraCounters, StreamStats};
use camera_core::three_a_status::{FocusOutcome, ThreeAStatus};

use super::*;

fn catalog() -> Arc<CameraCatalog> {
    Arc::new(
        CameraCatalog::new(
            vec![main_wide_camera(), ultra_wide_camera(), front_camera()],
            Vec::new(),
        )
        .expect("каталог образцов"),
    )
}

fn status_in(state: CameraState) -> CameraStatus {
    CameraStatus {
        state,
        presence: AppPresence::Foreground,
        catalog: catalog(),
        configured: None,
        stream: StreamStats::empty(),
        last_unavailable: None,
        counters: CameraCounters::default(),
        three_a: ThreeAStatus::initial(),
    }
}

fn streaming(camera: &str, readout: ReadoutAspect) -> CameraState {
    let mode = StreamMode {
        raw_format: RawFormat::Raw10,
        readout,
        frame_rate: TargetFrameRate::new(30).expect("30 к/с"),
    };
    let buffer_size = match camera {
        "3" => PixelSize::new(4000, 3000),
        _ => PixelSize::new(4096, 3072),
    };
    CameraState::Streaming {
        stream: ConfiguredStream {
            stream: ResolvedStream {
                camera: camera_id(camera),
                raw_format: mode.raw_format,
                buffer_size,
                readout: mode.readout,
                frame_rate: mode.frame_rate,
            },
            frame_rate_mechanism: FrameRateMechanism::HalDefault,
        },
    }
}

#[test]
fn closed_or_opening_camera_is_starting_with_no_stream() {
    for state in [
        CameraState::Closed {
            reason: ClosedReason::NotRequested,
        },
        CameraState::Closed {
            reason: ClosedReason::AppInBackground,
        },
        CameraState::Opening { camera: camera_id("2") },
    ] {
        let facts = camera_facts_from_status(&status_in(state.clone()));
        assert_eq!(facts.presence, CameraPresence::Starting, "{state:?}");
        assert_eq!(facts.stream, None, "{state:?}");
    }
}

#[test]
fn absent_engine_is_a_failure_not_a_silent_starting() {
    let facts = facts_for_absent_engine();
    assert_eq!(
        facts.presence,
        CameraPresence::Unavailable {
            reason: UnavailableReason::Failure
        }
    );
    assert_eq!(facts.stream, None);
}

#[test]
fn unavailability_reasons_map_to_screen_reasons() {
    let cases = [
        (
            CameraUnavailableReason::PermissionDenied,
            UnavailableReason::PermissionDenied,
        ),
        (
            CameraUnavailableReason::DisabledByPolicy,
            UnavailableReason::DisabledByPolicy,
        ),
        (CameraUnavailableReason::Evicted, UnavailableReason::InUse),
        (CameraUnavailableReason::InUseByAnotherClient, UnavailableReason::InUse),
        (CameraUnavailableReason::TooManyCamerasOpen, UnavailableReason::InUse),
        (CameraUnavailableReason::DeviceFailure, UnavailableReason::Failure),
        (CameraUnavailableReason::ServiceFailure, UnavailableReason::Failure),
        (CameraUnavailableReason::CameraNotPresent, UnavailableReason::Failure),
    ];
    for (engine_reason, expected) in cases {
        let state = CameraState::Error {
            error: CameraError::Recoverable {
                reason: engine_reason,
                recovery: RecoveryPlan::WaitForExternalAction,
            },
        };
        assert_eq!(
            camera_facts_from_status(&status_in(state)).presence,
            CameraPresence::Unavailable { reason: expected },
            "{engine_reason:?}"
        );
    }
    let exhausted = CameraState::Error {
        error: CameraError::Fatal {
            cause: FatalCause::RecoveryExhausted {
                reason: CameraUnavailableReason::PermissionDenied,
                attempts: 3,
            },
        },
    };
    assert_eq!(
        camera_facts_from_status(&status_in(exhausted)).presence,
        CameraPresence::Unavailable {
            reason: UnavailableReason::PermissionDenied
        },
        "причина исчерпанных попыток не теряется"
    );
    let panicked = CameraState::Error {
        error: CameraError::Fatal {
            cause: FatalCause::EnginePanicked {
                message: "x".to_owned(),
            },
        },
    };
    assert_eq!(
        camera_facts_from_status(&status_in(panicked)).presence,
        CameraPresence::Unavailable {
            reason: UnavailableReason::Failure
        }
    );
}

#[test]
fn point_lock_and_focus_come_from_the_three_a_status() {
    let mut status = status_in(streaming("2", ReadoutAspect::FourByThree));
    let region = MeteringRegion::new(SensorPoint::new(0.25, 0.75).expect("точка"), 0.1).expect("регион");
    status.three_a.point = PointOfInterest::Point {
        region,
        request: PointRequestId(7),
    };
    status.three_a.lock = AeAfLock::Locked;
    status.three_a.focus = FocusProgress::Settled {
        request: PointRequestId(7),
        outcome: FocusOutcome::Focused,
    };

    let facts = camera_facts_from_status(&status);

    assert_eq!(facts.presence, CameraPresence::Live);
    assert_eq!(
        facts.point_of_interest,
        PointOfInterest::Point {
            region,
            request: PointRequestId(7)
        }
    );
    assert_eq!(facts.lock, AeAfLock::Locked);
    assert_eq!(facts.focus, FocusPhase::Focused);
}

#[test]
fn every_focus_progress_has_its_phase() {
    let request = PointRequestId(1);
    let cases = [
        (FocusProgress::FixedLens, FocusPhase::Continuous),
        (FocusProgress::Manual, FocusPhase::Continuous),
        (FocusProgress::Continuous { af_state: None }, FocusPhase::Continuous),
        (FocusProgress::Scanning { request }, FocusPhase::Scanning),
        (
            FocusProgress::Settled {
                request,
                outcome: FocusOutcome::NotFocused,
            },
            FocusPhase::NotFocused,
        ),
        (
            FocusProgress::Settled {
                request,
                outcome: FocusOutcome::TimedOut,
            },
            FocusPhase::NoResult,
        ),
        (
            FocusProgress::Settled {
                request,
                outcome: FocusOutcome::TriggerNotDelivered,
            },
            FocusPhase::NoResult,
        ),
    ];
    for (progress, expected) in cases {
        assert_eq!(focus_phase_of(progress), expected, "{progress:?}");
    }
}

#[test]
fn exposure_range_is_taken_from_the_active_camera() {
    // Основная камера образцов: −18…18 шагов по 1/6 EV = ±3 EV.
    let facts = camera_facts_from_status(&status_in(streaming("2", ReadoutAspect::FourByThree)));
    assert_eq!(
        facts.exposure_range,
        ExposureRangeFact::Supported {
            min_ev: -3.0,
            max_ev: 3.0
        }
    );
    // Камера без записи в каталоге (или без камеры) — «не сообщила», а не чужой диапазон.
    let unknown = camera_facts_from_status(&status_in(streaming("99", ReadoutAspect::FourByThree)));
    assert_eq!(unknown.exposure_range, ExposureRangeFact::NotReported);
    let closed = camera_facts_from_status(&status_in(CameraState::Closed {
        reason: ClosedReason::NotRequested,
    }));
    assert_eq!(closed.exposure_range, ExposureRangeFact::NotReported);
}

#[test]
fn exposure_range_translation_covers_unsupported_and_broken_steps() {
    assert_eq!(
        exposure_range_of(&ExposureCompensationSupport::NotSupported),
        ExposureRangeFact::Unsupported
    );
    let zero_range = ExposureCompensationSupport::Supported {
        min_steps: 0,
        max_steps: 0,
        step_ev: Rational {
            numerator: 1,
            denominator: 3,
        },
    };
    assert_eq!(
        exposure_range_of(&zero_range),
        ExposureRangeFact::Unsupported,
        "диапазон [0, 0]"
    );
    let broken = ExposureCompensationSupport::Supported {
        min_steps: -6,
        max_steps: 6,
        step_ev: Rational {
            numerator: 1,
            denominator: 0,
        },
    };
    assert_eq!(
        exposure_range_of(&broken),
        ExposureRangeFact::NotReported,
        "нулевой знаменатель"
    );
}

#[test]
fn stream_geometry_comes_from_the_passport_of_the_streaming_camera() {
    let facts = camera_facts_from_status(&status_in(streaming("2", ReadoutAspect::SixteenByNine)));
    let geometry = facts.stream.expect("поток идёт");
    // 16:9 занимает верх буфера 4096×3072: строки 0–2303 (4096 × 9 / 16).
    assert_eq!(
        geometry.valid_area,
        PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(4096, 2304)
        }
    );
    assert_eq!(geometry.active_array_size, PixelSize::new(4096, 3072));
    assert_eq!(geometry.sensor_orientation, SensorOrientation::Degrees90);
    assert_eq!(geometry.mirror, MirrorMode::None, "задняя камера без зеркала");

    let full = camera_facts_from_status(&status_in(streaming("2", ReadoutAspect::FourByThree)));
    assert_eq!(
        full.stream.expect("поток").valid_area.size,
        PixelSize::new(4096, 3072),
        "4:3 занимает весь буфер"
    );
}

#[test]
fn front_camera_is_mirrored_and_other_camera_uses_its_own_active_array() {
    let front = camera_facts_from_status(&status_in(streaming("1", ReadoutAspect::FourByThree)));
    let geometry = front.stream.expect("поток фронталки");
    assert_eq!(geometry.mirror, MirrorMode::Horizontal);
    assert_eq!(geometry.sensor_orientation, SensorOrientation::Degrees270);

    let wide = camera_facts_from_status(&status_in(streaming("3", ReadoutAspect::FourByThree)));
    assert_eq!(
        wide.stream.expect("поток ширика").active_array_size,
        PixelSize::new(4000, 3000)
    );
}

#[test]
fn no_stream_geometry_before_streaming_or_when_the_camera_is_unknown() {
    let opening = status_in(CameraState::Opened { camera: camera_id("2") });
    assert_eq!(camera_facts_from_status(&opening).stream, None);
    let unknown = status_in(streaming("99", ReadoutAspect::FourByThree));
    assert_eq!(camera_facts_from_status(&unknown).stream, None);
}

#[test]
fn watcher_reports_only_real_changes() {
    let mut watcher = CameraFactsWatcher::new();
    let first = camera_facts_from_status(&status_in(streaming("2", ReadoutAspect::FourByThree)));

    assert_eq!(
        watcher.observe(first),
        FactsChange::Changed(first),
        "первое наблюдение — изменение"
    );
    assert_eq!(watcher.observe(first), FactsChange::Unchanged, "те же факты");

    let mut locked_status = status_in(streaming("2", ReadoutAspect::FourByThree));
    locked_status.three_a.lock = AeAfLock::Locked;
    let locked = camera_facts_from_status(&locked_status);
    assert_eq!(watcher.observe(locked), FactsChange::Changed(locked));
    assert_eq!(watcher.observe(locked), FactsChange::Unchanged);
    // Возврат к прежним фактам — снова изменение (сравнивается с последними отправленными).
    assert_eq!(watcher.observe(first), FactsChange::Changed(first));
}

/// Поток 16:9 в буфере, где полоса 16:9 не помещается (4096×2000 < 4096×9/16 = 2304 строки): паспорт не соберётся.
fn streaming_without_a_passport() -> CameraState {
    let mut state = streaming("2", ReadoutAspect::SixteenByNine);
    if let CameraState::Streaming { stream } = &mut state {
        stream.stream.buffer_size = PixelSize::new(4096, 2000);
    }
    state
}

#[test]
fn the_passport_is_built_once_per_stream_not_once_per_frame() {
    let mut cache = StreamGeometryCache::default();
    let status = status_in(streaming("2", ReadoutAspect::FourByThree));
    let first = camera_facts_with_cache(&status, &mut cache);
    for _ in 0..50 {
        assert_eq!(
            camera_facts_with_cache(&status, &mut cache),
            first,
            "кэш отдаёт те же факты"
        );
    }
    assert_eq!(cache.diagnostics().passport_builds, 1, "50 кадров — одна сборка");
    assert_eq!(first.stream, camera_facts_from_status(&status).stream);

    // Смена потока (другая раскладка) — пересчёт, и геометрия действительно новая.
    let widescreen = camera_facts_with_cache(&status_in(streaming("2", ReadoutAspect::SixteenByNine)), &mut cache);
    assert_eq!(cache.diagnostics().passport_builds, 2);
    assert_eq!(
        widescreen.stream.expect("геометрия").valid_area.size,
        PixelSize::new(4096, 2304)
    );
    // Возврат к прежнему потоку — тоже пересчёт (кэш помнит один, последний, поток).
    let back = camera_facts_with_cache(&status, &mut cache);
    assert_eq!(cache.diagnostics().passport_builds, 3);
    assert_eq!(back.stream, first.stream);
}

#[test]
fn a_passport_error_is_counted_once_per_stream_and_yields_no_geometry() {
    let mut cache = StreamGeometryCache::default();
    let status = status_in(streaming_without_a_passport());
    for _ in 0..10 {
        assert_eq!(camera_facts_with_cache(&status, &mut cache).stream, None);
    }
    let diagnostics = cache.diagnostics();
    assert_eq!(
        (diagnostics.passport_builds, diagnostics.passport_failures),
        (1, 1),
        "ошибка посчитана один раз на поток, а не на кадр"
    );
    // Следующий, исправный поток снова даёт геометрию, счётчик отказов не растёт.
    let good = camera_facts_with_cache(&status_in(streaming("2", ReadoutAspect::FourByThree)), &mut cache);
    assert!(good.stream.is_some());
    assert_eq!(cache.diagnostics().passport_failures, 1);
}

#[test]
fn an_unknown_camera_is_not_cached_and_is_not_a_passport_failure() {
    let mut cache = StreamGeometryCache::default();
    let unknown = status_in(streaming("99", ReadoutAspect::FourByThree));
    assert_eq!(camera_facts_with_cache(&unknown, &mut cache).stream, None);
    assert_eq!(cache.diagnostics(), FactsDiagnostics::default());
}

#[test]
fn sensor_orientation_is_the_hal_fact_of_the_active_camera() {
    let rear = status_in(streaming("2", ReadoutAspect::FourByThree));
    assert_eq!(
        sensor_orientation_of_active_camera(&rear),
        Some(SensorOrientation::Degrees90)
    );
    let front = status_in(streaming("1", ReadoutAspect::FourByThree));
    assert_eq!(
        sensor_orientation_of_active_camera(&front),
        Some(SensorOrientation::Degrees270)
    );
    let closed = status_in(CameraState::Closed {
        reason: ClosedReason::NotRequested,
    });
    assert_eq!(
        sensor_orientation_of_active_camera(&closed),
        None,
        "камеры нет — ориентации нет"
    );
}
