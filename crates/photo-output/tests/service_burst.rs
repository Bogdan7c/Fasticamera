//! Серия нажатий: «кадр захвачен» не ждёт файлов прошлых снимков (решение 140), а открытых кодеров
//! одновременно не больше `max_open_encoders`. Всё через настоящий рабочий поток службы.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — способ сообщить о провале"
)]

mod service_support;

use std::time::{Duration, Instant};

use photo_output::service::StillServiceConfig;
use service_support::{Rig, facts_for, raw_for};
use still_contract::develop::{DevelopEvent, PrimaryTargetOffer};
use still_contract::phase::StillPhase;
use still_contract::plan::{PrimaryFormat, RawOutput, StillId};

/// Задержка каждого тяжёлого платформенного вызова в серии.
const SLOW_CALL: Duration = Duration::from_millis(100);
/// Сколько снимков в серии.
const BURST: u64 = 10;

/// Конфиг с запасом по срокам: серия нарочно длиннее обычного, а срыв по сроку здесь не предмет проверки.
fn patient_config() -> StillServiceConfig {
    StillServiceConfig {
        capture_timeout_ms: 30_000,
        develop_timeout_ms: 60_000,
        heif_finish_timeout_ms: 5_000,
        shutdown_timeout_ms: 5_000,
        ..StillServiceConfig::default()
    }
}

/// Нажатие: допуск и «кадр захвачен» от рендера без ожидания цели. Возвращает момент передачи события.
fn press(rig: &Rig, id: u64) -> Instant {
    rig.admit(id);
    let secured_sent = Instant::now();
    rig.handle
        .submit_develop_event(DevelopEvent::Secured {
            facts: facts_for(id, PrimaryFormat::Heif, RawOutput::Dng),
            raw: Some(raw_for()),
        })
        .expect("событие принято");
    secured_sent
}

/// Рендер-подделка: проявляет снимки строго по очереди (берёт цель, «проявляет» 20 мс, сообщает итог).
fn spawn_render(rig: &Rig, count: u64) -> std::thread::JoinHandle<()> {
    let handle = rig.handle.clone();
    let targets = rig.targets.clone();
    std::thread::spawn(move || {
        for id in 1..=count {
            let offer = targets.wait_offer(StillId(id), Duration::from_secs(60));
            assert!(matches!(offer, PrimaryTargetOffer::Ready { .. }), "{offer:?}");
            std::thread::sleep(Duration::from_millis(20));
            handle
                .submit_develop_event(DevelopEvent::PrimaryRendered { id: StillId(id) })
                .expect("итог принят");
        }
    })
}

#[test]
fn secured_phase_is_reported_at_handover_while_worker_is_busy_with_previous_stills() {
    let rig = Rig::start_with(patient_config());
    rig.store.create_latency.set(SLOW_CALL);
    rig.heif.open_latency.set(SLOW_CALL);
    rig.heif.finish_latency.set(SLOW_CALL);
    rig.formats.dng_latency.set(SLOW_CALL);
    let render = spawn_render(&rig, BURST);

    let mut delays = Vec::new();
    for id in 1..=BURST {
        std::thread::sleep(Duration::from_millis(100));
        let secured_sent = press(&rig, id);
        rig.phases.wait_secured(StillId(id), service_support::WAIT);
        let secured_at = rig.phases.secured_at(StillId(id)).expect("Secured пришёл");
        delays.push(secured_at.saturating_duration_since(secured_sent));
    }

    // Рабочий поток на каждый снимок тратит ≈ 0,5 с, а нажатия идут раз в 0,1 с: если «кадр захвачен»
    // ждёт поток, задержка растёт с каждым снимком. Здесь она не зависит от потока вовсе.
    for (index, delay) in delays.iter().enumerate() {
        assert!(
            *delay < Duration::from_millis(50),
            "Secured снимка {} опоздал на {delay:?}; все задержки: {delays:?}",
            index + 1
        );
    }
    // Серия при этом сохраняется целиком, а Secured каждого снимка идёт раньше его итога.
    for id in 1..=BURST {
        assert!(matches!(rig.wait_final(id), StillPhase::Saved { .. }));
        let phases = rig.phases.of(StillId(id));
        assert!(matches!(phases.first(), Some(StillPhase::Secured { .. })), "{phases:?}");
        assert_eq!(phases.len(), 2, "один Secured и один итог: {phases:?}");
    }
    render.join().unwrap();
    assert_eq!(rig.handle.stats().saved, BURST);
}

/// Нажатие без ожидания: допуск и `Secured` нужного формата.
fn press_as(rig: &Rig, id: u64, primary: PrimaryFormat, raw: RawOutput) {
    rig.admit(id);
    rig.handle
        .submit_develop_event(DevelopEvent::Secured {
            facts: facts_for(id, primary, raw),
            raw: (raw == RawOutput::Dng).then(raw_for),
        })
        .expect("событие принято");
}

#[test]
fn open_encoders_never_exceed_the_limit_and_targets_are_handed_out_in_secured_order() {
    let rig = Rig::start_with(StillServiceConfig {
        max_open_encoders: 2,
        ..patient_config()
    });
    for id in 1..=5 {
        press_as(&rig, id, PrimaryFormat::Heif, RawOutput::Dng);
    }

    // Двум первым цель выдана сразу, остальные ждут места; DNG записан у всех пятерых.
    rig.targets.wait_offer(StillId(1), service_support::WAIT);
    rig.targets.wait_offer(StillId(2), service_support::WAIT);
    service_support::wait_until("DNG всех снимков записаны", || {
        rig.store
            .log
            .drafts()
            .iter()
            .filter(|draft| draft.bytes == service_support::FakeFormats::DNG_BYTES)
            .count()
            == 5
    });
    let stats = rig.handle.stats();
    assert_eq!((stats.encoders_open_now, stats.encoder_offers_deferred), (2, 3));
    assert_eq!(rig.heif.opens().len(), 2, "третий кодер не открыт, пока нет места");
    assert_eq!(rig.targets.count(), 0, "ждущим снимкам цель не выдана");

    // Первый снимок готов: место освободилось, цель получает третий (строго по порядку).
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });
    assert!(matches!(rig.wait_final(1), StillPhase::Saved { .. }));
    rig.targets.wait_offer(StillId(3), service_support::WAIT);
    assert_eq!(rig.heif.opens().len(), 3);
    assert_eq!(rig.targets.count(), 0, "четвёртому и пятому места ещё нет");

    for id in 2..=5 {
        if id > 3 {
            rig.targets.wait_offer(StillId(id), service_support::WAIT);
        }
        rig.send(DevelopEvent::PrimaryRendered { id: StillId(id) });
        assert!(matches!(rig.wait_final(id), StillPhase::Saved { .. }));
    }
    assert!(rig.heif.peak_open() <= 2, "пик кодеров: {}", rig.heif.peak_open());
    let order: Vec<u64> = rig.targets.offer_order().iter().map(|id| id.0).collect();
    assert_eq!(order, vec![1, 2, 3, 4, 5]);
    assert_eq!(rig.handle.stats().encoders_open_now, 0);
}

#[test]
fn jpeg_target_is_never_deferred_by_the_encoder_limit() {
    let rig = Rig::start_with(StillServiceConfig {
        max_open_encoders: 1,
        ..patient_config()
    });
    press_as(&rig, 1, PrimaryFormat::Heif, RawOutput::Skip);
    press_as(&rig, 2, PrimaryFormat::Heif, RawOutput::Skip);
    press_as(&rig, 3, PrimaryFormat::Jpeg, RawOutput::Skip);

    // Единственное место занято первым HEIF, второй HEIF ждёт, а JPEG получает цель сразу.
    rig.targets.wait_offer(StillId(1), service_support::WAIT);
    let jpeg_offer = rig.targets.wait_offer(StillId(3), service_support::WAIT);
    assert!(matches!(
        jpeg_offer,
        PrimaryTargetOffer::Ready {
            target: still_contract::develop::PrimaryTarget::Readback,
            ..
        }
    ));
    assert_eq!(rig.targets.count(), 0, "второй HEIF всё ещё без цели");
    assert_eq!(rig.handle.stats().encoders_open_now, 1);
}

#[test]
fn failed_finish_still_frees_the_encoder_place_for_the_next_still() {
    let rig = Rig::start_with(StillServiceConfig {
        max_open_encoders: 1,
        ..patient_config()
    });
    rig.heif.fail_finish();
    press_as(&rig, 1, PrimaryFormat::Heif, RawOutput::Skip);
    press_as(&rig, 2, PrimaryFormat::Heif, RawOutput::Skip);
    rig.targets.wait_offer(StillId(1), service_support::WAIT);

    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });

    // Первый не сохранился (finish упал), но место освободилось: второй получает цель.
    assert!(matches!(rig.wait_final(1), StillPhase::Failed { .. }));
    rig.targets.wait_offer(StillId(2), service_support::WAIT);
    assert_eq!(rig.heif.peak_open(), 1);
}

#[test]
fn still_expired_while_waiting_for_an_encoder_releases_the_render_and_its_drafts() {
    let rig = Rig::start_with(StillServiceConfig {
        max_open_encoders: 1,
        develop_timeout_ms: 200,
        ..patient_config()
    });
    press_as(&rig, 1, PrimaryFormat::Heif, RawOutput::Dng);
    press_as(&rig, 2, PrimaryFormat::Heif, RawOutput::Dng);

    // Оба срываются по сроку проявления; второму цель так и не выдавали, поэтому рендеру сразу
    // отвечают «цели нет» — он не ждёт собственный срок.
    assert!(matches!(rig.wait_final(1), StillPhase::Failed { .. }));
    assert!(matches!(rig.wait_final(2), StillPhase::Failed { .. }));
    let offers: Vec<_> = (1..=2)
        .map(|id| rig.targets.wait_offer(StillId(id), service_support::WAIT))
        .collect();
    assert!(matches!(offers[0], PrimaryTargetOffer::Ready { .. }));
    assert!(
        matches!(offers[1], PrimaryTargetOffer::Unavailable { .. }),
        "{offers:?}"
    );
    assert_eq!(rig.heif.opens().len(), 1, "кодер второго никогда не открывался");
    assert_eq!(rig.store.log.published_count(), 0);
    assert_eq!(rig.store.log.discarded_count(), 4, "две пары черновиков");
    assert_eq!(rig.handle.stats().encoders_open_now, 0);
}

#[test]
fn capture_and_develop_deadlines_are_judged_by_arrival_time_not_processing_time() {
    let rig = Rig::start_with(StillServiceConfig {
        capture_timeout_ms: 300,
        develop_timeout_ms: 300,
        ..patient_config()
    });
    // Снимок 1 уже проявляется (цель выдана, рендер ещё работает).
    press_as(&rig, 1, PrimaryFormat::Heif, RawOutput::Skip);
    rig.targets.wait_offer(StillId(1), service_support::WAIT);
    // Снимок 2 допущен: его срок захвата идёт.
    rig.admit(2);

    // Рабочий поток застревает в хранилище на снимке 3 дольше обоих сроков.
    rig.store.create_gate.close();
    let calls_before = rig.store.create_calls();
    press_as(&rig, 3, PrimaryFormat::Heif, RawOutput::Skip);
    service_support::wait_until("рабочий поток застрял на снимке 3", || {
        rig.store.create_calls() > calls_before
    });

    // Пока поток стоит, вовремя приходят «кадр захвачен» снимка 2 и итог рендера снимка 1.
    rig.handle
        .submit_develop_event(DevelopEvent::Secured {
            facts: facts_for(2, PrimaryFormat::Heif, RawOutput::Skip),
            raw: None,
        })
        .unwrap();
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(2) });
    std::thread::sleep(Duration::from_millis(500));
    rig.store.create_gate.open();

    // Оба пришли до срока — значит оба сохраняются, хотя обработаны уже после срока.
    assert!(matches!(rig.wait_final(1), StillPhase::Saved { .. }));
    assert!(matches!(rig.wait_final(2), StillPhase::Saved { .. }));
    // Сорван по сроку только снимок 3: его итога не ждал никто.
    assert!(matches!(rig.wait_final(3), StillPhase::Failed { .. }));
    assert_eq!(rig.handle.stats().timed_out, 1);
}

#[test]
fn duplicate_secured_reports_the_phase_only_once() {
    let rig = Rig::start_with(patient_config());
    press_as(&rig, 1, PrimaryFormat::Heif, RawOutput::Skip);
    rig.targets.wait_offer(StillId(1), service_support::WAIT);

    rig.send(DevelopEvent::Secured {
        facts: facts_for(1, PrimaryFormat::Heif, RawOutput::Skip),
        raw: None,
    });
    service_support::wait_until(
        "повторный Secured учтён как опоздавший",
        || rig.handle.stats().late_events == 1,
    );
    rig.send(DevelopEvent::PrimaryRendered { id: StillId(1) });
    rig.wait_final(1);

    let secured = rig
        .phases
        .of(StillId(1))
        .into_iter()
        .filter(|phase| matches!(phase, StillPhase::Secured { .. }))
        .count();
    assert_eq!(secured, 1);
}
