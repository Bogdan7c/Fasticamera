//! Функциональные тесты сценария `capture` (сессия 14): настоящий маршрутизатор, настоящий движок камеры на
//! fake-платформе, НАСТОЯЩИЙ экран камеры с исполнителем эффектов, НАСТОЯЩАЯ служба записи снимков (`photo-output`) и
//! настоящий сервис снимка приложения. Подставных актёров три, и все — то, чего в процессе ПК нет:
//! * **поток рендера** — [`RenderMirror`] (счётчики показа) и [`LiveRing`] (кольцо показанных кадров активной камеры:
//!   без живого кадра сценарий не стартует);
//! * **«проявление» рендера** — [`StillResponder`]: по плану снимка шлёт `Secured` и «картинка нарисована», как это делает
//!   поток рендера, и считает принятые команды (`RenderStatus::still_commands`);
//! * **платформа** — хранилище (`FakeStore`), кодер HEIF (`FakeHeifFactory`), DNG и Exif-подделки.
//!
//! Каждый тест доходит до строки результата и проверяет то, что изменилось бы при поломке: адреса в отчёте совпадают с
//! файлами, опубликованными в хранилище, учёт серии сходится, провалы названы.

use std::collections::BTreeSet;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::AtomicUsize;

use camera_contract::geometry::{PixelRect, PixelSize};
use render_loop::viewfinder::{BlackKind, FrameCropReport, FrameLabel, FrameRecord};
use shell_contract::still_media::{
    EncodeError, HeifEncodeSettings, HeifEncoderService, HeifSurfaceEncoder, PendingMedia,
};
use shell_contract::surface::{PlatformWindow, SurfaceGeneration};
use still_contract::develop::{DevelopEvent, DevelopFailure};
use still_contract::plan::RawOutput;
use still_contract::shot::{OwnedPixels, RawSamples16};
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest};
use ui_draw::geometry::PxSize;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_service::CameraScreenService;
use crate::capture_backend::{CaptureBackend, CaptureMode};
use crate::photo_config::PhotoConfig;
use crate::still_runtime::{StillRuntime, StillRuntimeDeps};
use crate::still_test_support::{
    AnyCameraCatalog, CountingSound, FakeFormats, FakeJpeg, FakeRenderPort, FakeStore, FixedSweep, RenderMode,
    facts_for,
};
use android_shell::still_orphan_sweep::OrphanSweepReport;

const SURFACE_WIDTH_PX: u32 = 1440;
const SURFACE_HEIGHT_PX: u32 = 3216;
const PX_PER_PT: f32 = SURFACE_WIDTH_PX as f32 / 440.0;
const RING_CAPACITY: usize = 240;
const FRAME_MS: u64 = 8;

// ---- Платформа: часы и кодер HEIF ----------------------------------------------------------------------------

/// Настенные часы, каждый вызов которых на секунду позже: у настоящего MediaStore каждый файл получает свой адрес,
/// а подставное хранилище строит адрес из имени файла (оно зависит от времени съёмки) — одинаковые часы склеили бы
/// все снимки в один адрес.
#[derive(Default)]
struct TickingWallClock {
    calls: std::sync::atomic::AtomicI64,
}

impl photo_output::service::WallClock for TickingWallClock {
    fn now(&self) -> photo_output::exif::CaptureMoment {
        let call = self.calls.fetch_add(1, Ordering::SeqCst);
        photo_output::exif::CaptureMoment {
            utc_millis: 1_700_000_000_000 + call * 1_000,
            utc_offset_minutes: 180,
        }
    }
}

// ---- Платформа: кодер HEIF -----------------------------------------------------------------------------------

struct DanglingWindow;

impl PlatformWindow for DanglingWindow {
    fn raw_window(&self) -> NonNull<c_void> {
        NonNull::dangling()
    }
}

/// Кодер-подделка: отдаёт входную поверхность и завершается успешно (настоящий запишет HEIC в черновик).
struct FakeHeifFactory;

struct FakeHeifEncoder {
    surface: Option<shell_contract::surface::NativeSurface>,
}

impl HeifEncoderService for FakeHeifFactory {
    fn open(
        &self,
        _target: &dyn PendingMedia,
        _settings: &HeifEncodeSettings,
    ) -> Result<Box<dyn HeifSurfaceEncoder>, EncodeError> {
        Ok(Box::new(FakeHeifEncoder {
            surface: Some(shell_contract::surface::NativeSurface::new(
                SurfaceGeneration::new(1),
                Box::new(DanglingWindow),
            )),
        }))
    }
}

impl HeifSurfaceEncoder for FakeHeifEncoder {
    fn take_input_surface(&mut self) -> Option<shell_contract::surface::NativeSurface> {
        self.surface.take()
    }

    fn finish(self: Box<Self>, _timeout: Duration) -> Result<(), EncodeError> {
        Ok(())
    }

    fn abort(self: Box<Self>) {}
}

// ---- Поток рендера: кольцо живых кадров ----------------------------------------------------------------------

/// Показывает ли подставной рендер живую картинку.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Picture {
    /// Живые кадры активной камеры.
    Live,
    /// Стартовый чёрный экран: камера идёт, кадры не показываются.
    NeverLive,
    /// Несколько живых кадров, затем снимок диагностики рендера замирает (поток рендера занят
    /// проявлением — так было на телефоне в 14c): последний `present_time` больше не двигается.
    FrozenAfterLive,
}

/// Сколько живых кадров публикует `Picture::FrozenAfterLive`, прежде чем замереть.
const FROZEN_AFTER_LIVE_FRAMES: u64 = 20;

struct LiveRing {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl LiveRing {
    fn start(harness: &Harness, picture: Picture) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let render = Arc::clone(&harness.render);
        let handle = harness.rig().handle();
        let join = thread::spawn(move || {
            let mut ring: Vec<FrameRecord> = Vec::new();
            let mut live_frames = 0_u64;
            while !thread_stop.load(Ordering::SeqCst) {
                let status = handle.status();
                let Ok(now) = monotonic_now() else {
                    thread::sleep(Duration::from_millis(1));
                    continue;
                };
                if picture == Picture::FrozenAfterLive && live_frames >= FROZEN_AFTER_LIVE_FRAMES {
                    // Диагностика замерла: снимок статуса больше не обновляется.
                    thread::sleep(Duration::from_millis(FRAME_MS));
                    continue;
                }
                let record = match (status.lens.active.as_ref(), picture) {
                    (Some(active), Picture::Live | Picture::FrozenAfterLive) => {
                        live_frames += 1;
                        let framing = camera_contract::zoom::ZoomFraming {
                            aspect: camera_contract::viewfinder_crop::CropAspect::SensorNative,
                            facing: active.facing,
                            ratio: camera_contract::zoom::ZoomRatio::ONE,
                            catalog: Arc::clone(&status.lens.zoom_catalog),
                        };
                        FrameRecord {
                            present_time: now,
                            label: FrameLabel::Live,
                            backdrop: BackdropContinuity::Live,
                            crop: Some(FrameCropReport {
                                resolved: PixelRect {
                                    left: 0,
                                    top: 0,
                                    size: PixelSize::new(4096, 3072),
                                },
                                framed: framing.crop_for_camera(&active.camera),
                                camera: active.camera.clone(),
                            }),
                            held_magnification: 1.0,
                        }
                    }
                    _ => FrameRecord {
                        present_time: now,
                        label: FrameLabel::Black(BlackKind::NoPicture),
                        backdrop: BackdropContinuity::NoCameraFrame,
                        crop: None,
                        held_magnification: 1.0,
                    },
                };
                ring.push(record);
                if ring.len() > RING_CAPACITY {
                    ring.remove(0);
                }
                let snapshot = ring.clone();
                render.update(|status| {
                    let viewfinder = status.backend.viewfinder.get_or_insert_with(fresh_viewfinder);
                    viewfinder.continuity.recent_frames = snapshot;
                    viewfinder.continuity.counters.live_frames_presented = live_frames;
                });
                thread::sleep(Duration::from_millis(FRAME_MS));
            }
        });
        Self { stop, join: Some(join) }
    }
}

impl Drop for LiveRing {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

// ---- «Проявление» рендера -------------------------------------------------------------------------------------

/// Как «рендер» отвечает на планы снимков.
#[derive(Clone, Default)]
struct RenderBehaviour {
    /// Задержка до «кадр выбран» после получения плана.
    secure_delay_ms: u64,
    /// Номера планов (с 1), которые срываются ошибкой GPU после захвата.
    failing_plans: BTreeSet<usize>,
    /// Какой кадр «выбирает рендер»: честный, с настоящими часами (см. [`ResponderChoice`]).
    choice: ResponderChoice,
}

/// Выбор кадра ответчика рендера. Метка сенсора строится от НАСТОЯЩИХ часов процесса и момента нажатия плана, а не
/// константой: тогда сценарий, переводящий её через измеренное смещение часов, получает ту задержку, что задана здесь.
#[derive(Clone, Copy)]
struct ResponderChoice {
    /// Сколько мс назад (до нажатия) началась экспозиция выбранного кадра.
    latency_ms: i64,
    /// `None` — ровно показанный кадр; `Some(причина)` — замена.
    substituted: Option<still_contract::frame_choice::SubstitutionReason>,
}

impl Default for ResponderChoice {
    fn default() -> Self {
        Self {
            latency_ms: 80,
            substituted: None,
        }
    }
}

impl ResponderChoice {
    /// Выбор для плана: метка сенсора = нажатие в шкале BOOTTIME − задержка.
    fn for_press(self, pressed_at: shell_contract::clock::MonotonicNanos) -> still_contract::frame_choice::FrameChoice {
        use crate::clock_offset::{ClockOffsetSource, SystemClocks};
        let offset = SystemClocks.sample_offset().expect("системные часы читаются");
        let sensor_timestamp = camera_contract::capture_result::SensorTimestamp::from_nanos(
            offset.monotonic_to_boottime(pressed_at).as_nanos() - self.latency_ms * 1_000_000,
        );
        match self.substituted {
            None => still_contract::frame_choice::FrameChoice::ShownAtPress {
                sensor_timestamp,
                shown_at: shell_contract::clock::MonotonicNanos::from_nanos(pressed_at.as_nanos() - 10_000_000),
            },
            Some(reason) => still_contract::frame_choice::FrameChoice::Substituted {
                sensor_timestamp,
                wanted: None,
                reason,
            },
        }
    }
}

struct StillResponder {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl StillResponder {
    fn start(harness: &Harness, port: Arc<FakeRenderPort>, behaviour: RenderBehaviour) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let render = Arc::clone(&harness.render);
        let answered = Arc::new(AtomicUsize::new(0));
        let join = thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                let plans = port.plans();
                let already = answered.load(Ordering::SeqCst);
                if let Some(plan) = plans.get(already) {
                    let number = already + 1;
                    answered.fetch_add(1, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(behaviour.secure_delay_ms));
                    // Рендер принял команду `Begin`: учёт, который читает сценарий.
                    render.update(|status| status.still_commands.begins_accepted += 1);
                    let mut facts = facts_for(plan.id);
                    facts.outputs = plan.outputs;
                    let choice = behaviour.choice.for_press(plan.press.at);
                    facts.sensor_timestamp = choice.sensor_timestamp();
                    facts.choice = choice;
                    let raw = (plan.outputs.raw == RawOutput::Dng).then(|| {
                        RawSamples16::new(PixelSize::new(16, 12), Arc::new(OwnedPixels(vec![7; 16 * 12 * 2])))
                            .expect("размер совпал")
                    });
                    port.emit(plan.id, DevelopEvent::Secured { facts, raw });
                    if behaviour.failing_plans.contains(&number) {
                        port.emit(
                            plan.id,
                            DevelopEvent::Failed {
                                id: plan.id,
                                failure: DevelopFailure::Gpu("тест: проявление упало".to_owned()),
                            },
                        );
                    } else {
                        port.emit(plan.id, DevelopEvent::PrimaryRendered { id: plan.id });
                    }
                } else {
                    thread::sleep(Duration::from_millis(1));
                }
            }
        });
        Self { stop, join: Some(join) }
    }
}

impl Drop for StillResponder {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

// ---- Стенд ---------------------------------------------------------------------------------------------------

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

fn screen_config() -> CameraScreenConfig {
    CameraScreenConfig {
        fake_capture_delay_ms: 0,
        report_min_interval_ms: 0,
        ..CameraScreenConfig::default()
    }
}

/// Конфиг сценария: короткие сроки в реальном времени стенда.
fn capture_config(tune: impl FnOnce(&mut crate::capture_scenario_config::CaptureScenarioConfig)) -> CameraConfig {
    let mut config = short_config(|_, _| {});
    config.scenarios.capture.ready_timeout_ms = 3_000;
    config.scenarios.capture.viewfinder_settle_ms = 50;
    config.scenarios.capture.secured_timeout_ms = 2_000;
    config.scenarios.capture.saved_timeout_ms = 6_000;
    config.scenarios.capture.raw_toggle_timeout_ms = 1_000;
    config.scenarios.capture.pause_between_steps_ms = 100;
    config.scenarios.capture.burst.drain_timeout_ms = 6_000;
    tune(&mut config.scenarios.capture);
    config
}

/// Всё, что нужно прогону от снимка: набор для экрана, «рендер» снимков, хранилище и служба (её `Drop` останавливает
/// рабочий поток, поэтому её держит стенд).
struct Stand {
    backend: CaptureBackend,
    store: Arc<FakeStore>,
    port: Arc<FakeRenderPort>,
    _service: Option<photo_output::service::StillService>,
}

/// Настоящая служба снимков вокруг подделок платформы.
fn real_stand() -> Stand {
    let port = FakeRenderPort::new(RenderMode::Accepting);
    let store = Arc::new(FakeStore::default());
    let runtime = StillRuntime::start(
        &PhotoConfig::default(),
        StillRuntimeDeps {
            store: store.clone(),
            heif: Arc::new(FakeHeifFactory),
            jpeg: Arc::new(FakeJpeg),
            formats: Arc::new(FakeFormats),
            clock: Arc::new(TickingWallClock::default()),
            cameras: Arc::new(AnyCameraCatalog),
            render: port.clone(),
            shutter_sound: Arc::new(CountingSound::default()),
            orphan_sweep: Arc::new(FixedSweep(OrphanSweepReport::Removed { count: 0 })),
        },
    )
    .expect("служба запускается");
    let (service, backend) = runtime.into_parts();
    Stand {
        backend,
        store,
        port,
        _service: Some(service),
    }
}

/// Стенд с заданным набором (фейк или запасной фейк): службы нет.
fn stand_with(backend: CaptureBackend) -> Stand {
    Stand {
        backend,
        store: Arc::new(FakeStore::default()),
        port: FakeRenderPort::new(RenderMode::Accepting),
        _service: None,
    }
}

struct Run {
    harness: Harness,
    result: serde_json::Value,
    store: Arc<FakeStore>,
    _service: Option<photo_output::service::StillService>,
}

/// Прогон `capture` до итога.
fn run_capture(config: CameraConfig, stand: Stand, render: RenderBehaviour, picture: Picture) -> Run {
    let Stand {
        backend,
        store,
        port,
        _service,
    } = stand;
    let mut harness = harness_for_rig(FakeCameraRig::with_product_config(config));
    harness.router.camera_screen = CameraScreenService::with_capture(screen_config(), backend);
    harness.router.install_camera_screen();
    harness.deliver(ShellEvent::Resumed);
    harness.surface_created(1);
    harness.surface_changed(1);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let _mirror = RenderMirror::start(&harness, MirrorFaults::default());
    let _ring = LiveRing::start(&harness, picture);
    let _responder = StillResponder::start(&harness, port, render);
    harness.rig().wait_streaming();
    harness.start(Scenario::parse("capture").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(40);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        ui_frame(&harness);
    }
    let result = harness.output.single_result();
    // Ещё несколько кадров экрана: действия, вложенные при завершении (возврат RAW), разбираются на кадрах.
    for _ in 0..30 {
        harness.router.step(Duration::from_millis(2));
        ui_frame(&harness);
        thread::sleep(Duration::from_millis(2));
    }
    Run {
        harness,
        result,
        store,
        _service,
    }
}

fn run_healthy() -> Run {
    run_capture(
        capture_config(|_| {}),
        real_stand(),
        RenderBehaviour::default(),
        Picture::Live,
    )
}

/// Адреса файлов, опубликованных хранилищем, по порядку записи.
fn published_uris(store: &FakeStore) -> Vec<String> {
    store
        .drafts()
        .into_iter()
        .filter(|draft| draft.state == crate::still_test_support::DraftState::Published)
        .map(|draft| format!("content://fake/{}", draft.name))
        .collect()
}

fn uris_of(result: &serde_json::Value) -> Vec<String> {
    result["report"]["uris"]
        .as_array()
        .map(|uris| uris.iter().filter_map(|uri| uri.as_str().map(str::to_owned)).collect())
        .unwrap_or_default()
}

// ---- Тесты ---------------------------------------------------------------------------------------------------

#[test]
fn capture_takes_a_heif_then_heif_with_dng_then_a_burst_and_the_report_lists_exactly_the_published_files() {
    let run = run_healthy();

    assert_eq!(run.result["status"], "passed", "{}", run.result);
    let report = &run.result["report"];
    // Адреса отчёта — ровно файлы, опубликованные хранилищем: 1 + (HEIC + DNG) + 10.
    let reported: BTreeSet<String> = uris_of(&run.result).into_iter().collect();
    let published: BTreeSet<String> = published_uris(&run.store).into_iter().collect();
    assert_eq!(reported.len(), 13, "{report}");
    assert_eq!(reported, published);
    assert_eq!(report["shots_saved"], 12);
    assert_eq!(report["mode"]["mode"], "real");
    assert_eq!(report["mode_end"]["mode"], "real");
    // Серия: все 10 нажатий прошли, ни одного отказа, видоискатель не просел.
    assert_eq!(report["burst"]["presses"], 10);
    assert_eq!(report["burst"]["saved"], 10);
    assert_eq!(report["burst"]["refused_by_screen"], 0);
    assert_eq!(
        report["render_still"]["begins_accepted"], 12,
        "рендер принял 12 команд снимка"
    );
    // Второй снимок — с DNG, остальные без.
    let shots = report["shots"].as_array().expect("снимки");
    assert_eq!(shots[0]["outcome"], "saved");
    assert_eq!(shots[1]["outcome"], "saved_with_raw");
    assert!(shots[2..].iter().all(|shot| shot["outcome"] == "saved"));
    // Файлы настоящей службы: у DNG-снимка заголовок TIFF из подделки формата.
    let drafts = run.store.drafts();
    assert!(
        drafts.iter().any(|draft| draft.bytes.starts_with(b"II*\0FAKE-DNG")),
        "DNG записан"
    );
    // Сценарий оставил RAW выключенным, а экран — на месте.
    let screen = run.harness.router.camera_screen();
    assert_eq!(
        screen.report().expect("отчёт").capture_settings.raw,
        camera_screen::settings::RawCapture::Off
    );
    assert_eq!(screen.installs(), 1);
    assert_eq!(screen.capture_diagnostics().mode, CaptureMode::Real);
}

#[test]
fn the_detail_lines_hold_every_shot_with_its_times_and_the_reserved_render_place() {
    let run = run_healthy();

    let items: Vec<serde_json::Value> = stages_named(&run.harness, "detail_capture")
        .into_iter()
        .flat_map(|stage| stage["items"].as_array().cloned().unwrap_or_default())
        .collect();
    // Выбор кадра дошёл от рендера до отчёта: ровно показанный кадр, задержка ≈ заданных 80 мс (допуск на смещение часов).
    assert_eq!(items[0]["frame_summary"]["exact"], 12);
    let max_delay = items[0]["frame_summary"]["max_press_to_frame_ms"]
        .as_f64()
        .expect("задержка измерена");
    assert!((79.0..=81.0).contains(&max_delay), "{}", items[0]);
    assert!(
        items[0]["render_still_detail"].is_null(),
        "рендер ПК диагностику снимков не публикует"
    );
    let shots: Vec<&serde_json::Value> = items.iter().filter(|item| item["request"].is_u64()).collect();
    assert_eq!(shots.len(), 12);
    for shot in shots {
        assert!(
            shot["secured_after_ms"].is_u64() && shot["saved_after_ms"].is_u64(),
            "{shot}"
        );
        assert_eq!(shot["frame_choice"]["kind"], "exact", "{shot}");
        let delay = shot["press_to_frame_ms"].as_f64().expect("задержка снимка");
        assert!((79.0..=81.0).contains(&delay), "{shot}");
        assert_eq!(shot["shown_before_press_ms"], 10.0);
    }
}

#[test]
fn a_fallback_fake_instead_of_the_real_service_fails_the_run_at_once_with_the_reason() {
    let backend = CaptureBackend::fake_fallback(&screen_config(), "камера недоступна: нет паспорта телефона");

    let run = run_capture(
        capture_config(|_| {}),
        stand_with(backend),
        RenderBehaviour::default(),
        Picture::Live,
    );

    assert_eq!(run.result["status"], "failed", "{}", run.result);
    let error = run.result["error"].as_str().expect("причина");
    assert!(
        error.contains("запасной фейк") && error.contains("нет паспорта телефона"),
        "{error}"
    );
    assert_eq!(
        run.harness.router.camera_screen().capture_requests_accepted(),
        0,
        "затвор не нажимали"
    );
}

#[test]
fn a_plain_fake_is_refused_too() {
    let run = run_capture(
        capture_config(|_| {}),
        stand_with(CaptureBackend::fake(&screen_config())),
        RenderBehaviour::default(),
        Picture::Live,
    );

    assert_eq!(run.result["status"], "failed");
    assert!(
        run.result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("не настоящий"),
        "{}",
        run.result
    );
}

#[test]
fn burst_presses_carry_their_own_moment_even_when_the_render_snapshot_is_frozen() {
    // 14c, телефон: метка нажатия бралась из последнего показанного кадра рендера; пока рендер занят
    // проявлением, снимок стоит, и нажатия через 100 мс получали одну и ту же старую метку — рендер искал
    // кадр за 100–300 мс до нажатия и не находил его в кольце. Метка обязана быть моментом самого нажатия.
    let run = run_capture(
        capture_config(|_| {}),
        real_stand(),
        RenderBehaviour::default(),
        Picture::FrozenAfterLive,
    );

    let journal = run.harness.router.camera_screen().capture_journal();
    let mut presses: Vec<i64> = journal.iter().map(|request| request.pressed_at.as_nanos()).collect();
    assert!(
        presses.len() >= 5,
        "сценарий дошёл до серии: {} нажатий; {}",
        presses.len(),
        run.result
    );
    presses.sort_unstable();
    let gaps_ms: Vec<i64> = presses.windows(2).map(|pair| (pair[1] - pair[0]) / 1_000_000).collect();
    // Нажатия сценария разнесены минимум на интервал серии (100 мс) минус дрожь тика.
    assert!(
        gaps_ms.iter().all(|gap| *gap >= 90),
        "метки нажатий повторяются или слиплись: {gaps_ms:?}"
    );
}

#[test]
fn without_a_live_frame_of_the_active_camera_the_shutter_is_never_pressed() {
    let run = run_capture(
        capture_config(|config| config.ready_timeout_ms = 600),
        real_stand(),
        RenderBehaviour::default(),
        Picture::NeverLive,
    );

    assert_eq!(run.result["status"], "failed", "{}", run.result);
    let error = run.result["error"].as_str().unwrap_or_default();
    assert!(error.contains("не готово") && error.contains("живого кадра"), "{error}");
    assert_eq!(run.harness.router.camera_screen().capture_requests_accepted(), 0);
    assert!(run.store.drafts().is_empty());
}

#[test]
fn a_failed_first_shot_fails_the_run_naming_the_shot_and_leaves_no_addresses() {
    let render = RenderBehaviour {
        failing_plans: [1].into_iter().collect(),
        ..RenderBehaviour::default()
    };

    let run = run_capture(capture_config(|_| {}), real_stand(), render, Picture::Live);

    assert_eq!(run.result["status"], "failed", "{}", run.result);
    let error = run.result["error"].as_str().unwrap_or_default();
    assert!(error.contains("снимок 1") && error.contains("сорвался"), "{error}");
    assert!(uris_of(&run.result).is_empty(), "сорванный снимок без файла");
    assert!(published_uris(&run.store).is_empty());
}

#[test]
fn a_failure_with_raw_switched_on_hands_the_setting_back_off() {
    // Второй снимок («HEIF + DNG») срывается: RAW к этому моменту включён значком и обязан вернуться.
    let render = RenderBehaviour {
        failing_plans: [2].into_iter().collect(),
        ..RenderBehaviour::default()
    };

    let run = run_capture(capture_config(|_| {}), real_stand(), render, Picture::Live);

    assert_eq!(run.result["status"], "failed", "{}", run.result);
    let screen = run.harness.router.camera_screen();
    assert_eq!(
        screen.report().expect("отчёт").capture_settings.raw,
        camera_screen::settings::RawCapture::Off,
        "владелец не остаётся с включённым RAW"
    );
    // Файл первого снимка всё же появился, и его адрес в строке результата — иначе скрипт его не удалит.
    assert_eq!(uris_of(&run.result).len(), 1, "{}", run.result);
}

#[test]
fn a_render_that_substitutes_the_frame_fails_the_run_naming_the_share_and_the_evicted_ring() {
    let render = RenderBehaviour {
        choice: ResponderChoice {
            latency_ms: 80,
            substituted: Some(still_contract::frame_choice::SubstitutionReason::ShownFrameEvicted),
        },
        ..RenderBehaviour::default()
    };

    let run = run_capture(capture_config(|_| {}), real_stand(), render, Picture::Live);

    assert_eq!(run.result["status"], "failed", "{}", run.result);
    let error = run.result["error"].as_str().expect("текст ошибки");
    assert!(error.contains("точно показанный кадр взят у 0 из 12"), "{error}");
    assert!(error.contains("вытеснен из кольца у 12"), "{error}");
    assert_eq!(run.result["report"]["frames"]["evicted"], 12);
}

#[test]
fn a_render_that_picks_a_frame_far_in_the_past_fails_the_run_with_the_measured_delay() {
    let render = RenderBehaviour {
        choice: ResponderChoice {
            latency_ms: 400,
            substituted: None,
        },
        ..RenderBehaviour::default()
    };

    let run = run_capture(capture_config(|_| {}), real_stand(), render, Picture::Live);

    assert_eq!(run.result["status"], "failed", "{}", run.result);
    let error = run.result["error"].as_str().expect("текст ошибки");
    assert!(
        error.contains("задержка «нажатие → начало экспозиции кадра» до 40"),
        "{error}"
    );
    let max_delay = run.result["report"]["frames"]["p2f_max_ms"].as_f64().expect("задержка");
    assert!((399.0..=401.0).contains(&max_delay), "{max_delay}");
}

#[test]
fn a_slow_capture_makes_the_screen_refuse_extra_burst_presses_and_the_accounting_still_closes() {
    // «Кадр захвачен» приходит через 250 мс, нажатия идут каждые 100 мс: затвор занят, экран отказывает сам.
    let render = RenderBehaviour {
        secure_delay_ms: 250,
        ..RenderBehaviour::default()
    };

    // Отказов экрана тут десять минус принятые: порог (по умолчанию 3) поднят, чтобы судить именно учёт.
    let config = capture_config(|capture| capture.burst.max_refused_presses = 9);
    let run = run_capture(config, real_stand(), render, Picture::Live);

    assert_eq!(run.result["status"], "passed", "{}", run.result);
    let burst = &run.result["report"]["burst"];
    let presses = burst["presses"].as_u64().expect("нажатия");
    let refused = burst["refused_by_screen"].as_u64().expect("отказы экрана");
    assert_eq!(presses, 10);
    assert!(refused > 0, "отказы экрана видны, а не растворились: {burst}");
    assert_eq!(presses, burst["submitted"].as_u64().expect("попытки") + refused);
    assert_eq!(burst["saved"], burst["accepted"], "всё принятое сохранено");
    assert_eq!(
        uris_of(&run.result).len(),
        1 + 2 + burst["saved"].as_u64().expect("сохранено") as usize,
        "адреса — только настоящие файлы"
    );
}

#[test]
fn capture_is_registered_with_a_screen_that_stays_on_and_with_real_shots() {
    use crate::camera_screen_service::CameraScreenPolicy;
    use crate::scenario::camera::CameraScenario;
    assert!(CameraScenario::available().contains(&CameraScenario::Capture));
    assert_eq!(CameraScenario::Capture.name(), "capture");
    let policy = CameraScenario::Capture.observing().camera_screen_policy();
    assert_eq!(policy, CameraScreenPolicy::KeepRunning);
    assert!(!policy.wants_fake_capture(), "снимки у `capture` настоящие");
}
