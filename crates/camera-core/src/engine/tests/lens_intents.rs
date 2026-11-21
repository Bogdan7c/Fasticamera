//! Чтение намерений экрана движком: смена коэффициента внутри объектива ничего не трогает,
//! поколения, приоритет команд, переворот на фронт и обратно.

use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::LensIntent;

use super::lens_support::{LensRig, ultra_wide_id};
use super::*;
use crate::lens_status::LensSwitchState;

#[test]
fn hundred_ratio_intents_inside_one_lens_touch_neither_backend_nor_state() {
    let mut lens = LensRig::streaming_on_wide();
    let sessions_before = lens.rig.engine.status().counters.sessions_configured;
    for step in 0..100 {
        lens.intent(LensFacing::Back, 1.0 + step as f32 * 0.009);
    }
    assert!(lens.rig.fake.take_calls().is_empty(), "ни одного вызова бэкенда");
    assert!(lens.rig.fresh_events().is_empty(), "ни одного события");
    let status = lens.rig.engine.status();
    assert_eq!(status.counters.sessions_configured, sessions_before);
    assert_eq!(status.lens.switch, LensSwitchState::Steady { role: LensRole::Wide });
    assert!(status.lens.last_intent.is_some_and(|intent| intent.ratio.value() > 1.8));
}

#[test]
fn stale_generation_is_not_reapplied_over_a_later_command() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);
    lens.frames(1);
    assert_eq!(lens.rig.fake.open_camera(), Some(ultra_wide_id()));

    // Позже пришла команда: она побеждает, а повторный толчок без нового намерения ничего не делает.
    assert_eq!(
        lens.rig.send(CameraCommand::SwitchTo { role: LensRole::Wide }),
        CommandOutcome::Accepted
    );
    lens.rig.fake.take_calls();
    lens.poll();
    lens.poll();
    assert!(lens.rig.fake.take_calls().is_empty());
    assert_eq!(
        lens.rig.fake.open_camera(),
        Some(camera_contract::fixtures::camera_id("2"))
    );
}

#[test]
fn newer_intent_after_a_command_wins_over_the_command() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.send(CameraCommand::SwitchTo {
        role: LensRole::UltraWide,
    });
    lens.intent(LensFacing::Back, 1.5);
    assert_eq!(
        lens.rig.fake.open_camera(),
        Some(camera_contract::fixtures::camera_id("2")),
        "позднее намерение выбрало Wide"
    );
}

#[test]
fn intent_before_the_camera_is_requested_is_remembered_but_opens_nothing() {
    let mut lens = LensRig::closed();
    lens.intent(LensFacing::Back, 0.6);
    assert!(lens.rig.fake.take_calls().is_empty());
    let status = lens.rig.engine.status().lens;
    assert!(status.last_intent.is_some());
    assert_eq!(status.counters.intents_ignored, 0, "это не ошибка");
    assert_eq!(lens.rig.state_name(), "closed");
}

#[test]
fn intent_read_before_the_first_open_is_applied_right_after_it_and_the_stream_starts_on_that_lens() {
    let mut lens = LensRig::closed();
    lens.intent_discrete(LensFacing::Back, 0.6);
    assert!(lens.rig.fake.take_calls().is_empty(), "до Open камеру не открываем");

    // Команда просит основную камеру (умолчание продукта), а экран раньше выбрал ширик.
    assert_eq!(
        lens.rig.send(CameraCommand::Open { target: wide() }),
        CommandOutcome::Accepted
    );
    lens.rig.send(CameraCommand::Configure {
        mode: mode_raw10_4x3_at(60),
    });
    lens.rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    lens.frames(3);

    assert!(
        !lens
            .rig
            .fresh_events()
            .iter()
            .any(|event| matches!(event, CameraEvent::LensSwitched { .. })),
        "намерение до первого показа — первый показ, а не смена объектива"
    );
    assert_eq!(lens.rig.engine.status().lens.counters.switches_completed, 0);
    assert_eq!(
        lens.open_cameras(),
        vec![ultra_wide_id().to_string()],
        "основную камеру не открывали вовсе: отложенное намерение сменило цель до перестройки"
    );
    assert_eq!(
        lens.switch_state(),
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );
    assert_eq!(lens.rig.engine.status().lens.counters.intents_ignored, 0);
}

#[test]
fn a_deferred_intent_is_applied_once_and_does_not_override_a_later_command() {
    let mut lens = LensRig::closed();
    lens.intent_discrete(LensFacing::Back, 0.6);
    lens.rig.send(CameraCommand::Open { target: wide() });
    lens.rig.send(CameraCommand::Configure {
        mode: mode_raw10_4x3_at(60),
    });
    lens.rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    lens.frames(3);

    // Позднее явное решение (команда) побеждает; отложенное намерение второй раз не применяется.
    lens.rig.send(CameraCommand::SwitchTo { role: LensRole::Wide });
    lens.frames(3);
    lens.poll();
    lens.rig.send(CameraCommand::Close);
    lens.rig.send(CameraCommand::Open { target: wide() });
    lens.rig.send(CameraCommand::Configure {
        mode: mode_raw10_4x3_at(60),
    });
    lens.rig.send(CameraCommand::StartStreaming {
        intent: CaptureIntent::Preview,
    });
    lens.rig.fake.take_calls();
    lens.frames(3);
    assert_eq!(
        lens.switch_state(),
        LensSwitchState::Steady { role: LensRole::Wide },
        "после Close и новой Open поток на основной: старое намерение второй раз не применяется"
    );
}

#[test]
fn intent_published_before_the_catalog_is_read_is_applied_after_loading() {
    let intents = camera_contract::zoom::LensIntentChannel::new();
    let config = CameraEngineConfig::default();
    let fake = FakeBackendControl::new();
    let collector = Arc::new(Collector::default());
    let listener: Arc<dyn BackendEventListener> = Arc::clone(&collector) as Arc<dyn BackendEventListener>;
    let backend = fake.backend(BackendEventSink::new(listener));
    let wiring = ThreeAWiring::vendor(&config.three_a);
    let engine = CameraEngine::with_wiring(config, AppPresence::Foreground, backend, wiring)
        .with_lens_wiring(crate::lens_wiring::LensWiring::new(intents.clone()));
    let mut rig = Rig::from_parts(engine, fake, collector);

    intents.publish(LensIntent {
        facing: LensFacing::Back,
        ratio: camera_contract::zoom::ZoomRatio::new(1.0).expect("коэффициент"),
        gesture: camera_contract::zoom::ZoomGesture::Continuous,
    });
    rig.engine.refresh_lens_intent(rig.now);
    assert!(
        rig.engine.status().lens.last_intent.is_none(),
        "каталога нет — намерение не потрачено"
    );

    rig.engine.load_catalog(rig.now);
    assert!(
        rig.engine.status().lens.last_intent.is_some(),
        "после каталога прочитано"
    );
}

#[test]
fn intent_for_a_direction_without_cameras_is_counted_not_silently_dropped() {
    let mut lens = LensRig::closed();
    lens.rig
        .fake
        .set_cameras(vec![camera_contract::fixtures::main_wide_camera()]);
    lens.rig.engine.load_catalog(lens.rig.now);
    lens.intent(LensFacing::Front, 1.0);
    assert_eq!(lens.rig.engine.status().lens.counters.intents_ignored, 1);
    assert!(
        lens.rig
            .fake
            .take_calls()
            .iter()
            .all(|call| matches!(call, BackendCall::LoadCatalog))
    );
}

#[test]
fn front_flip_and_back_returns_to_the_remembered_back_lens_through_hysteresis() {
    let mut lens = LensRig::streaming_on_wide();
    lens.intent(LensFacing::Back, 0.6);
    lens.frames(1);
    assert_eq!(
        lens.switch_state(),
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );

    lens.intent(LensFacing::Front, 1.0);
    lens.frames(1);
    assert_eq!(
        lens.rig.fake.open_camera(),
        Some(camera_contract::fixtures::camera_id("1"))
    );
    assert_eq!(lens.switch_state(), LensSwitchState::Steady { role: LensRole::Front });

    // 1.02× — ниже порога гистерезиса от ширика: возвращаемся на ширик, а не на основную.
    lens.intent(LensFacing::Back, 1.02);
    assert_eq!(lens.rig.fake.open_camera(), Some(ultra_wide_id()));
    lens.frames(1);
    assert_eq!(
        lens.switch_state(),
        LensSwitchState::Steady {
            role: LensRole::UltraWide
        }
    );
}

#[test]
fn zoom_catalog_is_published_in_the_status_after_the_catalog_loads() {
    let lens = LensRig::closed();
    let status = lens.rig.engine.status().lens;
    let back = status.zoom_catalog.back.as_ref().expect("задняя шкала");
    assert!((back.range.min.value() - 0.593).abs() < 0.002);
    assert!(status.zoom_catalog.front.is_some());
    assert_eq!(status.switch, LensSwitchState::Idle);
    assert!(lens.rig.engine.zoom_diagnostics().iter().all(|diagnostic| matches!(
        diagnostic,
        crate::zoom::ZoomDiagnostic::MaxDigitalZoomNotReported { .. }
    )));
}
