//! Белая через настоящий поток камеры: ответ источника будит движок тем же толчком, что и AE, и не теряется,
//! если опубликован раньше, чем композиция привязала получателя толчка.

use std::sync::Arc;
use std::time::Duration;

use camera_contract::auto_exposure::{CameraTag, SignalDelivery};
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::fixtures::camera_id;
use camera_contract::white::{
    SceneIlluminant, WhiteAdjustment, WhiteAnswer, WhiteConfidence, WhiteOrigin, WhiteRequestId, WhiteSourceId,
    WhiteSourceOutcome, WhiteTransition,
};

use crate::backend::BackendFactory;
use crate::command::InvalidStateReason;
use crate::config::CameraEngineConfig;
use crate::fake::FakeBackendControl;
use crate::runtime::{CameraRuntime, ShutdownReport};
use crate::status::AppPresence;
use crate::three_a_wiring::ThreeAWiring;
use crate::white::{
    RememberedOrigin, WhiteMemoryLoad, WhiteMemoryRecord, WhiteRequestState, WhiteTakeReport, WhiteTakeState,
    WhiteWiring,
};

const WAIT: Duration = Duration::from_secs(10);

#[test]
fn an_answer_published_before_the_engine_is_bound_is_not_lost() {
    let white = WhiteWiring::default();
    let answers = white.answers.clone();
    let directives = white.directives.clone();
    let config = CameraEngineConfig::default();
    let wiring = ThreeAWiring::vendor(&config.three_a).with_white(white);
    let fake = FakeBackendControl::new();
    let (runtime, handle) = CameraRuntime::spawn_with_wiring(config, AppPresence::Background, fake.factory(), wiring)
        .expect("поток запускается");

    // Приложение возобновилось: движок завёл заявку номер 1.
    handle.enter_foreground().expect("команда принята");
    handle
        .wait_until_status(WAIT, |status| {
            matches!(status.three_a.white.request, WhiteRequestState::Waiting { .. })
        })
        .expect("заявка открытия выдана");
    assert_eq!(
        directives
            .latest()
            .and_then(|(_, directive)| directive.request)
            .map(|request| request.id),
        Some(WhiteRequestId(1))
    );

    // Источник ответил до того, как композиция привязала получателя толчка.
    let phone_white = WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(3_900.0).expect("температура"),
        tint: Tint::from_duv(0.008).expect("оттенок"),
    };
    let (_, delivery) = answers.publish(WhiteAnswer {
        request: WhiteRequestId(1),
        camera: CameraTag::of(&camera_id("2")),
        measured_frame: None,
        outcome: WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(phone_white),
            confidence: WhiteConfidence::FULL,
        },
    });
    assert_eq!(delivery, SignalDelivery::NotBoundYet, "получателя ещё нет");
    assert_eq!(
        handle.status().three_a.white.held.origin,
        WhiteOrigin::Fallback,
        "пока толчка не было, движок не читал"
    );

    // Композиция привязала движок: ему досылается один толчок, и ответ применяется.
    answers.refresh_signal().bind(Arc::new(handle.three_a_waker()));
    let status = handle
        .wait_until_status(WAIT, |status| {
            status.three_a.white.held.origin == WhiteOrigin::Source(WhiteSourceId::PHONE)
        })
        .expect("ответ, лежавший в ящике, дошёл до движка");
    assert_eq!(status.three_a.white.held.setting, phone_white);
    assert_eq!(
        directives.latest().map(|(_, directive)| directive.white),
        Some(phone_white)
    );
    assert_eq!(status.three_a.white.stale_answers, 0);
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

fn white_of(kelvin: f64, duv: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::from_duv(duv).expect("оттенок"),
    }
}

#[test]
fn the_snapshot_before_the_first_publication_already_holds_the_remembered_white_and_the_config_range() {
    // Бэкенд ещё не создан (фабрика ждёт): движок не опубликовал ни одного снимка, статус — тот, что собрал
    // `EngineOutput::new`. Он не должен показывать запасной свет вместо запомненного и умолчания вместо конфига.
    let remembered = WhiteMemoryRecord {
        setting: white_of(3_150.0, -0.012),
        origin: RememberedOrigin::Manual,
    };
    let white = WhiteWiring {
        remembered: WhiteMemoryLoad::Remembered(remembered),
        ..WhiteWiring::default()
    };
    let mut config = CameraEngineConfig::default();
    config.three_a.white.max_kelvin = 9_000.0;
    config.three_a.white.max_abs_duv = 0.03;
    let wiring = ThreeAWiring::vendor(&config.three_a).with_white(white);
    let fake = FakeBackendControl::new();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let inner = fake.factory();
    let factory: BackendFactory = Box::new(move |sink| {
        let _ = gate.recv();
        inner(sink)
    });
    let (runtime, handle) =
        CameraRuntime::spawn_with_wiring(config, AppPresence::Background, factory, wiring).expect("поток запускается");

    let before = handle.status().three_a.white;
    assert_eq!(before.held.setting, remembered.setting);
    assert_eq!(before.held.origin, WhiteOrigin::Manual);
    assert_eq!(
        before.range.max_temperature.kelvin(),
        9_000.0,
        "предел из конфига, не умолчание"
    );
    assert_eq!(before.range.min_temperature.kelvin(), 2_000.0);
    assert_eq!(before.range.max_abs_tint.duv(), 0.03);

    // Движок собрался и опубликовал первый снимок: белая и пределы те же.
    release.send(()).expect("фабрика ждёт");
    let after = handle
        .wait_until_status(WAIT, |status| !status.catalog.cameras().is_empty())
        .expect("первый снимок движка")
        .three_a
        .white;
    assert_eq!(after.held, before.held);
    assert_eq!(after.range, before.range);
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}

#[test]
fn adjust_white_and_a_refused_take_are_visible_in_the_status_published_after_the_command() {
    let config = CameraEngineConfig::default();
    let wiring = ThreeAWiring::vendor(&config.three_a).with_white(WhiteWiring::default());
    let (runtime, handle) = CameraRuntime::spawn_with_wiring(
        config,
        AppPresence::Background,
        FakeBackendControl::new().factory(),
        wiring,
    )
    .expect("поток запускается");

    let adjust = handle
        .adjust_white(
            WhiteAdjustment::Temperature(white_of(4_200.0, 0.0).temperature),
            WhiteTransition::Immediate,
        )
        .expect("принята");
    let status = handle
        .wait_until_status(WAIT, |status| status.three_a.white.held.origin == WhiteOrigin::Manual)
        .expect("правка дошла");
    assert_eq!(status.three_a.white.held.setting.temperature.kelvin(), 4_200.0);
    assert_eq!(
        status.three_a.white.held.setting.tint.duv(),
        0.0,
        "оттенок остался запасным"
    );
    assert!(
        handle
            .drain_events(100)
            .iter()
            .any(|event| event.command_outcome(adjust) == Some(&crate::command::CommandOutcome::Accepted))
    );

    let take = handle
        .take_white(WhiteSourceId::PHONE, WhiteTransition::Immediate)
        .expect("принята");
    let status = handle
        .wait_until_status(WAIT, |status| status.three_a.white.last_take.is_some())
        .expect("отчёт о команде виден в снимке");
    assert_eq!(
        status.three_a.white.last_take,
        Some(WhiteTakeReport {
            command: take,
            state: WhiteTakeState::NotAsked {
                reason: InvalidStateReason::NotStreaming
            }
        })
    );
    assert_eq!(runtime.shutdown(WAIT), ShutdownReport::Stopped);
}
