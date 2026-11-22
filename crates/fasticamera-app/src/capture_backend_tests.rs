//! Тесты набора «кто снимает»: режимы видны в диагностике, запасной фейк называет причину, настоящий снимок
//! поднимается и отдаёт счётчики, счёт принятых запросов не зависит от режима.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use android_shell::still_orphan_sweep::OrphanSweepReport;
use shell_contract::clock::MonotonicNanos;

use super::*;
use crate::photo_config::PhotoConfig;
use crate::still_runtime::{StillRuntime, StillRuntimeDeps, start_still_or_fallback};
use crate::still_test_support::{
    AnyCameraCatalog, CountingSound, FakeFormats, FakeJpeg, FakeRenderPort, FakeStore, FixedClock, FixedSweep,
    RefusingHeif, RenderMode, shot,
};

fn screen_config() -> CameraScreenConfig {
    CameraScreenConfig {
        fake_capture_delay_ms: 0,
        fake_capture_secured_delay_ms: 0,
        ..CameraScreenConfig::default()
    }
}

fn deps() -> StillRuntimeDeps {
    StillRuntimeDeps {
        store: Arc::new(FakeStore::default()),
        heif: Arc::new(RefusingHeif),
        jpeg: Arc::new(FakeJpeg),
        formats: Arc::new(FakeFormats),
        clock: Arc::new(FixedClock),
        cameras: Arc::new(AnyCameraCatalog),
        render: FakeRenderPort::new(RenderMode::Accepting),
        shutter_sound: Arc::new(CountingSound::default()),
        orphan_sweep: Arc::new(FixedSweep(OrphanSweepReport::Removed { count: 2 })),
    }
}

fn backend_of(
    assembly: crate::still_runtime::StillAssembly,
) -> (Option<photo_output::service::StillService>, CaptureBackend) {
    match assembly.capture {
        CaptureWiring::Backend(backend) => (assembly.service, backend),
        CaptureWiring::Fake => panic!("ожидали готовый набор"),
    }
}

#[test]
fn plain_fake_reports_fake_mode_and_counts_submitted_requests() {
    let backend = CaptureBackend::fake(&screen_config());
    backend.service().submit(shot(1), MonotonicNanos::from_nanos(0));

    let diagnostics = backend.diagnostics();

    assert_eq!(diagnostics.mode, CaptureMode::Fake);
    assert!(diagnostics.real.is_none());
    assert_eq!(diagnostics.fake.expect("фейк").counters.submitted, 1);
    assert_eq!(backend.accepted_total(), 1);
    assert_eq!(backend.journal().len(), 1);
}

#[test]
fn missing_prerequisite_gives_a_visible_fallback_with_the_reason() {
    let assembly = start_still_or_fallback(
        &PhotoConfig::default(),
        &screen_config(),
        Err("камера недоступна: нет разрешения".to_owned()),
    );
    let (service, backend) = backend_of(assembly);

    assert!(service.is_none(), "службы нет, поток не запускался");
    let diagnostics = backend.diagnostics();
    assert_eq!(
        diagnostics.mode,
        CaptureMode::FakeFallback {
            reason: "камера недоступна: нет разрешения".to_owned()
        }
    );
    assert!(diagnostics.fake.is_some() && diagnostics.real.is_none());
}

#[test]
fn service_that_fails_to_start_falls_back_with_its_error_text() {
    let mut config = PhotoConfig::default();
    config.service.event_queue_capacity = 0;

    let (service, backend) = backend_of(start_still_or_fallback(&config, &screen_config(), Ok(deps())));

    assert!(service.is_none());
    match backend.diagnostics().mode {
        CaptureMode::FakeFallback { reason } => assert!(reason.contains("event_queue_capacity"), "{reason}"),
        other => panic!("ожидали запасной фейк, получили {other:?}"),
    }
}

#[test]
fn healthy_start_gives_the_real_mode_with_service_counters_and_sweep_report() {
    let (service, backend) = backend_of(start_still_or_fallback(
        &PhotoConfig::default(),
        &screen_config(),
        Ok(deps()),
    ));
    let service = service.expect("служба поднята и принадлежит обвязке");
    backend.service().submit(shot(1), MonotonicNanos::from_nanos(0));

    let diagnostics = backend.diagnostics();

    assert_eq!(diagnostics.mode, CaptureMode::Real);
    assert!(diagnostics.fake.is_none());
    let real = diagnostics.real.expect("настоящая диагностика");
    assert_eq!(real.service.accepted, 1);
    assert_eq!(real.capture.accepted, 1);
    assert_eq!(real.orphan_drafts, OrphanSweepReport::Removed { count: 2 });
    assert_eq!(backend.accepted_total(), 1, "счёт принятых одинаков в обоих режимах");
    assert_eq!(service.stats().pending_now, 1);
}

#[test]
fn diagnostics_serialize_with_a_visible_mode_tag() {
    let backend = CaptureBackend::fake_fallback(&screen_config(), "рендер недоступен");
    let json = serde_json::to_value(backend.diagnostics()).unwrap();
    assert_eq!(json["mode"], "fake_fallback");
    assert_eq!(json["reason"], "рендер недоступен");
    assert_eq!(json["fake"]["counters"]["submitted"], 0);
}

#[test]
fn runtime_can_be_started_directly_and_stats_are_readable() {
    let runtime = StillRuntime::start(&PhotoConfig::default(), deps()).expect("старт");
    assert_eq!(runtime.stats().accepted, 0);
}

#[test]
fn real_backend_plays_the_platform_shutter_sound_and_the_fake_stays_silent() {
    use crate::still_test_support::{StillRig, lock};

    let rig = StillRig::start();
    let outcome = rig.backend.shutter_sound().play_shutter_sound();
    assert_eq!(
        outcome,
        shell_contract::commands::CommandOutcome::Queued,
        "звук идёт на платформу"
    );
    assert_eq!(*lock(&rig.sound.calls), 1);

    let fake = CaptureBackend::fake(&screen_config());
    assert_eq!(
        fake.shutter_sound().play_shutter_sound(),
        shell_contract::commands::CommandOutcome::NotAttached,
        "у фейка звука нет, просьба считается в диагностике"
    );
    assert_eq!(fake.diagnostics().fake.expect("фейк").shutter_sound_requests, 1);
}

#[test]
fn a_real_backend_sends_shots_to_a_scenario_fake_and_the_real_service_returns_after_it() {
    let (service, mut backend) = backend_of(start_still_or_fallback(
        &PhotoConfig::default(),
        &screen_config(),
        Ok(deps()),
    ));
    let service = service.expect("служба поднята");
    let executor_view = backend.service();

    let substitution = backend.substitute_fake_for_scenario("screen", &screen_config());
    let during = executor_view.submit(shot(1), MonotonicNanos::from_nanos(0));
    let diagnostics_during = backend.diagnostics();
    let restored = backend.restore_after_scenario();
    let after = executor_view.submit(shot(2), MonotonicNanos::from_nanos(0));

    assert_eq!(substitution, FakeSubstitution::Substituted);
    assert_eq!(during, crate::camera_screen_capture::SubmitOutcome::Accepted);
    assert_eq!(
        diagnostics_during.mode,
        CaptureMode::FakeForScenario { scenario: "screen" },
        "в диагностике виден фейк с названием сценария"
    );
    assert_eq!(diagnostics_during.fake.expect("счётчики фейка").counters.submitted, 1);
    assert!(restored);
    assert_eq!(after, crate::camera_screen_capture::SubmitOutcome::Accepted);
    // Настоящая служба получила только второй запрос: первый в галерею не ушёл бы ни при каком исходе.
    assert_eq!(service.stats().accepted, 1);
    let diagnostics_after = backend.diagnostics();
    assert_eq!(diagnostics_after.mode, CaptureMode::Real);
    assert_eq!(
        diagnostics_after.real.expect("настоящая диагностика").capture.accepted,
        1
    );
}

#[test]
fn the_scenario_fake_keeps_its_own_journal_and_counter_apart_from_the_real_one() {
    let (_service, mut backend) = backend_of(start_still_or_fallback(
        &PhotoConfig::default(),
        &screen_config(),
        Ok(deps()),
    ));
    backend.substitute_fake_for_scenario("screen", &screen_config());
    backend.service().submit(shot(7), MonotonicNanos::from_nanos(0));

    assert_eq!(backend.accepted_total(), 1);
    assert_eq!(
        backend
            .journal()
            .iter()
            .map(|request| request.request.0)
            .collect::<Vec<_>>(),
        vec![7]
    );

    backend.restore_after_scenario();
    assert_eq!(backend.accepted_total(), 0, "настоящий счёт не тронут фейком");
    assert!(backend.journal().is_empty());
}

#[test]
fn substitution_is_not_needed_when_a_fake_already_works_and_restore_then_says_nothing_was_changed() {
    for mut backend in [
        CaptureBackend::fake(&screen_config()),
        CaptureBackend::fake_fallback(&screen_config(), "нет рендера"),
    ] {
        let mode_before = backend.diagnostics().mode;
        assert_eq!(
            backend.substitute_fake_for_scenario("screen", &screen_config()),
            FakeSubstitution::NotNeeded
        );
        assert_eq!(backend.diagnostics().mode, mode_before, "режим не изменился");
        assert!(!backend.restore_after_scenario());
    }
}

#[test]
fn a_repeated_substitution_does_not_stack_and_one_restore_undoes_it() {
    let (_service, mut backend) = backend_of(start_still_or_fallback(
        &PhotoConfig::default(),
        &screen_config(),
        Ok(deps()),
    ));
    assert_eq!(
        backend.substitute_fake_for_scenario("screen", &screen_config()),
        FakeSubstitution::Substituted
    );
    assert_eq!(
        backend.substitute_fake_for_scenario("screen", &screen_config()),
        FakeSubstitution::NotNeeded
    );
    assert!(backend.restore_after_scenario());
    assert_eq!(backend.diagnostics().mode, CaptureMode::Real);
}

#[test]
fn the_fake_mode_serializes_with_its_scenario_name() {
    let (_service, mut backend) = backend_of(start_still_or_fallback(
        &PhotoConfig::default(),
        &screen_config(),
        Ok(deps()),
    ));
    backend.substitute_fake_for_scenario("screen", &screen_config());
    let json = serde_json::to_value(backend.diagnostics()).unwrap();
    assert_eq!(json["mode"], "fake_for_scenario");
    assert_eq!(json["scenario"], "screen");
}
