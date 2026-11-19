//! Отчёт и вердикт сценария `glass` (сессия 11): стекло поверх живого видоискателя.
//!
//! Сценарий ведёт тот же замер видоискателя, что `viewfinder` ([`ViewfinderSmokeObservation`]), и добавляет
//! наблюдения за слоем UI: приращения счётчиков [`UiOverlayDiagnostics`] за окно, GPU-время кадра UI против
//! бюджета, гравитацию. Вердикт ([`evaluate_glass`]) — чистая функция: список нарушений, пустой — прошёл.
//!
//! Правила вердикта:
//! 0. прогрев (`warmup`) в окно не входит: счётчики окна считаются от его конца (см. `glass.rs`).
//! 1. запуск и поток без отказа камеры/рендера (причина — одна, остальное не гадаем);
//! 2. источник UI установлен (к концу окна `active_source` есть), `frames_drawn > 0`;
//! 3. `frames_failed = 0`, `source_failures = 0`, `limit_overflows = 0` (текст последней ошибки — в нарушении);
//! 4. `pyramid_rebuilds > 0`: пирамида размытия фона реально строилась — иначе стекло рисовалось без фона;
//! 5. GPU-время UI замерено, и p95 не больше бюджета `период кадра × ui_gpu_budget_fraction`
//!    (период — из `render.preferred_frame_rate_hz`, решение 36; порог калибруется в 11a, решение 56);
//! 6. кадры, показанные позже задуманного vsync, — только цифра `late_frames`, не провал (решение 91):
//!    ровное опоздание на такт — это задержка, а не рывок (у `viewfinder` без стекла их 0…67 % от прогона
//!    к прогону). Рывок для глаза — кадр выпал или показан дважды: их судит правило 8 с допуском на дрейф;
//! 7. источник снят (иначе стекло осталось бы на экране после сценария);
//! 8. видоискатель не хуже порогов `viewfinder` (к/с, «ровно один раз», копии, validation, джиттер-буфер…) —
//!    нарушения идут с пометкой «видоискатель».
//!
//! Гравитация без показаний за окно — заметка ([`glass_notes`]), не провал: свет от наклона смотрит 11a глазами.

use render_loop::ui_overlay::UiOverlayDiagnostics;
use serde::Serialize;
use serde_json::Value;
use shell_contract::gravity::{GravityReading, GravitySlot, GravitySource};

use super::detail::to_value;
use super::viewfinder_metrics::{GpuTimeMs, round2};
use super::viewfinder_smoke_report::{ViewfinderSmokeObservation, evaluate_viewfinder};
use crate::glass_scenario_config::GlassScenarioConfig;
use crate::viewfinder_scenario_config::ViewfinderSmokeConfig;

/// Приращения счётчиков слоя UI за окно наблюдения (накопительные счётчики рендера минус значения на старте окна).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct OverlayWindow {
    /// Имя источника, который стоит в рендере к концу окна (`None` — не установлен).
    pub active_source: Option<String>,
    pub installs: u64,
    pub removals: u64,
    pub frames_drawn: u64,
    pub frames_failed: u64,
    pub source_failures: u64,
    pub limit_overflows: u64,
    pub pyramid_rebuilds: u64,
    pub pyramid_reuses: u64,
    /// GPU-время кадра UI: скользящее окно рендера на конец наблюдения (не приращение).
    pub gpu_time: Option<GpuTimeMs>,
    /// Текст последней ошибки слоя UI на конец окна.
    pub last_error: Option<String>,
}

impl OverlayWindow {
    /// Приращения между снимками: `before` — до установки источника, `after` — конец окна.
    pub fn between(before: &UiOverlayDiagnostics, after: &UiOverlayDiagnostics) -> Self {
        let delta = |pick: fn(&UiOverlayDiagnostics) -> u64| pick(after).saturating_sub(pick(before));
        Self {
            active_source: after.active_source.clone(),
            installs: delta(|diagnostics| diagnostics.installs),
            removals: delta(|diagnostics| diagnostics.removals),
            frames_drawn: delta(|diagnostics| diagnostics.frames_drawn),
            frames_failed: delta(|diagnostics| diagnostics.frames_failed),
            source_failures: delta(|diagnostics| diagnostics.source_failures),
            limit_overflows: delta(|diagnostics| diagnostics.limit_overflows),
            pyramid_rebuilds: delta(|diagnostics| diagnostics.pyramid_rebuilds),
            pyramid_reuses: delta(|diagnostics| diagnostics.pyramid_reuses),
            gpu_time: GpuTimeMs::of_optional(after.gpu_time.as_ref()),
            last_error: after.last_error.clone(),
        }
    }
}

/// Прогрев слоя UI перед окном замера: сколько кадров UI и мс на него ушло (в окно они не входят).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct WarmupReport {
    /// Кадров UI нарисовано с установки источника до начала окна.
    pub frames: u64,
    /// Сколько мс прошло от установки источника до начала окна.
    pub duration_ms: u64,
}

/// Бюджет GPU-времени кадра UI.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct UiGpuBudget {
    /// Частота показа, по которой считан бюджет (`render.preferred_frame_rate_hz`), Гц.
    pub refresh_hz: f64,
    /// Доля периода кадра (`ui_gpu_budget_fraction`).
    pub fraction: f64,
    /// Бюджет, мс.
    pub budget_ms: f64,
}

impl UiGpuBudget {
    pub fn of(config: &GlassScenarioConfig, refresh_hz: f64) -> Self {
        Self {
            refresh_hz,
            fraction: config.ui_gpu_budget_fraction,
            budget_ms: round2(config.ui_gpu_budget_ms(refresh_hz)),
        }
    }
}

/// Гравитация (свет стекла от наклона) за окно.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum GravityReport {
    /// Подписаться не удалось: сцена идёт со светом в покое. Не провал сценария.
    Unavailable { reason: String },
    /// Подписка жива.
    Active {
        /// Откуда вектор: `dedicated_sensor` / `filtered_accelerometer` / `injected` / `unspecified`.
        source: &'static str,
        /// Показаний датчика принято за окно (0 — датчик молчит: заметка в отчёте).
        samples_in_window: u64,
    },
}

/// Имя источника гравитации для отчёта.
pub fn gravity_source_name(source: GravitySource) -> &'static str {
    match source {
        GravitySource::Unspecified => "unspecified",
        GravitySource::DedicatedSensor => "dedicated_sensor",
        GravitySource::FilteredAccelerometer => "filtered_accelerometer",
        GravitySource::Injected => "injected",
    }
}

/// Сколько показаний датчик принял с запуска (0 — ни одного).
pub fn gravity_samples_received(slot: &GravitySlot) -> u64 {
    match slot.read() {
        GravityReading::NoSampleYet => 0,
        GravityReading::Latest { samples_received, .. } => samples_received,
    }
}

/// Итог снятия источника UI в конце сценария.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum SourceRemoval {
    /// Источник не ставился (сценарий кончился раньше): снимать нечего.
    NotInstalled,
    /// Команда снятия доставлена потоку рендера.
    Delivered,
    /// Доставить не удалось (рендер завершён или не поднимался): стекло могло остаться на экране.
    NotDelivered { reason: String },
}

/// Наблюдения прогона `glass`.
#[derive(Debug, Clone, PartialEq)]
pub struct GlassObservation {
    /// Замер видоискателя за то же окно (те же поля и пороги, что у `viewfinder`).
    pub viewfinder: ViewfinderSmokeObservation,
    pub overlay: OverlayWindow,
    /// Прогрев перед окном (нули — окно не начиналось).
    pub warmup: WarmupReport,
    pub budget: UiGpuBudget,
    pub gravity: GravityReport,
    pub removal: SourceRemoval,
}

impl GlassObservation {
    /// Кадров, показанных позже задуманного vsync, за окно. Это задержка (планировщик показа целился в
    /// линию раньше, чем успел GPU), а не рывок: ровный поток «на такт позже» глаз не видит. Только отчёт.
    pub fn late_frames(&self) -> u64 {
        let shift = &self.viewfinder.window.present_target.vsync_shift;
        shift.late_1 + shift.late_2 + shift.late_3_plus
    }
}

/// Вердикт: список нарушений (пустой — прошёл).
pub fn evaluate_glass(smoke: &ViewfinderSmokeConfig, observation: &GlassObservation) -> Vec<String> {
    if let Some(failure) = &observation.viewfinder.failure {
        return vec![failure.describe()];
    }
    let mut violations = overlay_violations(&observation.overlay);
    violations.extend(gpu_time_violations(&observation.overlay, &observation.budget));
    if let SourceRemoval::NotDelivered { reason } = &observation.removal {
        violations.push(format!("источник UI не снят: {reason}"));
    }
    violations.extend(
        evaluate_viewfinder(smoke, &observation.viewfinder)
            .into_iter()
            .map(|violation| format!("видоискатель: {violation}")),
    );
    violations
}

/// Счётчики слоя UI: источник стоял, кадры рисовались без отказов, пирамида фона строилась.
fn overlay_violations(overlay: &OverlayWindow) -> Vec<String> {
    let mut violations = Vec::new();
    match &overlay.active_source {
        None => violations.push("источник UI не установлен к концу окна (active_source пуст)".to_owned()),
        Some(_) if overlay.frames_drawn == 0 => {
            violations.push("слой UI не нарисовал ни одного кадра (frames_drawn = 0)".to_owned());
        }
        Some(_) => {}
    }
    let last_error = overlay.last_error.as_deref().unwrap_or("текста ошибки нет");
    for (name, value) in [
        ("frames_failed", overlay.frames_failed),
        ("source_failures", overlay.source_failures),
        ("limit_overflows", overlay.limit_overflows),
    ] {
        if value != 0 {
            violations.push(format!(
                "{name} = {value}, ожидалось 0 (последняя ошибка: {last_error})"
            ));
        }
    }
    if overlay.pyramid_rebuilds == 0 {
        violations.push(
            "пирамида размытия фона ни разу не строилась (pyramid_rebuilds = 0): стекло рисовалось без фона".to_owned(),
        );
    }
    violations
}

/// GPU-время кадра UI: замер есть и p95 в бюджете.
fn gpu_time_violations(overlay: &OverlayWindow, budget: &UiGpuBudget) -> Vec<String> {
    match overlay.gpu_time {
        None => vec!["нет замеров GPU-времени кадра UI (ui_overlay.gpu_time)".to_owned()],
        Some(time) if time.p95_ms > budget.budget_ms => vec![format!(
            "GPU-время кадра UI p95 = {} мс больше бюджета {} мс ({} от периода кадра при {} Гц)",
            time.p95_ms, budget.budget_ms, budget.fraction, budget.refresh_hz
        )],
        Some(_) => Vec::new(),
    }
}

/// Заметки прошедшего (и не только) прогона: что не провал, но владельцу стоит знать.
pub fn glass_notes(observation: &GlassObservation) -> Vec<String> {
    match &observation.gravity {
        GravityReport::Unavailable { reason } => {
            vec![format!(
                "гравитация недоступна ({reason}): свет стекла в покое, блик от наклона не проверен"
            )]
        }
        GravityReport::Active {
            samples_in_window: 0,
            source,
        } => vec![format!(
            "гравитация ({source}) не дала ни одного показания за окно: блик от наклона не проверен"
        )],
        GravityReport::Active { .. } => Vec::new(),
    }
}

impl GlassObservation {
    /// Сводка для строки результата: только ограниченное по размеру; подробности — `detail_glass`.
    pub fn summary(&self) -> Value {
        let viewfinder = &self.viewfinder;
        serde_json::json!({
            "failure": viewfinder.failure,
            "window_ms": viewfinder.window.window_ms,
            "overlay": self.overlay,
            "warmup": self.warmup,
            "ui_gpu_budget": self.budget,
            "late_frames": self.late_frames(),
            "source_removal": self.removal,
            "gravity": self.gravity,
            "notes": glass_notes(self),
            "viewfinder": {
                "sensor_fps": viewfinder.sensor_fps.map(round2),
                "isp_fps": viewfinder.window.isp_frames_per_second().map(round2),
                "frames_processed": viewfinder.window.isp.frames_processed,
                "frames_presented": viewfinder.window.camera_frames_presented,
                "cpu_copies": viewfinder.window.isp.cpu_copies + viewfinder.window.stream_cpu_copies,
                "shown_once_ratio": viewfinder.pacing.shown_once_ratio,
                "dropped": viewfinder.pacing.dropped,
                "jitter_overflow": viewfinder.pacing.jitter_overflow,
                "repeats": viewfinder.pacing.repeats,
                "frame_total": viewfinder.gpu_time.frame_total,
            },
            "validation": viewfinder.validation.as_ref().map(|validation| serde_json::json!({
                "mode": validation.mode, "errors": validation.errors,
            })),
        })
    }

    /// Подробности: полная сводка `viewfinder` и его части, затем слой UI целиком.
    pub fn detail_items(&self) -> Vec<Value> {
        let mut items = vec![
            serde_json::json!({ "overlay": to_value(&self.overlay), "warmup": to_value(&self.warmup), "gravity": to_value(&self.gravity) }),
            serde_json::json!({ "viewfinder_summary": self.viewfinder.summary() }),
        ];
        items.extend(self.viewfinder.detail_items());
        items
    }
}

#[cfg(test)]
#[path = "glass_report_tests.rs"]
mod tests;
