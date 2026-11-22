//! Сквозной случай настоящего снимка через НАСТОЯЩИЙ экран камеры и маршрутизатор: клавиша громкости → затвор →
//! эффект `Capture` → настоящая служба записи → события «рендера» → `CaptureSecured`/`CaptureFinished` в редьюсере
//! экрана → миниатюра. Платформа и поток рендера — подделки (`still_test_support`).

use still_contract::develop::DevelopEvent;
use still_contract::plan::StillId;

use super::*;
use crate::capture_backend::CaptureMode;
use crate::photo_config::PhotoConfig;
use crate::still_test_support::{
    DraftState, FakeJpeg, FakeRenderPort, RenderMode, facts_for, readback_image, start_still_runtime,
};

#[test]
fn volume_key_with_the_real_still_service_ends_in_a_published_file_and_a_thumbnail() {
    let render = FakeRenderPort::new(RenderMode::Accepting);
    let still = start_still_runtime(&PhotoConfig::default(), &render);
    let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
        CameraConfig::default(),
    ));
    harness.router.camera_screen = CameraScreenService::with_capture(instant_capture_config(), still.backend);
    harness.router.install_camera_screen();
    start_streaming(&mut harness);
    let mut clock = RenderClock::new();
    clock.frames(&harness, 2);

    volume_key(&mut harness, KeyPhase::Pressed, KeyRepeat::Initial, clock.now_ms);
    clock.frames(&harness, 3);
    volume_key(&mut harness, KeyPhase::Released, KeyRepeat::Initial, clock.now_ms);
    clock.frames(&harness, 3);

    // Нажатие дошло до «рендера» планом: один снимок, номер службы сквозной.
    let plans = render.plans();
    assert_eq!(plans.len(), 1, "одно нажатие — один план рендеру: {plans:?}");
    let id = plans[0].id;
    assert_eq!(id, StillId(1));
    let screen_request = harness.router.camera_screen().capture_journal()[0].request;

    // «Рендер» снимает кадр и проявляет картинку; служба пишет файл и сообщает фазы экрану.
    render.emit(
        id,
        DevelopEvent::Secured {
            facts: facts_for(id),
            raw: None,
        },
    );
    render.wait_readback_target(id);
    render.emit(
        id,
        DevelopEvent::PrimaryReadback {
            id,
            image: readback_image(),
        },
    );
    // Фазы лежат в канале экрана и разбираются на кадрах: крутим кадры, пока миниатюра не изменится.
    let deadline = std::time::Instant::now() + WAIT;
    while harness.router.camera_screen().report().expect("отчёт").thumbnail == ThumbnailState::Empty {
        assert!(std::time::Instant::now() < deadline, "миниатюра так и не появилась");
        clock.frames(&harness, 1);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }

    let report = harness.router.camera_screen().report().expect("отчёт");
    assert_eq!(
        report.thumbnail,
        ThumbnailState::Last { shot: screen_request },
        "CaptureFinished(Saved) от настоящей службы дошёл до редьюсера экрана под ЕГО номером запроса"
    );
    let diagnostics = harness.router.camera_screen().capture_diagnostics();
    assert_eq!(diagnostics.mode, CaptureMode::Real);
    let real = diagnostics.real.expect("настоящая диагностика");
    assert_eq!((real.service.saved, real.screen.saved_delivered), (1, 1));
    let drafts = still.store.drafts();
    assert_eq!(drafts.len(), 1);
    assert_eq!(drafts[0].state, DraftState::Published);
    assert!(
        drafts[0].bytes.ends_with(&FakeJpeg::BODY),
        "файл содержит JPEG платформы"
    );
    assert_eq!(harness.router.camera_screen().capture_requests_accepted(), 1);
    drop(still.service);
}

#[test]
fn a_scenario_asking_for_fake_capture_keeps_the_real_service_out_of_the_gallery_and_gives_it_back_after() {
    let render = FakeRenderPort::new(RenderMode::Accepting);
    let still = start_still_runtime(&PhotoConfig::default(), &render);
    let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
        CameraConfig::default(),
    ));
    harness.router.camera_screen = CameraScreenService::with_capture(instant_capture_config(), still.backend);
    harness.router.install_camera_screen();
    start_streaming(&mut harness);
    let mut clock = RenderClock::new();
    clock.frames(&harness, 2);
    assert_eq!(
        harness.router.camera_screen().capture_diagnostics().mode,
        CaptureMode::Real
    );

    // Сценарий, жмущий затвор ради проверки экрана (как `screen`): экран остаётся, снимки уходят в фейк.
    let finish = start_probe(&mut harness, CameraScreenPolicy::KeepRunningWithFakeCapture, false);
    assert_eq!(*harness.router.camera_screen().status(), CameraScreenStatus::Installed);
    assert_eq!(
        harness.router.camera_screen().capture_diagnostics().mode,
        CaptureMode::FakeForScenario {
            scenario: "screen-probe"
        }
    );
    volume_key(&mut harness, KeyPhase::Pressed, KeyRepeat::Initial, clock.now_ms);
    clock.frames(&harness, 4);
    volume_key(&mut harness, KeyPhase::Released, KeyRepeat::Initial, clock.now_ms);
    clock.frames(&harness, 2);

    assert!(
        render.plans().is_empty(),
        "настоящему рендеру план не ушёл: снимок не пойдёт в галерею"
    );
    assert!(still.store.drafts().is_empty(), "черновиков в хранилище нет");
    let during = harness.router.camera_screen().capture_diagnostics();
    assert_eq!(during.fake.expect("счётчики фейка").counters.submitted, 1);
    assert_eq!(
        harness
            .router
            .camera_screen()
            .report()
            .expect("отчёт")
            .effects
            .capture
            .accepted,
        1,
        "экран получил свой ответ от фейка и работает как обычно"
    );

    finish_probe(&mut harness, &finish);
    assert_eq!(
        harness.router.camera_screen().capture_diagnostics().mode,
        CaptureMode::Real,
        "после сценария снимки снова настоящие"
    );
    assert_eq!(harness.router.camera_screen().installs(), 1, "экран не пересоздавался");
    volume_key(&mut harness, KeyPhase::Pressed, KeyRepeat::Initial, clock.now_ms);
    clock.frames(&harness, 3);
    assert_eq!(render.plans().len(), 1, "теперь нажатие дошло до настоящего рендера");
    drop(still.service);
}

#[test]
fn the_real_service_comes_back_even_when_the_scenario_is_pushed_out_by_another_one() {
    let render = FakeRenderPort::new(RenderMode::Accepting);
    let still = start_still_runtime(&PhotoConfig::default(), &render);
    let mut harness = harness_for_rig(crate::camera_test_support::FakeCameraRig::with_config(
        CameraConfig::default(),
    ));
    harness.router.camera_screen = CameraScreenService::with_capture(instant_capture_config(), still.backend);
    harness.router.install_camera_screen();
    start_streaming(&mut harness);

    let _first = start_probe(&mut harness, CameraScreenPolicy::KeepRunningWithFakeCapture, false);
    assert!(matches!(
        harness.router.camera_screen().capture_diagnostics().mode,
        CaptureMode::FakeForScenario { .. }
    ));
    // Новый запуск вытесняет прежний: путь `finish` тот же, и настоящий сервис обязан вернуться.
    let second = Arc::new(Mutex::new(false));
    harness
        .starter
        .start(Box::new(ProbeScenario {
            policy: CameraScreenPolicy::KeepRunning,
            finish: Arc::clone(&second),
            installs_own_ui: false,
        }))
        .expect("старт");
    harness.router.step(Duration::from_millis(1));

    assert_eq!(
        harness.router.camera_screen().capture_diagnostics().mode,
        CaptureMode::Real
    );
    drop(still.service);
}
