//! Стенд для сквозных тестов `viewfinder-stage-budget` на продуктовом экране (сессия 16a).
//!
//! Играет двух актёров, которых на ПК нет в процессе:
//! * **поток рендера** ([`SurfaceMirror`], подключается к `RenderMirror`): ведёт то, что 16a добавила в диагностику, по
//!   записанным командам: маску подавления (`gpu_work_suppression`), кадры UI «нарисованные / подавленные», запрос и
//!   проходы цветовых приборов, а частоту показа выдаёт как OxygenOS — 120 Гц только пока запрошено 120 И палец на
//!   экране;
//! * **скрипт-драйвер** ([`run_surface`]): читает стадии `budget_gesture` (свайпы входа и выхода) и `budget_hold_touch`
//!   (удерживаемый палец) и выполняет их: свайп — настоящими касаниями по настоящему экрану камеры, палец — отметкой
//!   «держится до такого-то времени» для зеркала рендера.
//!
//! Помощники касаний и часов — минимальные копии из `event_router_screen_tests.rs` (тот модуль не предок этого).

use std::sync::Mutex;

use render_loop::ui_overlay::UiOverlayDiagnostics;
use render_loop::viewfinder::{GpuWorkSuppression, SuppressibleGpuWork, ViewfinderCommand, ViewfinderDiagnostics};
use scope_contract::ScopeIntent;
use shell_contract::clock::MonotonicNanos;
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_draw::frame_source::UiFrameRequest;
use ui_draw::geometry::PxSize;

use super::*;
use crate::camera_screen_config::CameraScreenConfig;
use crate::camera_screen_service::CameraScreenService;
use crate::gpu_test_support::RecordingRenderControl;

const SURFACE_WIDTH_PX: u32 = 1440;
const SURFACE_HEIGHT_PX: u32 = 3216;
/// 1 pt = ширина / 440 px (решение 82).
const PX_PER_PT: f32 = SURFACE_WIDTH_PX as f32 / 440.0;
/// Период кадра синтетического рендера, мс.
const FRAME_MS: i64 = 16;

/// Что ломает подставной рендер на этом стенде.
#[derive(Clone, Copy, Default)]
pub struct SurfaceFaults {
    /// Команда подавления доходит, но рендер её не применяет (в статусе маска остаётся `NONE`, кадры UI рисуются).
    pub suppression_ignored: bool,
}

/// Как ведёт себя скрипт-драйвер.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Driver {
    /// Выполняет каждый жест и держит палец.
    Performs,
    /// Ничего не делает (скрипт упал или не читает logcat).
    Silent,
    /// Свайпы выполняет, а удерживаемое касание — нет (палец не держится).
    IgnoresHolds,
}

/// Модель продуктового экрана в подставном потоке рендера.
pub struct SurfaceMirror {
    faults: SurfaceFaults,
    /// До какого момента палец держится на экране (ставит драйвер, читает зеркало).
    touch_held_until: Arc<Mutex<Option<Instant>>>,
    last_presented: u64,
    ui_frames_drawn: u64,
    ui_frames_suppressed: u64,
    scope_passes: u64,
}

impl SurfaceMirror {
    pub fn new(faults: SurfaceFaults, touch_held_until: Arc<Mutex<Option<Instant>>>) -> Self {
        Self {
            faults,
            touch_held_until,
            last_presented: 0,
            ui_frames_drawn: 0,
            ui_frames_suppressed: 0,
            scope_passes: 0,
        }
    }

    /// Дописывает в диагностику видоискателя то, что зависит от записанных команд.
    pub fn apply(&mut self, control: &RecordingRenderControl, viewfinder: &mut ViewfinderDiagnostics, presented: u64) {
        let delta = presented.saturating_sub(self.last_presented);
        self.last_presented = presented;
        let commands = control.recorded();
        let suppression = commands.iter().rev().find_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::SuppressGpuWork(mask)) => Some(*mask),
            _ => None,
        });
        let applied = if self.faults.suppression_ignored {
            GpuWorkSuppression::NONE
        } else {
            suppression.unwrap_or(GpuWorkSuppression::NONE)
        };
        viewfinder.gpu_work_suppression = applied;
        // Как настоящий рендер (`session/record.rs`): путь кадра отладочной мозаики не доходит до развилки UI и приборов —
        // кадры UI не рисуются и не считаются подавленными, приборы не считаются.
        let mosaic = effective(&commands).painter == ViewfinderPainter::DebugMosaic;
        if suppression.is_some() {
            // Под мозаикой счётчики стоят (приращение 0), но значения всё равно переписываются: общий подставной UI
            // (`mirror_ui_overlay`) считает кадры сам и рос бы.
            self.count_ui_frames(if mosaic { 0 } else { delta }, applied, &mut viewfinder.ui_overlay);
        }
        let scopes_command = commands.iter().rev().find_map(|adjustment| match adjustment {
            RenderAdjustment::Viewfinder(ViewfinderCommand::Scopes(intent)) => Some(*intent),
            _ => None,
        });
        if let Some(intent) = scopes_command {
            self.count_scope_passes(delta, applied, mosaic, intent, viewfinder);
        }
    }

    fn count_ui_frames(&mut self, delta: u64, applied: GpuWorkSuppression, overlay: &mut UiOverlayDiagnostics) {
        if overlay.active_source.is_some() {
            if applied.suppresses(SuppressibleGpuWork::UiOverlayDrawing) {
                self.ui_frames_suppressed += delta;
            } else {
                self.ui_frames_drawn += delta;
            }
        }
        overlay.frames_drawn = self.ui_frames_drawn;
        overlay.frames_suppressed = self.ui_frames_suppressed;
    }

    fn count_scope_passes(
        &mut self,
        delta: u64,
        applied: GpuWorkSuppression,
        mosaic: bool,
        intent: ScopeIntent,
        viewfinder: &mut ViewfinderDiagnostics,
    ) {
        use render_loop::viewfinder::ScopeResources;
        let request = match intent {
            ScopeIntent::Show(request) => Some(request),
            ScopeIntent::Hide => None,
        };
        let computing = request.is_some() && !mosaic && !applied.suppresses(SuppressibleGpuWork::ColorScopes);
        let scopes = &mut viewfinder.scopes;
        scopes.requested = request;
        scopes.residency.resources = if computing {
            ScopeResources::Live
        } else {
            ScopeResources::Absent
        };
        if computing {
            self.scope_passes += delta;
            scopes.gpu_time = Some(micros(800));
        }
        // Время приборов при подавлении НЕ сбрасывается: так ведёт себя настоящий рендер (скользящее окно залипает),
        // и сценарий обязан такое время не читать.
        scopes.counters.passes_recorded = self.scope_passes;
        scopes.float32_controls = gpu_vulkan::context::Float32Controls {
            rounding_mode_rte: true,
            denorm_preserve: true,
            denorm_flush_to_zero: false,
        };
    }

    /// Частота показа по запросу платформы: 120 Гц только пока палец на экране (как OxygenOS), иначе 60.
    pub fn effective_refresh_hz(&self, requested_hz: f64) -> f64 {
        let held = self
            .touch_held_until
            .lock()
            .expect("замок")
            .is_some_and(|until| Instant::now() < until);
        if requested_hz >= 120.0 && held { 120.0 } else { 60.0 }
    }
}

/// Синтетические часы потока рендера: каждый кадр на 16 мс позже прошлого.
struct UiClock {
    now_ms: i64,
}

impl UiClock {
    fn frame(&mut self, harness: &Harness) {
        self.now_ms += FRAME_MS;
        let request = UiFrameRequest {
            present_time: MonotonicNanos::from_nanos(self.now_ms * 1_000_000),
            logical_size: PxSize::new(SURFACE_WIDTH_PX as f32, SURFACE_HEIGHT_PX as f32),
            px_per_pt: PX_PER_PT,
            backdrop: ui_draw::frame_source::BackdropContinuity::Live,
        };
        if let Some(produced) = harness.control.produce_ui_frame(&request) {
            produced.expect("кадр экрана построен");
        }
    }

    fn frames(&mut self, harness: &Harness, count: i64) {
        for _ in 0..count {
            self.frame(harness);
        }
    }
}

fn touch(harness: &mut Harness, phase: TouchPhase, fraction_x: f64, fraction_y: f64, at_ms: i64) {
    harness.deliver(ShellEvent::Touch(TouchSample {
        pointer: PointerId::new(0),
        phase,
        position: SurfacePoint {
            x_px: (fraction_x * f64::from(SURFACE_WIDTH_PX - 1)) as f32,
            y_px: (fraction_y * f64::from(SURFACE_HEIGHT_PX - 1)) as f32,
        },
        time: MonotonicNanos::from_nanos(at_ms * 1_000_000),
    }));
}

fn number(gesture: &serde_json::Value, field: &str) -> f64 {
    gesture[field]
        .as_f64()
        .unwrap_or_else(|| panic!("в жесте стадии нет числа {field}: {gesture}"))
}

/// Свайп из стадии `budget_gesture` настоящими касаниями (как `adb input swipe`).
fn perform_swipe(harness: &mut Harness, clock: &mut UiClock, gesture: &serde_json::Value) {
    assert_eq!(gesture["kind"], "swipe", "жест входа и выхода — свайп: {gesture}");
    let (x0, y0) = (number(gesture, "x"), number(gesture, "y"));
    let (x1, y1) = (number(gesture, "end_x"), number(gesture, "end_y"));
    let duration_ms = number(gesture, "duration_ms") as i64;
    let moves = (duration_ms / FRAME_MS).max(2);
    let start = clock.now_ms;
    touch(harness, TouchPhase::Began, x0, y0, start);
    for index in 1..=moves {
        let t = index as f64 / moves as f64;
        clock.frame(harness);
        touch(
            harness,
            TouchPhase::Moved,
            x0 + (x1 - x0) * t,
            y0 + (y1 - y0) * t,
            start + duration_ms * index / moves,
        );
    }
    clock.frame(harness);
    touch(harness, TouchPhase::Ended, x1, y1, start + duration_ms + 10);
    clock.frames(harness, 4);
}

/// Конфиг экрана: мгновенный фейк-снимок и отчёт на каждый кадр (тест читает его сразу).
pub fn instant_screen_config() -> CameraScreenConfig {
    CameraScreenConfig {
        fake_capture_delay_ms: 0,
        report_min_interval_ms: 0,
        ..CameraScreenConfig::default()
    }
}

/// Прогон `viewfinder-stage-budget` на настоящем экране до итога.
pub fn run_surface(
    config: CameraConfig,
    faults: SurfaceFaults,
    driver: Driver,
    plug: PowerPlug,
) -> (Harness, serde_json::Value) {
    let (harness, result, _times) = run_surface_timed(config, faults, driver, plug);
    (harness, result)
}

/// То же, плюс момент, когда тест впервые увидел каждую стадию (по порядку выдачи): так проверяется «не раньше, чем».
pub fn run_surface_timed(
    config: CameraConfig,
    faults: SurfaceFaults,
    driver: Driver,
    plug: PowerPlug,
) -> (Harness, serde_json::Value, Vec<(String, Instant)>) {
    let mut harness = harness_for_rig(FakeCameraRig::with_config(config));
    harness.router.camera_screen = CameraScreenService::new(instant_screen_config());
    harness.router.install_camera_screen();
    harness.deliver(ShellEvent::Resumed);
    let paced = PumpFaults {
        paced_at_sensor_rate: true,
        ..PumpFaults::default()
    };
    let _pump = StreamPump::start(harness.rig(), Arc::clone(&harness.render), paced);
    let touch_held_until = Arc::new(Mutex::new(None));
    let _mirror = RenderMirror::start_with_surface(
        &harness,
        MirrorFaults::default(),
        Some(SurfaceMirror::new(faults, Arc::clone(&touch_held_until))),
    );
    let _battery = BatteryPump::start(&harness, plug);
    harness.start(Scenario::parse("viewfinder-stage-budget").expect("сценарий зарегистрирован"));
    let mut clock = UiClock { now_ms: 0 };
    let (mut gestures_done, mut holds_done) = (0, 0);
    let mut stage_times: Vec<(String, Instant)> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(90);
    while harness.output.results.lock().expect("замок").is_empty() && Instant::now() < deadline {
        harness.router.step(Duration::from_millis(2));
        clock.frame(&harness);
        let names = harness.output.stage_names();
        while stage_times.len() < names.len() {
            stage_times.push((names[stage_times.len()].clone(), Instant::now()));
        }
        let gestures = stages_named(&harness, "budget_gesture");
        while driver != Driver::Silent && gestures_done < gestures.len() {
            perform_swipe(&mut harness, &mut clock, &gestures[gestures_done]["gesture"]);
            gestures_done += 1;
        }
        let holds = stages_named(&harness, "budget_hold_touch");
        while holds_done < holds.len() {
            let hold = &holds[holds_done];
            holds_done += 1;
            if driver == Driver::Performs && hold["needs_new_press"] == true {
                let hold_ms = hold["gesture"]["hold_ms"].as_u64().expect("hold_ms");
                *touch_held_until.lock().expect("замок") = Some(Instant::now() + Duration::from_millis(hold_ms));
            }
        }
    }
    let result = harness.output.single_result();
    (harness, result, stage_times)
}
