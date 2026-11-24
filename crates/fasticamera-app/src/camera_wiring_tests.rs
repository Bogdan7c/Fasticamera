//! Запуск движка камеры: путь «конфиг → поток движка → каталог» и запасной путь при отказе.

use camera_core::AppPresence;

use super::*;
use crate::camera_test_support::{FakeCameraRig, WAIT};

#[test]
fn started_engine_reads_the_catalog_and_begins_in_background() {
    let rig = FakeCameraRig::new();
    let status = rig
        .handle()
        .wait_until_status(WAIT, |status| !status.catalog.cameras().is_empty())
        .expect("каталог прочитан");
    assert_eq!(status.presence, AppPresence::Background);
    assert_eq!(
        status.catalog.cameras().len(),
        4,
        "видимые камеры OnePlus 11: 0, 1, 2, 3"
    );
    assert!(
        rig.control
            .calls()
            .contains(&camera_core::fake::BackendCall::LoadCatalog),
        "каталог прочитан бэкендом"
    );
    assert!(
        !rig.control.is_device_open(),
        "без автозапуска видоискателя движок камеру сам не открывает"
    );
}

#[test]
fn backend_diagnostics_and_stream_services_reach_the_service() {
    let mut config = CameraConfig::default();
    config.raw_stream.max_pending_frames = 3;
    let rig = FakeCameraRig::with_config(config);
    rig.diagnostics.reader().record_frame_acquired();
    rig.stream_counters.update(|counters| counters.frames_offered = 12);
    let running = rig.services.running().expect("запущен");
    assert_eq!(running.diagnostics_snapshot().reader.frames_acquired, 1);
    let stream = rig.services.raw_stream();
    assert_eq!(stream.counters().frames_offered, 12, "счётчики потока видны сценариям");
    assert_eq!(
        stream.pool_plan_upper_bound().pending_pairs,
        3,
        "план пула считается из конфига потока"
    );
}

#[test]
fn invalid_raw_stream_config_gives_unavailable_service_naming_the_field() {
    let mut config = CameraConfig::default();
    config.raw_stream.pool_slack = 99;
    let control = camera_core::fake::FakeBackendControl::new();
    let services = start_camera(
        &config,
        CameraBackendParts {
            factory: control.factory(),
            diagnostics: Arc::new(camera_android::BackendDiagnostics::new(4)),
            raw_stream: RawStreamServices::detached(),
        },
    );
    let error = services.running().err().expect("камера недоступна");
    assert!(error.reason.contains("camera.raw_stream.pool_slack"), "{error}");
    assert!(control.calls().is_empty(), "бэкенд не создавался");
}

#[test]
fn invalid_config_gives_unavailable_service_with_the_reason_and_no_thread() {
    let mut config = CameraConfig::default();
    config.engine.recovery.max_consecutive_attempts = 0;
    let control = camera_core::fake::FakeBackendControl::new();
    let services = start_camera(
        &config,
        CameraBackendParts {
            factory: control.factory(),
            diagnostics: Arc::new(camera_android::BackendDiagnostics::new(4)),
            raw_stream: RawStreamServices::detached(),
        },
    );
    let error = services.running().err().expect("камера недоступна");
    assert!(error.reason.contains("неверные настройки камеры"), "{error}");
    assert!(error.reason.contains("max_consecutive_attempts"), "{error}");
    assert!(control.calls().is_empty(), "бэкенд не создавался и не вызывался");
}

/// Ящики белой, переданные корнем композиции, действительно соединены с движком: указание рендеру приходит в
/// тот же ящик, который держит читатель (поток рендера), а ответ источника, положенный рендером в общий ящик
/// ответов, доходит до владельца белого (толчок привязан) и закрывает заявку.
#[test]
fn white_boxes_of_the_composition_root_are_connected_to_the_engine() {
    use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
    use camera_contract::white::{
        SceneIlluminant, WhiteAnswer, WhiteAnswerChannel, WhiteConfidence, WhiteDirective, WhiteSourceOutcome,
    };
    use camera_core::white::{WhiteRequestOutcome, WhiteRequestState};

    let reader_slot = camera_contract::three_a::DirectiveSlot::<WhiteDirective>::new();
    let answers = WhiteAnswerChannel::new();
    let mut config = CameraConfig::default();
    config.viewfinder_autostart.enabled = false;
    let control = camera_core::fake::FakeBackendControl::new();
    let services = start_camera_with_links(
        &config,
        CameraBackendParts {
            factory: control.factory(),
            diagnostics: Arc::new(camera_android::BackendDiagnostics::new(4)),
            raw_stream: RawStreamServices::detached(),
        },
        CameraLinks {
            white_directives: reader_slot.clone(),
            white_answers: answers.clone(),
            ..CameraLinks::default()
        },
    );
    let wait_for = |wanted: &dyn Fn(&WhiteDirective) -> bool| {
        let deadline = std::time::Instant::now() + WAIT;
        loop {
            if let Some((_, directive)) = reader_slot.latest().filter(|(_, directive)| wanted(directive)) {
                return directive;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "указание не пришло: {:?}",
                reader_slot.latest()
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };

    // Указание есть сразу при старте: белая есть всегда (запасной свет из конфига).
    let initial = wait_for(&|_| true);
    assert_eq!(initial.white.temperature.kelvin(), 5_600.0);
    assert_eq!(initial.request, None, "заявки до возобновления нет");

    // Возобновление (WB22) заводит заявку источнику «телефон»: она видна читателю в том же ящике.
    let handle = services.running().expect("запущен").handle().clone();
    handle.enter_foreground().expect("команда принята очередью");
    let asked = wait_for(&|directive| directive.request.is_some())
        .request
        .expect("заявка");

    // Ответ рендера в общий ящик: движок получает толчок (привязка) и применяет белую 3200 К.
    let lamp = WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(3_200.0).expect("температура"),
        tint: Tint::NEUTRAL,
    };
    let (_, delivery) = answers.publish(WhiteAnswer {
        request: asked.id,
        camera: camera_contract::auto_exposure::CameraTag::of(&camera_contract::fixtures::camera_id("2")),
        measured_frame: None,
        outcome: WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(lamp),
            confidence: WhiteConfidence::FULL,
        },
    });
    assert_eq!(
        delivery,
        camera_contract::auto_exposure::SignalDelivery::Delivered,
        "толчок ответов привязан к движку"
    );
    wait_for(&|directive| directive.white == lamp);
    let status = handle
        .wait_until_status(WAIT, |status| {
            matches!(
                status.three_a.white.request,
                WhiteRequestState::Idle {
                    last: Some(camera_core::white::WhiteRequestResult {
                        result: WhiteRequestOutcome::Applied { .. },
                        ..
                    })
                }
            )
        })
        .expect("заявка закрыта исходом Applied");
    assert_eq!(status.three_a.white.held.setting, lamp);
}

/// Память белой, открытая корнем композиции, доезжает до движка: ручная белая записывается в настоящий файл, а
/// счётчики записи видны у сервиса камеры (отказ диска не только в журнале).
#[test]
fn white_memory_opened_by_the_composition_root_receives_the_manual_white_and_reports_its_stats() {
    use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
    use camera_contract::white::WhiteTransition;

    use crate::app_directory::AppDirectory;
    use crate::white_memory::{WhiteMemoryConfig, WhiteMemoryHandoff, open_white_memory};

    let path = std::env::temp_dir().join(format!("fasticamera-wiring-white-{}", std::process::id()));
    std::fs::create_dir_all(&path).expect("папка создана");
    let directory = AppDirectory::new(path.to_string_lossy().into_owned()).expect("папка годится");
    let mut config = CameraConfig::default();
    config.viewfinder_autostart.enabled = false;
    let control = camera_core::fake::FakeBackendControl::new();
    let services = start_camera_with_links(
        &config,
        CameraBackendParts {
            factory: control.factory(),
            diagnostics: Arc::new(camera_android::BackendDiagnostics::new(4)),
            raw_stream: RawStreamServices::detached(),
        },
        CameraLinks {
            white_memory: WhiteMemoryHandoff::holding(open_white_memory(&directory, &WhiteMemoryConfig::default())),
            ..CameraLinks::default()
        },
    );

    let manual = WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(3_900.0).expect("температура"),
        tint: Tint::NEUTRAL,
    };
    let handle = services.running().expect("запущен").handle().clone();
    handle
        .set_white(manual, WhiteTransition::Immediate)
        .expect("команда принята очередью");

    let deadline = std::time::Instant::now() + WAIT;
    while services.white_memory_stats().saved == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "запись не дошла до диска: {:?}",
            services.white_memory_stats()
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let text = std::fs::read_to_string(path.join("white.json")).expect("файл памяти записан");
    assert!(
        text.contains("\"origin\": \"manual\"") && text.contains("3900"),
        "{text}"
    );
    assert_eq!(services.white_memory_stats().failed, 0);
    drop(services);
    std::fs::remove_dir_all(path).expect("уборка");
}

// --- Композиция нашего AE (08a): ящик предложений, толчок и переключатель `exposure_controller` ---

mod auto_exposure_composition {
    use std::time::{Duration, Instant};

    use camera_contract::auto_exposure::{
        ExposureProduct, ExposureProposal, MeasurementQuality, MeteredArea, ProposalConvergence,
    };
    use camera_contract::capture_result::SensorTimestamp;
    use camera_contract::control::CaptureIntent;
    use camera_contract::ids::{CameraTarget, LensRole};
    use camera_contract::raw_format::RawFormat;
    use camera_contract::stream::{ReadoutAspect, StreamMode, TargetFrameRate};
    use camera_core::fake::FakeBackendControl;
    use camera_core::request_controls::ExposurePlan;

    use super::*;
    use crate::auto_exposure_link::{ExposureControllerKind, RenderAutoExposure};

    fn proposal(seconds_iso: f64) -> ExposureProposal {
        ExposureProposal {
            // Заведомо позже любого кадра потока fake.
            measured_frame: SensorTimestamp::from_nanos(i64::MAX / 2),
            measured_on: camera_contract::auto_exposure::CameraTag::of(&camera_contract::fixtures::camera_id("2")),
            target: ExposureProduct::from_seconds_iso(seconds_iso).expect("цель"),
            metered_area: MeteredArea::WholeFrame,
            convergence: ProposalConvergence::Settled,
            quality: MeasurementQuality::Measured,
        }
    }

    /// Камера на fake с заданным переключателем; видоискатель продукта не автозапускается.
    fn start(kind: ExposureControllerKind, links: CameraLinks) -> (CameraServices, Arc<FakeBackendControl>) {
        let mut config = CameraConfig {
            exposure_controller: kind,
            ..CameraConfig::default()
        };
        config.viewfinder_autostart.enabled = false;
        let control = FakeBackendControl::new();
        let services = start_camera_with_links(
            &config,
            CameraBackendParts {
                factory: control.factory(),
                diagnostics: Arc::new(camera_android::BackendDiagnostics::new(4)),
                raw_stream: RawStreamServices::detached(),
            },
            links,
        );
        (services, control)
    }

    /// Открыть, настроить, запустить поток и отдать три кадра результатов.
    fn stream(services: &CameraServices, control: &FakeBackendControl) {
        let handle = services.running().expect("запущен").handle().clone();
        handle.enter_foreground().expect("foreground");
        handle.open(CameraTarget::Role(LensRole::Wide)).expect("open");
        handle
            .configure(StreamMode {
                raw_format: RawFormat::Raw10,
                readout: ReadoutAspect::SixteenByNine,
                frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
            })
            .expect("configure");
        handle.start_streaming(CaptureIntent::Preview).expect("start");
        wait_until("поток пошёл", || control.is_repeating());
        control.complete_frames(3, Duration::from_millis(16));
        handle
            .wait_until_status(WAIT, |status| status.three_a.last_result.is_some())
            .expect("результаты дошли до движка");
    }

    fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !condition() {
            assert!(Instant::now() < deadline, "не дождались: {what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn sensor_product(control: &FakeBackendControl) -> Option<f64> {
        match control.repeating_request()?.1.exposure {
            ExposurePlan::Sensor { exposure_time, iso } => {
                Some(exposure_time.duration().as_secs_f64() * f64::from(iso.value()))
            }
            ExposurePlan::VendorAuto { .. } => None,
        }
    }

    /// Рендер создаётся раньше камеры: его ранний толчок идёт в пустоту (учтён счётчиком), а после запуска
    /// привязка работает — следующее предложение будит движок. Предложение, прочитанное движком до начала
    /// потока, при старте потока по контракту отбрасывается как «прошлый поток» (см. `ProposalGate`): рендер
    /// обязан публиковать первое предложение каждого нового потока.
    #[test]
    fn early_push_before_the_camera_started_is_counted_and_the_later_binding_works() {
        let links = CameraLinks::default();
        let render = links
            .auto_exposure
            .render_end(ExposureControllerKind::Own)
            .expect("сторона рендера");
        render.proposals.publish(proposal(0.4));
        assert_eq!(render.proposals.refresh_signal().missed_before_bind(), 1);
        assert!(!render.proposals.refresh_signal().is_bound(), "камеры ещё нет");

        let (services, control) = start(ExposureControllerKind::Own, links);
        assert!(
            render.proposals.refresh_signal().is_bound(),
            "после запуска движок привязан"
        );
        stream(&services, &control);
        render.proposals.publish(proposal(0.8));

        wait_until(
            "запрос собран под предложение после привязки",
            || sensor_product(&control).is_some_and(|product| (product - 0.8).abs() < 0.02),
        );
    }

    #[test]
    fn proposal_after_the_start_wakes_the_engine_and_rebuilds_the_request() {
        let links = CameraLinks::default();
        let render = links
            .auto_exposure
            .render_end(ExposureControllerKind::Own)
            .expect("сторона рендера");
        let (services, control) = start(ExposureControllerKind::Own, links);
        stream(&services, &control);
        let before = sensor_product(&control).expect("наш AE строит ручной запрос с первого плана");

        render.proposals.publish(proposal(1.6));

        wait_until("запрос пересобран по толчку", || {
            sensor_product(&control).is_some_and(|product| (product - 1.6).abs() < 0.05)
        });
        assert!(
            (before - 1.6).abs() > 0.5,
            "до предложения стояло стартовое значение: {before}"
        );
    }

    #[test]
    fn vendor_controller_keeps_vendor_ae_and_gives_the_renderer_nothing() {
        let links = CameraLinks::default();
        assert!(
            links
                .render_side(ExposureControllerKind::Vendor)
                .auto_exposure
                .is_none()
        );
        let reader = links
            .auto_exposure
            .render_end(ExposureControllerKind::Own)
            .expect("ящики есть всегда");
        let (services, control) = start(ExposureControllerKind::Vendor, links);
        stream(&services, &control);

        assert!(
            matches!(
                control.repeating_request().map(|(_, controls)| controls.exposure),
                Some(ExposurePlan::VendorAuto { .. })
            ),
            "вендорский AE: запрос с AE_MODE_ON"
        );
        let status = services.running().expect("запущен").handle().status();
        assert_eq!(
            status.three_a.auto_exposure, None,
            "вендорский AE состояния нашего AE не отдаёт"
        );
        assert!(
            !reader.proposals.refresh_signal().is_bound(),
            "с вендорским AE толчок рендера не привязывается"
        );
    }

    #[test]
    fn our_controller_with_unavailable_renderer_runs_vendor_ae_and_says_why() {
        let links = CameraLinks {
            render_auto_exposure: RenderAutoExposure::Unavailable {
                reason: "GPU не поднялся".to_owned(),
            },
            ..CameraLinks::default()
        };
        let signal = links
            .auto_exposure
            .render_end(ExposureControllerKind::Own)
            .expect("ящики");
        let (services, control) = start(ExposureControllerKind::Own, links);
        stream(&services, &control);

        assert!(
            matches!(
                control.repeating_request().map(|(_, controls)| controls.exposure),
                Some(ExposurePlan::VendorAuto { .. })
            ),
            "без рендера камера не должна застрять на стартовой экспозиции нашего AE"
        );
        let choice = services.exposure_controller();
        assert_eq!(choice.effective, ExposureControllerKind::Vendor);
        assert!(
            choice
                .fallback_reason
                .as_deref()
                .is_some_and(|reason| reason.contains("GPU не поднялся"))
        );
        assert!(!signal.proposals.refresh_signal().is_bound(), "толчок не привязан");
    }

    #[test]
    fn our_controller_never_sends_vendor_ae_to_the_backend() {
        let (services, control) = start(ExposureControllerKind::Own, CameraLinks::default());
        stream(&services, &control);

        let vendor = control
            .calls()
            .iter()
            .filter(|call| {
                matches!(call, camera_core::fake::BackendCall::SetRepeatingRequest { controls, .. }
                    if matches!(controls.exposure, ExposurePlan::VendorAuto { .. }))
            })
            .count();
        assert_eq!(vendor, 0);
        assert!(sensor_product(&control).is_some());
    }
}
