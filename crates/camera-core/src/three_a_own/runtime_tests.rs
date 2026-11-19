//! Наш AE через настоящий поток камеры: предложение рендера будит поток через
//! `ThreeAWaker` (реализацию `RefreshSignal`), запрос пересобирается ровно один раз.

use std::sync::Arc;
use std::time::{Duration, Instant};

use camera_contract::auto_exposure::{
    ExposureProduct, ExposureProposal, ExposureProposalChannel, MeasurementQuality, MeteredArea, ProposalConvergence,
};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::{CaptureIntent, ManualControl};
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};
use camera_contract::three_a::MainsFrequency;

use crate::config::CameraEngineConfig;
use crate::fake::{BackendCall, FakeBackendControl};
use crate::request_controls::ExposurePlan;
use crate::runtime::{CameraRuntime, ShutdownReport};
use crate::status::AppPresence;
use crate::three_a_own::FlickerPolicy;
use crate::three_a_wiring::ThreeAWiring;

const WAIT: Duration = Duration::from_secs(10);

fn proposal(target: f64) -> ExposureProposal {
    ExposureProposal {
        // Заведомо позже любого кадра нового потока fake.
        measured_frame: SensorTimestamp::from_nanos(i64::MAX / 2),
        target: ExposureProduct::from_seconds_iso(target).expect("цель"),
        metered_area: MeteredArea::WholeFrame,
        convergence: ProposalConvergence::Searching,
        quality: MeasurementQuality::Measured,
    }
}

/// Конфиг с явно включённой защитой 50 Гц: тесты ниже проверяют пробуждение потока, а старт
/// 10 мс × ISO 400 — удобная метка «старт из конфига» (умолчание — защита выключена, решение 77).
fn protected_engine_config() -> CameraEngineConfig {
    let mut config = CameraEngineConfig::default();
    config.three_a.exposure.flicker = FlickerPolicy::AlwaysProtect;
    config
}

fn repeating_requests(fake: &FakeBackendControl) -> usize {
    fake.calls()
        .iter()
        .filter(|call| matches!(call, BackendCall::SetRepeatingRequest { .. }))
        .count()
}

fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !condition() {
        assert!(Instant::now() < deadline, "не дождались: {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn a_published_proposal_wakes_the_camera_thread_and_rebuilds_the_request_once() {
    let fake = FakeBackendControl::new();
    let config = protected_engine_config();
    let channel = ExposureProposalChannel::new();
    let wiring = ThreeAWiring::own(&config.three_a, channel.clone(), MainsFrequency::Hz50);
    let (runtime, handle) = CameraRuntime::spawn_with_wiring(config, AppPresence::Foreground, fake.factory(), wiring)
        .expect("поток запускается");
    // Композиция привязывает получателя толчка после запуска движка.
    channel.refresh_signal().bind(Arc::new(handle.three_a_waker()));

    handle
        .open(CameraTarget::Role(LensRole::Wide))
        .expect("команда принята");
    handle
        .configure(StreamMode {
            raw_format: RawFormat::Raw10,
            readout: ReadoutAspect::SixteenByNine,
            frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        })
        .expect("команда принята");
    handle.start_streaming(CaptureIntent::Preview).expect("команда принята");
    wait_for("поток пошёл", || fake.is_repeating());
    fake.complete_frames(3, Duration::from_millis(16));
    handle
        .wait_until_status(WAIT, |status| status.three_a.last_result.is_some())
        .expect("результаты дошли до движка");

    let sensor = || match fake.repeating_request().map(|(_, controls)| controls.exposure) {
        Some(ExposurePlan::Sensor { exposure_time, iso }) => Some((exposure_time.duration(), iso.value())),
        _ => None,
    };
    assert_eq!(sensor(), Some((Duration::from_millis(10), 400)), "старт из конфига");

    let before = repeating_requests(&fake);
    channel.publish(proposal(0.4));
    wait_for(
        "запрос пересобран под предложение",
        || sensor() == Some((Duration::from_millis(4), 100)),
    );
    assert_eq!(repeating_requests(&fake), before + 1, "ровно одна пересборка");

    // Двадцать одинаковых предложений: поток камеры просыпается, но запрос не трогает.
    let handled_before = handle.status().counters.commands_handled;
    for _ in 0..20 {
        channel.publish(proposal(0.4));
    }
    // Команда стоит в очереди после пробуждений (они идут раньше команд) — её ответ их замыкает.
    handle
        .apply_controls(ManualControl::AUTOMATIC)
        .expect("команда принята");
    handle
        .wait_until_status(WAIT, |status| status.counters.commands_handled > handled_before)
        .expect("команда обработана");
    assert_eq!(repeating_requests(&fake), before + 1, "повторы запрос не перестроили");
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn a_proposal_published_before_binding_is_applied_right_after_bind_without_new_publications() {
    let fake = FakeBackendControl::new();
    let config = protected_engine_config();
    let channel = ExposureProposalChannel::new();
    let wiring = ThreeAWiring::own(&config.three_a, channel.clone(), MainsFrequency::Hz50);
    let (runtime, handle) = CameraRuntime::spawn_with_wiring(config, AppPresence::Foreground, fake.factory(), wiring)
        .expect("поток запускается");
    handle
        .open(CameraTarget::Role(LensRole::Wide))
        .expect("команда принята");
    handle
        .configure(StreamMode {
            raw_format: RawFormat::Raw10,
            readout: ReadoutAspect::SixteenByNine,
            frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        })
        .expect("команда принята");
    handle.start_streaming(CaptureIntent::Preview).expect("команда принята");
    wait_for("поток пошёл", || fake.is_repeating());
    fake.complete_frames(3, Duration::from_millis(16));
    handle
        .wait_until_status(WAIT, |status| status.three_a.last_result.is_some())
        .expect("результаты дошли до движка");

    // Рендер успел опубликовать до привязки: толчок ушёл в пустоту.
    let (_, delivery) = channel.publish(proposal(0.4));
    assert_eq!(delivery, camera_contract::auto_exposure::SignalDelivery::NotBoundYet);
    let sensor = || match fake.repeating_request().map(|(_, controls)| controls.exposure) {
        Some(ExposurePlan::Sensor { exposure_time, iso }) => Some((exposure_time.duration(), iso.value())),
        _ => None,
    };
    assert_eq!(
        sensor(),
        Some((Duration::from_millis(10), 400)),
        "пока не привязаны — старт из конфига"
    );

    channel.refresh_signal().bind(Arc::new(handle.three_a_waker()));
    wait_for(
        "bind дослал толчок, запрос пересобран",
        || sensor() == Some((Duration::from_millis(4), 100)),
    );
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}
