//! Белая через настоящий поток камеры: ответ источника будит движок тем же толчком, что и AE, и не теряется,
//! если опубликован раньше, чем композиция привязала получателя толчка.

use std::sync::Arc;
use std::time::Duration;

use camera_contract::auto_exposure::{CameraTag, SignalDelivery};
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::fixtures::camera_id;
use camera_contract::white::{
    SceneIlluminant, WhiteAnswer, WhiteConfidence, WhiteOrigin, WhiteRequestId, WhiteSourceId, WhiteSourceOutcome,
};

use crate::config::CameraEngineConfig;
use crate::fake::FakeBackendControl;
use crate::runtime::{CameraRuntime, ShutdownReport};
use crate::status::AppPresence;
use crate::three_a_wiring::ThreeAWiring;
use crate::white::{WhiteRequestState, WhiteWiring};

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
