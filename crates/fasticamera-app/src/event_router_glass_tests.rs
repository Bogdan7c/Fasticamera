//! Функциональные тесты сценария `glass` (сессия 11): настоящий маршрутизатор, настоящий движок камеры на
//! fake-платформе, подставной живой поток (`StreamPump`) и подставной рендер ([`RenderMirror`]), который по
//! записанным командам UI ведёт диагностику слоя UI. Каждый тест доходит до строки результата.
//!
//! Что тесты доказывают: сценарий дожидается установившегося видоискателя, ставит демо-сцену источником кадра
//! UI, выдаёт стадию `glass_live`, по концу окна снимает источник ДО результата, а вердикт строится по
//! диагностике рендера (медленный GPU UI и отказы кадров — провал с понятной причиной).

use super::*;
use crate::gpu_test_support::RecordedUiOverlay;
use shell_contract::clock::BoottimeNanos;
use shell_contract::gravity::{GravitySample, GravityVector};

/// Окно наблюдения в тестах, мс.
const WINDOW_MS: u64 = 300;

/// Кадров UI прогрева в тестах (подставной рендер рисует кадр UI на каждый показанный кадр камеры).
const WARMUP_FRAMES: u64 = 8;

fn glass_config() -> CameraConfig {
    let mut config = short_config(|_, _| {});
    config.scenarios.glass.duration_ms = WINDOW_MS;
    config.scenarios.glass.warmup_ui_frames = WARMUP_FRAMES;
    config.scenarios.glass.warmup_timeout_ms = 200;
    config
}

/// Поток, который кладёт показания гравитации в ящик оболочки, пока жив.
struct GravityPump {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl GravityPump {
    fn start(harness: &Harness) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let slot = Arc::clone(&harness.shell.gravity);
        let join = thread::spawn(move || {
            let mut time_ns = 1_i64;
            while !thread_stop.load(Ordering::SeqCst) {
                slot.publish(GravitySample {
                    time: BoottimeNanos::from_nanos(time_ns),
                    vector: GravityVector {
                        x: 0.0,
                        y: 9.81,
                        z: 0.0,
                    },
                });
                time_ns += 5_000_000;
                thread::sleep(Duration::from_millis(5));
            }
        });
        Self { stop, join: Some(join) }
    }
}

impl Drop for GravityPump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Прогон `glass` до итога; `gravity_flows` — идут ли показания гравитации.
fn run_glass(faults: MirrorFaults, gravity_flows: bool) -> (Harness, serde_json::Value) {
    let mut harness = harness_for_rig(FakeCameraRig::with_config(glass_config()));
    harness.deliver(ShellEvent::Resumed);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let _mirror = RenderMirror::start(&harness, faults);
    let _gravity = gravity_flows.then(|| GravityPump::start(&harness));
    harness.start(Scenario::parse("glass").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
    }
    let result = harness.output.single_result();
    (harness, result)
}

#[test]
fn glass_installs_the_showcase_shows_it_for_the_window_and_removes_it_before_the_result() {
    let (harness, result) = run_glass(MirrorFaults::default(), true);

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["overlay"]["active_source"], "glass_showcase");
    assert!(report["overlay"]["frames_drawn"].as_u64().expect("кадры UI") > 0);
    assert_eq!(report["overlay"]["frames_failed"], 0);
    assert!(report["overlay"]["pyramid_rebuilds"].as_u64().expect("пирамида") > 0);
    assert_eq!(report["overlay"]["gpu_time"]["p95_ms"], 1.8);
    assert_eq!(
        report["ui_gpu_budget"]["budget_ms"], 8.33,
        "60 Гц × 0,5 из конфига (решение 90)"
    );
    assert_eq!(report["late_frames"], 0);
    assert_eq!(report["source_removal"]["state"], "delivered");
    assert_eq!(report["gravity"]["state"], "active");
    assert_eq!(report["gravity"]["source"], "injected");
    assert!(report["gravity"]["samples_in_window"].as_u64().expect("показания") > 0);
    assert_eq!(report["notes"], serde_json::json!([]));
    assert_eq!(report["viewfinder"]["cpu_copies"], 0);
    // Источник поставлен один раз и снят сценарием; конец сценария второй раз его не снимает.
    assert_eq!(
        harness.control.recorded_ui_overlay(),
        vec![RecordedUiOverlay::Install("glass_showcase"), RecordedUiOverlay::Remove]
    );
    // Скрипт снимает скриншоты на glass_live; подробности — до результата.
    let names = harness.output.stage_names();
    assert_eq!(
        names.iter().filter(|name| *name == "glass_live").count(),
        1,
        "{names:?}"
    );
    let position = |name: &str| names.iter().position(|stage| stage == name).expect(name);
    assert!(position("streaming") < position("glass_warmup"), "{names:?}");
    assert!(position("glass_warmup") < position("glass_live"), "{names:?}");
    assert!(position("glass_live") < position("detail_glass"), "{names:?}");
    let live = &stages_named(&harness, "glass_live")[0];
    assert_eq!(live["duration_ms"], WINDOW_MS);
    // glass_live выдан только после прогрева: UI нарисовал не меньше warmup_ui_frames кадров.
    assert!(
        live["warmup_frames"].as_u64().expect("кадры прогрева") >= WARMUP_FRAMES,
        "{live}"
    );
    assert_eq!(report["warmup"]["frames"], live["warmup_frames"]);
    // Кадры прогрева в окно не входят: рисовано в окне меньше, чем всего с установки.
    assert!(report["overlay"]["frames_drawn"].as_u64().expect("кадры UI") > 0);
    assert_fits_logcat(&harness, &result);
}

#[test]
fn a_silent_gravity_sensor_is_a_note_and_not_a_failure() {
    let (_harness, result) = run_glass(MirrorFaults::default(), false);

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["gravity"]["state"], "active");
    assert_eq!(report["gravity"]["samples_in_window"], 0);
    let notes = report["notes"].as_array().expect("заметки");
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(
        notes[0].as_str().unwrap_or_default().contains("ни одного показания"),
        "{notes:?}"
    );
}

#[test]
fn glass_fails_when_the_ui_gpu_time_is_over_the_budget_and_still_removes_the_source() {
    let faults = MirrorFaults {
        ui_overlay: UiOverlayFault::SlowGpu,
        ..MirrorFaults::default()
    };
    let (harness, result) = run_glass(faults, true);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("GPU-время кадра UI") && error.contains("8.33"),
        "{error}"
    );
    assert_eq!(result["report"]["overlay"]["gpu_time"]["p95_ms"], 9.3);
    assert_eq!(
        harness.control.recorded_ui_overlay().last(),
        Some(&RecordedUiOverlay::Remove),
        "источник снят и при провале"
    );
    assert_fits_logcat(&harness, &result);
}

#[test]
fn glass_fails_in_warmup_with_the_render_error_text_when_ui_frames_never_succeed() {
    let faults = MirrorFaults {
        ui_overlay: UiOverlayFault::DrawFails,
        ..MirrorFaults::default()
    };
    let (harness, result) = run_glass(faults, true);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    // Ни один кадр UI не удался: это провал прогрева, причина слоя UI приложена.
    assert!(
        error.contains("прогрева") && error.contains("нарисовано 0") && error.contains("превышен предел кадра"),
        "{error}"
    );
    assert_fits_logcat(&harness, &result);
}

#[test]
fn glass_without_a_live_viewfinder_fails_with_the_render_reason_and_never_installs_the_source() {
    let mut config = glass_config();
    // Живого потока нет (нет `StreamPump`), рендер камеру не показывает: ждём недолго.
    config.scenarios.viewfinder.render_live_timeout_ms = 200;
    let mut harness = harness_for_rig(FakeCameraRig::with_config(config));
    harness.deliver(ShellEvent::Resumed);
    harness.start(Scenario::parse("glass").expect("сценарий зарегистрирован"));
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
    }
    let result = harness.output.single_result();

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(error.contains("рендер не показал ни одного кадра камеры"), "{error}");
    assert_eq!(harness.control.recorded_ui_overlay(), Vec::<RecordedUiOverlay>::new());
}

#[test]
fn warmup_frames_stay_out_of_the_measured_window_even_when_they_fail() {
    let faults = MirrorFaults {
        ui_overlay: UiOverlayFault::FailsWhileWarmingUp,
        ..MirrorFaults::default()
    };
    let (harness, result) = run_glass(faults, true);

    // Первые кадры UI отказали, но они в прогреве: вердикт по окну чистый.
    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["overlay"]["frames_failed"], 0, "отказы прогрева не в окне");
    assert!(report["warmup"]["frames"].as_u64().expect("прогрев") >= WARMUP_FRAMES);
    let live = &stages_named(&harness, "glass_live")[0];
    assert!(live["warmup_frames"].as_u64().expect("кадры прогрева") >= WARMUP_FRAMES);
}

#[test]
fn glass_fails_with_the_warmup_reason_when_the_ui_never_draws_and_removes_the_source() {
    let faults = MirrorFaults {
        ui_overlay: UiOverlayFault::NeverDraws,
        ..MirrorFaults::default()
    };
    let (harness, result) = run_glass(faults, true);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("стекло не нарисовало 8 кадров") && error.contains("прогрева"),
        "{error}"
    );
    assert!(
        error.contains("рендер UI не создан"),
        "последняя ошибка слоя UI: {error}"
    );
    let names = harness.output.stage_names();
    assert!(names.contains(&"glass_warmup".to_owned()), "{names:?}");
    assert!(
        !names.contains(&"glass_live".to_owned()),
        "скриншоты на живом стекле без прогрева: {names:?}"
    );
    assert_eq!(
        harness.control.recorded_ui_overlay().last(),
        Some(&RecordedUiOverlay::Remove),
        "источник снят и при провале прогрева"
    );
}
