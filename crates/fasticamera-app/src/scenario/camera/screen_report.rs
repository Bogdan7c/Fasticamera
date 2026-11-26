//! Отчёт и вердикт сценария `screen` (сессия 12): экран камеры iOS поверх живого видоискателя под управлением скрипта.
//!
//! Сценарий ведёт тот же замер видоискателя, что `viewfinder` и `glass` ([`ViewfinderSmokeObservation`]), добавляет
//! итоги шагов (`screen_runner`), счётчики экрана и наблюдения за слоем UI. Вердикт ([`evaluate_screen`]) — чистая
//! функция: список нарушений, пустой — прошёл.
//!
//! Правила вердикта:
//! 1. запуск и поток без отказа камеры/рендера (причина — одна, остальное не гадаем);
//! 2. все шаги плана выполнены; провал шага — причина «ожидали X, увидели Y» (`screen_runner`), невыполненные
//!    шаги названы;
//! 3. счётчики экрана за окно: отказы кадров UI = 0, переполнение очереди эффектов = 0, события жестов без
//!    владельца = 0, переполнение/потеря входа = 0, отброшенный из-за отсутствия экрана вход = 0, отказы исполнителя
//!    эффектов (намерения камере, место видоискателя, запросы снимка) = 0;
//! 4. «тап дошёл до HAL» как в `3a`: результатов с неопознанным происхождением нет (сами триггеры доказывают шаги);
//! 5. слой UI: источник — `camera_screen`, кадры рисовались без отказов, GPU-время p95 в бюджете 0,5 периода кадра
//!    (решение 90);
//! 6. кадры позже задуманного vsync — только цифра `late_frames` (решение 91); рывок (кадр выпал или показан
//!    дважды) судят пороги видоискателя с допуском на дрейф часов — нарушения с пометкой «видоискатель».

use camera_screen::ScreenDiagnostics;
use serde::Serialize;
use serde_json::Value;

use super::detail::to_value;
use super::glass_report::{
    BackdropBlurExpectation, OverlayWindow, UiGpuBudget, WarmupReport, gpu_time_violations, overlay_violations,
};
use super::screen_runner::StepRecord;
use super::viewfinder_metrics::round2;
use super::viewfinder_smoke_report::{ViewfinderSmokeObservation, evaluate_viewfinder_under_touch_load};
use crate::camera_screen_service::DroppedInputCounters;
use crate::camera_screen_source::ScreenReport;
use crate::viewfinder_scenario_config::ViewfinderSmokeConfig;

/// Имя источника UI, которым рендер показывает экран камеры (`CAMERA_SCREEN_SOURCE_NAME`).
pub const EXPECTED_SOURCE: &str = "camera_screen";

/// Значение поля `ev_proof` сводки: EV доказан до желаемого состояния движка, применение HAL не проверялось.
pub const EV_PROOF_ENGINE_DESIRED: &str = "engine_desired_value";

/// Счётчики экрана за окно (приращения от «экран готов» до конца плана).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ScreenCounters {
    pub frames_produced: u64,
    pub frame_failures: u64,
    pub last_frame_failure: Option<String>,
    pub effects_overflowed: u64,
    pub unhandled_gestures: u64,
    /// Продолжения пана без начала в разделе калибровки (страховка маршрутизации жестов, должно быть 0).
    pub orphan_pan_continuations: u64,
    /// Отказы «картинки под пальцем» из-за пустого списка ведомых узлов (должно быть 0).
    pub finger_focus_refusals: u64,
    pub gestures_cancelled: u64,
    pub touches_received: u64,
    /// Вход, потерянный каналом: переполнение касаний и действий.
    pub input_overflowed: u64,
    /// Сообщения, отправленные уничтоженному получателю.
    pub input_receiver_gone: u64,
    /// Вход, отброшенный сервисом, потому что экрана не было.
    pub input_dropped_without_screen: u64,
    /// Отказы исполнителя эффектов: намерения камере, место видоискателя, запросы снимка.
    pub executor_refusals: u64,
    pub haptics_backpressure: u64,
    /// Результатов с неопознанным происхождением запроса за окно (счётчик движка, как в `3a`).
    pub results_with_unknown_origin: u64,
}

impl ScreenCounters {
    /// Приращения между двумя отчётами экрана; `unknown_origin` и `dropped` — по их владельцам.
    pub fn between(
        before: &ScreenReport,
        after: &ScreenReport,
        dropped: (DroppedInputCounters, DroppedInputCounters),
        results_with_unknown_origin: u64,
    ) -> Self {
        let (dropped_before, dropped_after) = dropped;
        let (start, end): (&ScreenDiagnostics, &ScreenDiagnostics) = (&before.diagnostics, &after.diagnostics);
        let grew = |end: u64, start: u64| end.saturating_sub(start);
        let refusals = |report: &ScreenReport| {
            let effects = &report.effects;
            effects.camera.queue_full
                + effects.camera.engine_stopped
                + effects.camera.no_engine
                + effects.framing.render_gone
                + effects.framing.render_unavailable
                + effects.framing.invalid_rect
                + effects.capture.rejected_busy
                + effects.capture.rejected_unavailable
        };
        Self {
            frames_produced: grew(end.frames_produced, start.frames_produced),
            frame_failures: grew(end.frame_failures, start.frame_failures),
            last_frame_failure: end.last_failure.clone(),
            effects_overflowed: grew(end.effects_overflowed, start.effects_overflowed),
            unhandled_gestures: grew(end.unhandled_gestures, start.unhandled_gestures),
            orphan_pan_continuations: grew(end.orphan_pan_continuations, start.orphan_pan_continuations),
            finger_focus_refusals: grew(end.finger_focus_refusals, start.finger_focus_refusals),
            gestures_cancelled: grew(end.gestures.cancelled, start.gestures.cancelled),
            touches_received: grew(end.touches_received, start.touches_received),
            input_overflowed: grew(
                after.input.overflowed_touches + after.input.overflowed_actions,
                before.input.overflowed_touches + before.input.overflowed_actions,
            ),
            input_receiver_gone: grew(after.input.receiver_gone, before.input.receiver_gone),
            input_dropped_without_screen: grew(
                dropped_after.touches + dropped_after.actions,
                dropped_before.touches + dropped_before.actions,
            ),
            executor_refusals: grew(refusals(after), refusals(before)),
            haptics_backpressure: grew(after.effects.haptics.backpressure, before.effects.haptics.backpressure),
            results_with_unknown_origin,
        }
    }
}

/// Наблюдения прогона `screen`.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenObservation {
    /// Замер видоискателя за то же окно (те же поля и пороги, что у `viewfinder`).
    pub viewfinder: ViewfinderSmokeObservation,
    /// Итоги шагов по порядку (последний мог провалиться).
    pub steps: Vec<StepRecord>,
    /// Шаги, до которых дело не дошло.
    pub unfinished: Vec<&'static str>,
    /// Причина провала плана (шаг или сквозной отказ); `None` — план прошёл целиком.
    pub plan_failure: Option<String>,
    pub screen: ScreenCounters,
    pub overlay: OverlayWindow,
    pub warmup: WarmupReport,
    pub budget: UiGpuBudget,
}

impl ScreenObservation {
    /// Кадров, показанных позже задуманного vsync, за окно: задержка, а не рывок (решение 91). Только отчёт.
    pub fn late_frames(&self) -> u64 {
        let shift = &self.viewfinder.window.present_target.vsync_shift;
        shift.late_1 + shift.late_2 + shift.late_3_plus
    }
}

/// Вердикт: список нарушений (пустой — прошёл).
pub fn evaluate_screen(smoke: &ViewfinderSmokeConfig, observation: &ScreenObservation) -> Vec<String> {
    if let Some(failure) = &observation.viewfinder.failure {
        return vec![failure.describe()];
    }
    let mut violations = plan_violations(observation);
    violations.extend(counter_violations(&observation.screen));
    violations.extend(overlay_source_violations(&observation.overlay));
    // Экран PHOTO без стекла (решение 297): пирамида размытия в окне замера не обязана строиться.
    violations.extend(overlay_violations(&observation.overlay, BackdropBlurExpectation::NotRequired));
    violations.extend(gpu_time_violations(&observation.overlay, &observation.budget));
    violations.extend(
        // Под касаниями переполнения буфера — только число в отчёте (решение 105).
        evaluate_viewfinder_under_touch_load(smoke, &observation.viewfinder)
            .into_iter()
            .map(|violation| format!("видоискатель: {violation}")),
    );
    violations
}

/// Шаги плана: провал с причиной, а если причины нет, но шаги не выполнены — их имена.
fn plan_violations(observation: &ScreenObservation) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(reason) = &observation.plan_failure {
        violations.push(reason.clone());
    } else if !observation.unfinished.is_empty() {
        violations.push(format!(
            "не выполнены шаги плана: {} (время сценария вышло раньше, скрипт не дошёл до них?)",
            observation.unfinished.join(", ")
        ));
    }
    violations
}

/// Счётчики экрана за окно: любое ненулевое значение — отдельное нарушение с цифрой.
fn counter_violations(screen: &ScreenCounters) -> Vec<String> {
    let mut violations = Vec::new();
    let last_failure = screen.last_frame_failure.as_deref().unwrap_or("текста ошибки нет");
    let must_be_zero = [
        ("отказов кадра экрана", screen.frame_failures, Some(last_failure)),
        ("переполнений очереди эффектов экрана", screen.effects_overflowed, None),
        ("событий жестов без владельца", screen.unhandled_gestures, None),
        (
            "отказов «картинки под пальцем» без ведомых узлов",
            screen.finger_focus_refusals,
            None,
        ),
        (
            "продолжений пана раздела без начала",
            screen.orphan_pan_continuations,
            None,
        ),
        ("потерь ввода из-за переполнения канала", screen.input_overflowed, None),
        ("сообщений уничтоженному экрану", screen.input_receiver_gone, None),
        (
            "вводов, отброшенных без экрана",
            screen.input_dropped_without_screen,
            None,
        ),
        ("отказов исполнителя эффектов", screen.executor_refusals, None),
        (
            "результатов с неопознанным происхождением (метка триггера теряется)",
            screen.results_with_unknown_origin,
            None,
        ),
    ];
    for (what, count, detail) in must_be_zero {
        if count != 0 {
            let suffix = detail.map(|text| format!(" (последняя: {text})")).unwrap_or_default();
            violations.push(format!("{what}: {count}, ожидалось 0{suffix}"));
        }
    }
    if screen.frames_produced == 0 {
        violations.push("экран не нарисовал ни одного кадра за окно (frames_produced = 0)".to_owned());
    }
    violations
}

/// Источник UI должен быть именно экраном камеры (а не демо-сценой стекла или пустотой).
fn overlay_source_violations(overlay: &OverlayWindow) -> Vec<String> {
    match overlay.active_source.as_deref() {
        Some(EXPECTED_SOURCE) | None => Vec::new(), // «None» назовёт `overlay_violations`
        Some(other) => vec![format!("источник UI в рендере — {other}, ожидался {EXPECTED_SOURCE}")],
    }
}

impl ScreenObservation {
    /// Сводка для строки результата: только ограниченное по размеру; подробности — `detail_screen`.
    pub fn summary(&self) -> Value {
        let viewfinder = &self.viewfinder;
        serde_json::json!({
            "failure": viewfinder.failure,
            "window_ms": viewfinder.window.window_ms,
            "steps": self.steps.iter().map(|step| serde_json::json!({ "name": step.name, "ok": step.ok })).collect::<Vec<_>>(),
            "steps_inferred": self.steps.iter().filter(|step| step.inferred).count(),
            // Шаги EV доказывают «желаемое EV дошло до ДВИЖКА» (`three_a.controls`), а не «HAL применил»: надёжного
            // признака применения в статусе нет. Скрипт показывает это заметкой, владелец проверяет на глаз.
            "ev_proof": EV_PROOF_ENGINE_DESIRED,
            "unfinished": self.unfinished,
            "plan_failure": self.plan_failure,
            "screen": self.screen,
            "overlay": self.overlay,
            "warmup": self.warmup,
            "ui_gpu_budget": self.budget,
            "late_frames": self.late_frames(),
            "viewfinder": {
                "sensor_fps": viewfinder.sensor_fps.map(round2),
                "isp_fps": viewfinder.window.isp_frames_per_second().map(round2),
                "frames_presented": viewfinder.window.camera_frames_presented,
                "cpu_copies": viewfinder.window.isp.cpu_copies + viewfinder.window.stream_cpu_copies,
                "shown_once_ratio": viewfinder.pacing.shown_once_ratio,
                "dropped": viewfinder.pacing.dropped,
                "jitter_overflow": viewfinder.pacing.jitter_overflow,
                "repeats": viewfinder.pacing.repeats,
            },
            "validation": viewfinder.validation.as_ref().map(|validation| serde_json::json!({
                "mode": validation.mode, "errors": validation.errors,
            })),
        })
    }

    /// Подробности: каждый шаг с тем, что увидели, затем слой UI, затем замер видоискателя.
    pub fn detail_items(&self) -> Vec<Value> {
        let mut items: Vec<Value> = self.steps.iter().map(to_value).collect();
        items.push(serde_json::json!({ "screen": to_value(&self.screen), "overlay": to_value(&self.overlay) }));
        items.push(serde_json::json!({ "viewfinder_summary": self.viewfinder.summary() }));
        items.extend(self.viewfinder.detail_items());
        items
    }
}

#[cfg(test)]
#[path = "screen_report_tests.rs"]
mod tests;
