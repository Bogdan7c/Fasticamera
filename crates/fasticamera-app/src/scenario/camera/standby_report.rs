//! Отчёт сценария `standby` (13b): что исполнитель записал, суммарные счётчики за прогон и вид отчёта
//! (сводка в строку результата, подробности — строкой `detail_standby`). Вердикт — `standby_verdict`.
//!
//! Строка результата ограничена 4000 байтами (`detail.rs`): сводка — только числа и короткие имена.

use camera_android::standby_diagnostics::StandbySlotSnapshot;
use serde::Serialize;
use serde_json::{Value, json};

use super::detail::to_value;
use super::standby_runner::{FpsRecord, StandbyRecord, SwapRecord};
use super::viewfinder_metrics::round2;

/// Приращения счётчиков потока кадров за прогон.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StreamTotals {
    pub standby_streams_begun: u64,
    pub standby_begins_refused: u64,
    /// Кадры тёплых, сразу возвращённые камере: доказательство, что тёплая стримит.
    pub standby_frames_returned: u64,
    pub standby_promotions: u64,
    pub frames_released_by_promotion: u64,
    pub standby_results_evicted: u64,
    pub standby_pool_exhausted: u64,
    /// Должно быть 0.
    pub leaked_leases: u64,
    pub frames_leaked: u64,
}

/// Приращения счётчиков движка за прогон.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct EngineTotals {
    pub standby_opened: u64,
    pub promotions: u64,
    pub promotion_fallbacks: u64,
    pub standby_failures: u64,
    pub results_dropped_after_swap: u64,
    pub switches_completed: u64,
    pub switches_failed: u64,
    pub intents_ignored: u64,
    pub stale_backend_events: u64,
    pub dropped_capture_events: u64,
}

/// Всё, что сценарий `standby` наблюдал.
#[derive(Debug, Clone, PartialEq)]
pub struct StandbyObservationReport {
    /// Отказ вне условий вердикта: камера не готова, шаг не дождался.
    pub run_failure: Option<String>,
    /// На каком шаге прогон кончился (`done` — дошёл до конца).
    pub stage: &'static str,
    pub record: StandbyRecord,
    pub stream: StreamTotals,
    pub engine: EngineTotals,
    /// Счётчики тёплых по ячейкам из диагностики бэкенда.
    pub backend: Vec<StandbySlotSnapshot>,
    /// Роль активного объектива в конце; `None` — потока нет.
    pub final_role: Option<&'static str>,
}

impl StandbyObservationReport {
    /// Наименьшая частота любой камеры среди окон, где обязаны стримить обе.
    pub fn min_fps_while_both_run(&self) -> Option<f64> {
        self.both_windows()
            .flat_map(|window| [window.primary_fps, window.standby_fps])
            .min_by(f64::total_cmp)
    }

    /// Окна, где обязаны стримить обе камеры.
    pub fn both_windows(&self) -> impl Iterator<Item = &FpsRecord> {
        self.record.windows.iter().filter(|window| window.both_cameras_running)
    }

    /// Результаты тёплых, предложенные потоку кадров, по всем ячейкам бэкенда.
    pub fn backend_results_offered(&self) -> u64 {
        self.backend.iter().map(|slot| slot.results_offered).sum()
    }

    /// Обмены, исполненные не повышением тёплой (то есть переоткрытием).
    pub fn swaps_not_promoted(&self) -> impl Iterator<Item = &SwapRecord> {
        self.record
            .swaps
            .iter()
            .filter(|swap| swap.method != camera_core::lens_status::LensSwitchMethod::PromotedStandby)
    }

    /// Сводка для строки результата: числа и короткие имена.
    pub fn summary(&self) -> Value {
        json!({
            "stage": self.stage,
            "failure": self.run_failure,
            "home_camera": self.record.home_camera,
            "neighbour_camera": self.record.neighbour_camera,
            "time_to_ready_ms": self.record.time_to_ready_ms,
            "swaps": self.record.swaps.iter().map(|swap| json!({
                "leg": to_value(&swap.leg),
                "to_role": swap.to_role,
                "method": to_value(&swap.method),
                "observed_ms": swap.observed_ms,
                "engine_elapsed_ms": swap.engine_elapsed_ms,
            })).collect::<Vec<_>>(),
            "windows": self.record.windows.iter().map(|window| json!({
                "name": window.name,
                "primary_fps": round2(window.primary_fps),
                "standby_fps": round2(window.standby_fps),
                "both": window.both_cameras_running,
            })).collect::<Vec<_>>(),
            "min_fps_while_both_run": self.min_fps_while_both_run().map(round2),
            "release_ms": self.record.release_ms,
            "final_role": self.final_role,
            "stream": to_value(&self.stream),
            "engine": to_value(&self.engine),
            "backend": to_value(&self.backend),
        })
    }

    /// Подробности: полная запись исполнителя (окна с длинами, обмены).
    pub fn detail_items(&self) -> Vec<Value> {
        vec![to_value(&self.record)]
    }
}
