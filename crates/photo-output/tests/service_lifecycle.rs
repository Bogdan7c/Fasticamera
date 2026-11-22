//! Сроки, опоздавшие события, остановка службы и конфиг.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — способ сообщить о провале"
)]

mod service_support;

use photo_output::service::{
    AdmissionRefused, DevelopSubmitError, ShutdownReport, StillServiceConfig, StillServiceStartError,
};
use service_support::{DraftState, EncoderEvent, Rig, fast_config};
use still_contract::develop::DevelopEvent;
use still_contract::phase::{StillFailure, StillPhase};
use still_contract::plan::{PrimaryFormat, RawOutput, StillId};

fn failure_of(phase: StillPhase) -> StillFailure {
    match phase {
        StillPhase::Failed { failure, .. } => failure,
        other => panic!("ждали Failed, пришло {other:?}"),
    }
}

#[test]
fn develop_deadline_fails_the_still_and_late_event_is_counted_and_ignored() {
    let rig = Rig::start_with(StillServiceConfig {
        develop_timeout_ms: 80,
        ..fast_config()
    });
    rig.admit(1);
    rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);

    // Рендер молчит дольше срока.
    let failure = failure_of(rig.wait_final(1));

    assert!(
        matches!(&failure, StillFailure::Develop(text) if text.contains("80 мс")),
        "{failure:?}"
    );
    assert_eq!(rig.heif.events(), vec![EncoderEvent::Aborted]);
    assert!(
        rig.store
            .log
            .drafts()
            .iter()
            .all(|draft| draft.state == DraftState::Discarded)
    );
    let stats = rig.handle.stats();
    assert_eq!((stats.timed_out, stats.failed_develop, stats.pending_now), (1, 1, 0));

    // Опоздавший итог: не публикует файл, не создаёт вторую итоговую фазу.
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });
    service_support::wait_until("опоздавшее событие учтено", || {
        rig.handle.stats().late_events == 1
    });
    assert_eq!(rig.store.log.published_count(), 0);
    assert_eq!(rig.phases.of(StillId(1)).len(), 2, "Secured и один Failed");
}

#[test]
fn capture_deadline_frees_the_slot_when_render_never_answers() {
    let rig = Rig::start_with(StillServiceConfig {
        capture_timeout_ms: 60,
        max_pending_stills: 1,
        ..fast_config()
    });
    rig.admit(1);

    let failure = failure_of(rig.wait_final(1));

    assert!(
        matches!(&failure, StillFailure::Develop(text) if text.contains("не захвачен")),
        "{failure:?}"
    );
    assert!(rig.handle.try_admit(StillId(2)).is_ok());
}

#[test]
fn late_secured_gets_unavailable_so_render_releases_the_frame() {
    let rig = Rig::start_with(StillServiceConfig {
        capture_timeout_ms: 60,
        ..fast_config()
    });
    rig.admit(1);
    rig.wait_final(1);

    let offer = rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);

    assert!(matches!(
        offer,
        still_contract::develop::PrimaryTargetOffer::Unavailable { .. }
    ));
    assert!(
        rig.store.log.drafts().is_empty(),
        "для опоздавшего снимка ничего не открывается"
    );
    assert_eq!(rig.handle.stats().late_events, 1);
}

#[test]
fn shutdown_with_unfinished_still_fails_it_and_removes_drafts() {
    let rig = Rig::start();
    rig.admit(1);
    rig.admit(2); // этот так и не дождался кадра
    rig.secure(1, PrimaryFormat::Heif, RawOutput::Dng);
    let handle = rig.handle.clone();
    let phases = rig.phases.clone();
    let store = rig.store.clone();
    let heif = rig.heif.clone();

    let report = rig.service.shutdown();

    assert_eq!(report, ShutdownReport::Stopped);
    for id in [1, 2] {
        assert_eq!(
            failure_of(phases.wait_final(StillId(id), service_support::WAIT)),
            StillFailure::ServiceUnavailable
        );
    }
    assert_eq!(heif.events(), vec![EncoderEvent::Aborted]);
    assert_eq!(store.log.published_count(), 0);
    assert!(
        store
            .log
            .drafts()
            .iter()
            .all(|draft| draft.state == DraftState::Discarded)
    );
    let stats = handle.stats();
    assert_eq!((stats.failed_unavailable, stats.pending_now), (2, 0));

    // После остановки служба ничего не принимает.
    assert_eq!(
        handle.try_admit(StillId(3)).unwrap_err(),
        AdmissionRefused::ServiceStopped
    );
    assert_eq!(
        handle.submit_develop_event(DevelopEvent::PrimaryRendered { id: StillId(1) }),
        Err(DevelopSubmitError::ServiceStopped)
    );
}

#[test]
fn ticket_handed_over_during_shutdown_still_gets_a_final_phase() {
    let rig = Rig::start();
    rig.admit(1);
    // Рабочий поток застревает в хранилище: `Shutdown` встанет в очередь за его работой.
    rig.store.create_gate.close();
    rig.send(DevelopEvent::Secured {
        facts: service_support::facts_for(1, PrimaryFormat::Heif, RawOutput::Skip),
        raw: None,
    });
    service_support::wait_until("рабочий поток застрял в хранилище", || {
        rig.store.create_calls() == 1
    });
    let late_ticket = rig.handle.try_admit(StillId(2)).unwrap();
    let Rig {
        service,
        handle,
        phases,
        store,
        ..
    } = rig;
    let stopper = std::thread::spawn(move || service.shutdown());
    service_support::wait_until("допуск закрыт остановкой", || {
        handle.try_admit(StillId(99)).err() == Some(AdmissionRefused::ServiceStopped)
    });
    // Остановка уже просит поток завершиться (запись в очередь идёт сразу за закрытием допуска).
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Билет попадает в очередь уже ПОСЛЕ `Shutdown`: раньше он так и пропадал без итога.
    let handed_over = handle.begin_still(late_ticket);
    store.create_gate.open();

    assert_eq!(stopper.join().unwrap(), ShutdownReport::Stopped);
    assert_eq!(
        handed_over,
        Ok(()),
        "очередь ещё принимала билет, значит итог обязан прийти"
    );
    assert_eq!(
        failure_of(phases.wait_final(StillId(2), service_support::WAIT)),
        StillFailure::ServiceUnavailable
    );
    assert_eq!(handle.stats().pending_now, 0);
}

#[test]
fn worker_panic_fails_every_live_still_and_stops_the_service() {
    let rig = Rig::start();
    rig.admit(1);
    rig.admit(2);
    rig.store.panic_on_create();

    // Обработка `Secured` первого снимка паникует внутри платформенного слоя.
    rig.send(DevelopEvent::Secured {
        facts: service_support::facts_for(1, PrimaryFormat::Heif, RawOutput::Skip),
        raw: None,
    });

    for id in [1, 2] {
        assert_eq!(
            failure_of(rig.wait_final(id)),
            StillFailure::ServiceUnavailable,
            "снимок {id} обязан получить итог"
        );
    }
    let stats = rig.handle.stats();
    assert_eq!(
        (stats.worker_panics, stats.failed_unavailable, stats.pending_now),
        (1, 2, 0)
    );
    // Служба мертва честно: новые снимки не принимаются.
    assert_eq!(
        rig.handle.try_admit(StillId(3)).unwrap_err(),
        AdmissionRefused::ServiceStopped
    );
}

#[test]
fn dropping_the_service_stops_it_like_shutdown() {
    let rig = Rig::start();
    rig.admit(1);
    let phases = rig.phases.clone();

    drop(rig);

    assert_eq!(
        failure_of(phases.wait_final(StillId(1), service_support::WAIT)),
        StillFailure::ServiceUnavailable
    );
}

#[test]
fn config_validation_rejects_each_bad_value() {
    let bad = [
        (
            "max_pending_stills",
            StillServiceConfig {
                max_pending_stills: 0,
                ..fast_config()
            },
        ),
        (
            "event_queue_capacity",
            StillServiceConfig {
                event_queue_capacity: 0,
                ..fast_config()
            },
        ),
        (
            "max_open_encoders",
            StillServiceConfig {
                max_open_encoders: 0,
                ..fast_config()
            },
        ),
        (
            "capture_timeout_ms",
            StillServiceConfig {
                capture_timeout_ms: 0,
                ..fast_config()
            },
        ),
        (
            "develop_timeout_ms",
            StillServiceConfig {
                develop_timeout_ms: 0,
                ..fast_config()
            },
        ),
        (
            "heif_finish_timeout_ms",
            StillServiceConfig {
                heif_finish_timeout_ms: 0,
                ..fast_config()
            },
        ),
        (
            "shutdown_timeout_ms",
            StillServiceConfig {
                shutdown_timeout_ms: 0,
                ..fast_config()
            },
        ),
        (
            "heif_quality",
            StillServiceConfig {
                heif_quality: 101,
                ..fast_config()
            },
        ),
        (
            "jpeg_quality",
            StillServiceConfig {
                jpeg_quality: 101,
                ..fast_config()
            },
        ),
        (
            "media_directory",
            StillServiceConfig {
                media_directory: "  ".to_owned(),
                ..fast_config()
            },
        ),
    ];
    for (field, config) in bad {
        assert_eq!(config.validate().unwrap_err().field, field, "{field}");
    }
    assert_eq!(StillServiceConfig::default().validate(), Ok(()));
    let boundary = StillServiceConfig {
        heif_quality: 100,
        jpeg_quality: 0,
        ..fast_config()
    };
    assert_eq!(boundary.validate(), Ok(()));
}

#[test]
fn config_defaults_match_decision_140_and_unknown_fields_are_rejected() {
    let defaults = StillServiceConfig::default();
    assert_eq!(defaults.max_pending_stills, 10);
    assert_eq!(defaults.media_directory, "DCIM/Camera");
    assert_eq!(defaults.max_open_encoders, 2);

    let parsed: StillServiceConfig = serde_json::from_str(r#"{"max_pending_stills": 3}"#).unwrap();
    assert_eq!(parsed.max_pending_stills, 3);
    assert_eq!(
        parsed.develop_timeout_ms, defaults.develop_timeout_ms,
        "остальное — по умолчанию"
    );

    let typo = serde_json::from_str::<StillServiceConfig>(r#"{"max_pending_stil": 3}"#);
    assert!(typo.is_err(), "опечатка в имени поля не должна молча проглатываться");
}

#[test]
fn invalid_config_does_not_start_the_service() {
    use photo_output::service::{PhotoFileFormats, StillService, StillServiceDeps};
    use std::sync::Arc;
    let rig = Rig::start();
    let deps = StillServiceDeps {
        store: rig.store.clone(),
        heif: rig.heif.clone(),
        jpeg: rig.jpeg.clone(),
        formats: Arc::new(PhotoFileFormats),
        clock: Arc::new(service_support::FixedClock),
        cameras: rig.cameras.clone(),
        phases: rig.phases.clone(),
        targets: rig.targets.clone(),
    };

    let result = StillService::start(
        StillServiceConfig {
            max_pending_stills: 0,
            ..fast_config()
        },
        deps,
    );

    assert!(matches!(result, Err(StillServiceStartError::InvalidConfig(_))));
}
