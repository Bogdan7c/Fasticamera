//! Функциональные тесты сценария `zoom` (сессия 13): настоящий маршрутизатор, настоящий движок камеры на fake-платформе
//! (он сам выбирает объектив по намерениям экрана и шлёт `LensSwitched`), НАСТОЯЩИЙ `CameraScreen` с исполнителем
//! эффектов и подставной «живой поток» (`StreamPump`). Подставных актёров двое, как в сценарии `screen`:
//! * **поток рендера** — рисует кадр UI установленным источником на ЧАСАХ MONOTONIC (одна шкала с кольцом кадров и
//!   метками действий сценария), а [`ContinuityMirror`] ведёт кольцо показанных кадров видоискателя: живые кадры
//!   текущей камеры, по смене камеры — несколько удержанных кадров, затем живые кадры новой камеры, с кадрированием
//!   по настоящей формуле каталога зума (зеркало фронтальной берётся оттуда же);
//! * **сценарий** — единственный, кто вкладывает действия и касания в экран.
//!
//! Тесты доходят до строки результата и проверяют то, что изменилось бы при поломке: времена и кадры шагов, вибрации
//! настоящего колеса, итоговое состояние экрана и камеры.

use camera_contract::ids::LensRole;
use camera_contract::viewfinder_crop::CropAspect;
use camera_contract::zoom::{ZoomFraming, ZoomRatio};
use render_loop::viewfinder::{BlackKind, FrameCropReport, FrameLabel, FrameRecord, TransitionKind};
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest};
use ui_draw::geometry::PxSize;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_service::CameraScreenService;
use crate::clock_offset::monotonic_now;

const SURFACE_WIDTH_PX: u32 = 1440;
const SURFACE_HEIGHT_PX: u32 = 3216;
/// 1 pt = ширина / 440 px (решение 82).
const PX_PER_PT: f32 = SURFACE_WIDTH_PX as f32 / 440.0;
/// Сколько записей держит подставное кольцо кадров (как `viewfinder.continuity.recent_frames`).
const RING_CAPACITY: usize = 240;
/// Сколько кадров рендер держит на смене камеры, пока новый кадр «готовится».
const HOLD_FRAMES: u32 = 4;
/// Период кадра подставного рендера, мс.
const FRAME_MS: u64 = 8;

/// Что подставное кольцо кадров делает не так (по умолчанию — здоровый рендер).
#[derive(Clone, Copy, Default)]
enum ContinuityFault {
    #[default]
    Healthy,
    /// После каждой смены камеры рендер показывает один чёрный кадр там, где ждали картинку.
    BlackFrameAfterEachSwitch,
}

/// Кольцо показанных кадров и счётчики непрерывности, которые на телефоне ведёт рендер.
struct ContinuityMirror {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl ContinuityMirror {
    fn start(harness: &Harness, fault: ContinuityFault) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let render = Arc::clone(&harness.render);
        let handle = harness.rig().handle();
        let join = thread::spawn(move || {
            let mut shown_camera: Option<camera_contract::ids::CameraId> = None;
            let mut hold_left = 0_u32;
            let mut black_owed = false;
            let mut ring: Vec<FrameRecord> = Vec::new();
            let mut counters = render_loop::viewfinder::ContinuityCounters::default();
            while !thread_stop.load(Ordering::SeqCst) {
                let status = handle.status();
                let active = status.lens.active.as_ref().map(|lens| lens.camera.clone());
                if active.is_some() && active != shown_camera {
                    if shown_camera.is_some() {
                        hold_left = HOLD_FRAMES;
                        counters.holds_started += 1;
                        black_owed = matches!(fault, ContinuityFault::BlackFrameAfterEachSwitch);
                    }
                    shown_camera = active.clone();
                }
                let Ok(now) = monotonic_now() else {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                };
                let record = if black_owed {
                    black_owed = false;
                    counters.black_frames_while_picture_expected += 1;
                    FrameRecord {
                        present_time: now,
                        label: FrameLabel::Black(BlackKind::PictureExpected),
                        backdrop: BackdropContinuity::NoCameraFrame,
                        crop: None,
                        held_magnification: 1.0,
                    }
                } else if hold_left > 0 || active.is_none() {
                    hold_left = hold_left.saturating_sub(1);
                    counters.held_frames_presented += 1;
                    FrameRecord {
                        present_time: now,
                        label: FrameLabel::Transition(TransitionKind::HeldLastFrame),
                        backdrop: BackdropContinuity::HeldLastFrame { since: now },
                        crop: None,
                        held_magnification: 1.0,
                    }
                } else {
                    counters.live_frames_presented += 1;
                    let camera = shown_camera.clone().expect("активная камера есть");
                    // Настоящая формула кадрирования каталога зума: из неё берётся и зеркало.
                    let framing = ZoomFraming {
                        aspect: CropAspect::SensorNative,
                        facing: status
                            .lens
                            .active
                            .as_ref()
                            .map_or(camera_contract::ids::LensFacing::Back, |lens| lens.facing),
                        ratio: ZoomRatio::ONE,
                        catalog: Arc::clone(&status.lens.zoom_catalog),
                    };
                    FrameRecord {
                        present_time: now,
                        label: FrameLabel::Live,
                        backdrop: BackdropContinuity::Live,
                        crop: Some(FrameCropReport {
                            resolved: camera_contract::geometry::PixelRect {
                                left: 0,
                                top: 0,
                                size: camera_contract::geometry::PixelSize::new(4096, 3072),
                            },
                            framed: framing.crop_for_camera(&camera),
                            camera,
                        }),
                        held_magnification: 1.0,
                    }
                };
                ring.push(record);
                if ring.len() > RING_CAPACITY {
                    ring.remove(0);
                }
                let snapshot = ring.clone();
                let counters_now = counters.clone();
                render.update(|status| {
                    let viewfinder = status.backend.viewfinder.get_or_insert_with(fresh_viewfinder);
                    viewfinder.continuity.recent_frames = snapshot;
                    viewfinder.continuity.counters = counters_now;
                    viewfinder.continuity.holding = hold_left > 0;
                });
                thread::sleep(Duration::from_millis(FRAME_MS));
            }
        });
        Self { stop, join: Some(join) }
    }
}

impl Drop for ContinuityMirror {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Кадр UI на часах MONOTONIC: та же шкала, что у кольца кадров и меток действий сценария.
fn ui_frame(harness: &Harness) {
    let Ok(now) = monotonic_now() else {
        return;
    };
    let request = UiFrameRequest {
        present_time: now,
        logical_size: PxSize::new(SURFACE_WIDTH_PX as f32, SURFACE_HEIGHT_PX as f32),
        px_per_pt: PX_PER_PT,
        backdrop: BackdropContinuity::Live,
    };
    if let Some(produced) = harness.control.produce_ui_frame(&request) {
        produced.expect("кадр экрана построен");
    }
}

/// Конфиг: короткие сроки, пороги из умолчаний (стенд в миллисекундах укладывается в них с запасом).
/// Гистерезис движка — по умолчанию: тап по кнопке — явный жест (`ZoomGesture::Discrete`), он выбирает объектив
/// без запаса, поэтому «1» с ширика возвращает основную камеру.
fn zoom_config(tune: impl FnOnce(&mut crate::zoom_scenario_config::ZoomScenarioConfig)) -> CameraConfig {
    let mut config = short_config(|_, _| {});
    config.scenarios.zoom.ready_timeout_ms = 5_000;
    config.scenarios.zoom.step_timeout_ms = 4_000;
    config.scenarios.zoom.step_settle_ms = 150;
    tune(&mut config.scenarios.zoom);
    config
}

fn instant_screen_config() -> CameraScreenConfig {
    CameraScreenConfig {
        fake_capture_delay_ms: 0,
        report_min_interval_ms: 0,
        ..CameraScreenConfig::default()
    }
}

/// Прогон `zoom` до итога.
fn run_zoom(config: CameraConfig, fault: ContinuityFault) -> (Harness, serde_json::Value) {
    let mut harness = harness_for_rig(FakeCameraRig::with_product_config(config));
    harness.router.camera_screen = CameraScreenService::new(instant_screen_config());
    harness.router.install_camera_screen();
    harness.deliver(ShellEvent::Resumed);
    // Размер поверхности нужен сценарию, чтобы перевести доли экрана в пиксели касаний.
    harness.surface_created(1);
    harness.surface_changed(1);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let _mirror = RenderMirror::start(&harness, MirrorFaults::default());
    let _continuity = ContinuityMirror::start(&harness, fault);
    // Продуктовый видоискатель идёт на основной камере: сценарий начинается как «пользователь открыл приложение».
    harness.rig().wait_streaming();
    harness.start(Scenario::parse("zoom").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(40);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        ui_frame(&harness);
    }
    let result = harness.output.single_result();
    (harness, result)
}

/// Элементы подробностей `detail_zoom` по порядку: сначала итоги, затем шаги.
fn detail_items(harness: &Harness) -> Vec<serde_json::Value> {
    stages_named(harness, "detail_zoom")
        .into_iter()
        .flat_map(|stage| stage["items"].as_array().cloned().unwrap_or_default())
        .collect()
}

#[test]
fn zoom_walks_every_lens_switch_flips_to_the_front_and_back_and_leaves_the_screen_on_one_x() {
    let (harness, result) = run_zoom(zoom_config(|_| {}), ContinuityFault::Healthy);

    assert_eq!(result["status"], "passed", "{result}");
    let items = detail_items(&harness);
    let steps = &items[1..];
    let names: Vec<&str> = steps
        .iter()
        .map(|step| step["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        names,
        vec![
            "01_tap_0.6",
            "02_tap_1",
            "03_tap_2",
            "04_tap_1",
            "05_tap_0.6",
            "06_flip_to_front",
            "07_flip_to_back",
            "08_tap_1_before_drag",
            "09_wheel_drag",
            "10_tap_1_final"
        ]
    );
    assert!(steps.iter().all(|step| step["failure"].is_null()), "{steps:?}");

    // Смены объективов: на .6 с основной, обратно, с ширика на 2× (возврат на основную) и обратно на .6, на фронталку и
    // назад. Для каждой записаны оба времени.
    let switching: Vec<&str> = steps
        .iter()
        .filter(|step| !step["switch"].is_null())
        .map(|step| step["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        switching,
        vec![
            "01_tap_0.6",
            "02_tap_1",
            "05_tap_0.6",
            "06_flip_to_front",
            "07_flip_to_back",
            "08_tap_1_before_drag"
        ],
        "тапы 2× и 1 на основной камере объектив не меняют; 03_tap_2 идёт с основной"
    );
    for step in steps.iter().filter(|step| !step["switch"].is_null()) {
        assert!(step["first_live_frame_ms"].is_f64(), "{step}");
        assert_eq!(step["black_frames_while_picture_expected"], 0, "{step}");
    }
    let front = steps
        .iter()
        .find(|step| step["name"] == "06_flip_to_front")
        .expect("шаг переворота");
    assert_eq!(front["mirror"], "correct", "кадры фронтальной зеркальны");
    assert_eq!(front["switch"]["to"], "front");

    // Протяжка по колесу: настоящие касания дали вибрации с промежутком не меньше 30 мс и увели зум.
    let drag = steps
        .iter()
        .find(|step| step["name"] == "09_wheel_drag")
        .expect("шаг протяжки");
    assert!(drag["haptics"]["count"].as_u64().unwrap_or(0) >= 2, "{drag}");
    assert!(drag["haptics"]["min_gap_ms"].as_f64().unwrap_or(0.0) >= 30.0, "{drag}");
    assert!(drag["screen_ratio"].as_f64().unwrap_or(0.0) > 1.5, "{drag}");

    // Итог: экран и камера оставлены на 1× основной камеры; чёрных кадров ноль; экран не переустанавливался.
    let summary = &result["report"];
    assert_eq!(summary["black_frames_while_picture_expected"], 0);
    assert_eq!(summary["final"]["ratio_milli"], 1_000);
    assert_eq!(summary["final"]["role"], "wide");
    assert_eq!(
        harness.rig().handle().status().lens.active.map(|lens| lens.role),
        Some(LensRole::Wide)
    );
    assert_eq!(
        harness.router.camera_screen().installs(),
        1,
        "экран остаётся на месте (KeepRunning)"
    );
    assert!(harness.router.camera_screen().input_stats().overflowed_touches == 0);
    assert_fits_logcat(&harness, &result);
}

#[test]
fn a_black_frame_after_a_switch_fails_the_run_naming_the_count() {
    let (_harness, result) = run_zoom(zoom_config(|_| {}), ContinuityFault::BlackFrameAfterEachSwitch);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("чёрных кадров") && error.contains("должно быть 0"),
        "{error}"
    );
}

#[test]
fn the_wheel_drag_point_of_the_config_lands_on_the_real_zoom_strip() {
    // Если раскладка экрана сдвинется, точка старта протяжки промахнётся мимо ленты, колесо не откроется, вибраций не
    // будет — тест покраснеет здесь, а не молча на телефоне (как точки `screen`).
    let (harness, result) = run_zoom(zoom_config(|_| {}), ContinuityFault::Healthy);
    assert_eq!(result["status"], "passed", "{result}");
    let items = detail_items(&harness);
    let drag = items
        .iter()
        .find(|item| item["name"] == "09_wheel_drag")
        .expect("шаг протяжки");
    assert!(
        drag["haptics"]["count"].as_u64().unwrap_or(0) >= 2,
        "палец не попал на ленту: {drag}"
    );
}

#[test]
fn a_steps_timeout_fails_the_run_with_the_step_name_and_what_was_awaited() {
    // Намерения экрана не доходят до движка: ящик не прочитан, смены не будет.
    let config = zoom_config(|zoom| zoom.step_timeout_ms = 400);
    let mut harness = harness_for_rig(FakeCameraRig::with_product_config(config));
    harness.router.camera_screen = CameraScreenService::new(instant_screen_config());
    harness.router.install_camera_screen();
    harness.deliver(ShellEvent::Resumed);
    // Размер поверхности нужен сценарию, чтобы перевести доли экрана в пиксели касаний.
    harness.surface_created(1);
    harness.surface_changed(1);
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), PumpFaults::default());
    let _mirror = RenderMirror::start(&harness, MirrorFaults::default());
    let _continuity = ContinuityMirror::start(&harness, ContinuityFault::Healthy);
    harness.rig().wait_streaming();
    // Отвязываем движок от ящика намерений: экран публикует в никуда.
    harness.camera.lens_intents().refresh_signal().unbind();
    harness.start(Scenario::parse("zoom").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(20);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        ui_frame(&harness);
    }
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("01_tap_0.6") && error.contains("не сообщил о смене объектива"),
        "{error}"
    );
}

#[test]
fn without_an_installed_screen_the_scenario_fails_at_once_with_a_reason() {
    let mut harness = harness_for_rig(FakeCameraRig::with_config(zoom_config(|_| {})));
    harness.deliver(ShellEvent::Resumed);
    // Размер поверхности нужен сценарию, чтобы перевести доли экрана в пиксели касаний.
    harness.surface_created(1);
    harness.surface_changed(1);
    harness.start(Scenario::parse("zoom").expect("сценарий зарегистрирован"));
    harness.run_until_result();

    let result = harness.output.single_result();
    assert_eq!(result["status"], "failed");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("экран камеры не установлен"),
        "{result}"
    );
}

#[test]
fn zoom_is_registered_with_a_screen_that_stays_on() {
    use crate::camera_screen_service::CameraScreenPolicy;
    use crate::scenario::camera::CameraScenario;
    assert!(CameraScenario::available().contains(&CameraScenario::Zoom));
    assert_eq!(CameraScenario::Zoom.name(), "zoom");
    assert_eq!(
        CameraScenario::Zoom.observing().camera_screen_policy(),
        CameraScreenPolicy::KeepRunning
    );
}
