//! Отчёт сценария `zoom` (сессия 13): итоги шагов, суммарные счётчики и вид отчёта (сводка в строку результата,
//! подробности — строками `detail_zoom`). Вердикт — отдельная чистая функция в `zoom_verdict`.
//!
//! Строка результата ограничена 4000 байтами (`detail.rs`), поэтому сводка — только числа и короткие имена; записи
//! шагов целиком уходят в `detail_zoom`.

use camera_contract::ids::{LensFacing, LensRole};
use render_loop::viewfinder::ContinuityCounters;
use serde::Serialize;
use serde_json::{Value, json};

use super::detail::to_value;
use super::zoom_record::ZoomStepRecord;
use crate::camera_screen_source::ZoomView;

/// Состояние экрана и камеры в конце прогона: сценарий обязан оставить 1× основной камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FinalState {
    pub ratio_milli: u32,
    pub facing: LensFacing,
    /// Роль активного объектива движка; `None` — потока нет.
    pub active_role: Option<LensRole>,
}

impl FinalState {
    pub fn of(zoom: ZoomView, active_role: Option<LensRole>) -> Self {
        Self {
            ratio_milli: zoom.ratio_milli,
            facing: zoom.facing,
            active_role,
        }
    }
}

/// Суммы за весь прогон (приращения от готовности до конца).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ZoomTotals {
    /// Чёрные кадры там, где ждали картинку. Должно быть 0.
    pub black_frames_while_picture_expected: u64,
    /// Кадры, показанные удержанными (размытый переход).
    pub held_frames: u64,
    pub holds_revealed_by_timeout: u64,
    /// Счётчики смен объективов движка за прогон.
    pub switches_completed: u64,
    pub switches_failed: u64,
    pub switches_retargeted: u64,
    /// Намерения экрана, которые движок не смог выполнить.
    pub intents_ignored: u64,
    /// Намерения объектива, которые экран положил в ящик.
    pub lens_intents_published: u64,
    /// Вложения в экран (действия и касания), которые канал входа не принял.
    pub input_refusals: u64,
}

impl ZoomTotals {
    /// Часть сумм, которая берётся из счётчиков непрерывности рендера.
    pub fn with_continuity(mut self, start: &ContinuityCounters, end: &ContinuityCounters) -> Self {
        self.black_frames_while_picture_expected = end
            .black_frames_while_picture_expected
            .saturating_sub(start.black_frames_while_picture_expected);
        self.held_frames = end.held_frames_presented.saturating_sub(start.held_frames_presented);
        self.holds_revealed_by_timeout = end
            .holds_revealed_by_timeout
            .saturating_sub(start.holds_revealed_by_timeout);
        self
    }
}

/// Всё, что сценарий `zoom` наблюдал.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoomObservationReport {
    /// Сколько шагов в плане.
    pub planned_steps: usize,
    pub steps: Vec<ZoomStepRecord>,
    /// Шаги, до которых дело не дошло.
    pub unfinished: Vec<String>,
    /// Отказ вне шагов: камера не готова, движок в ошибке, экран не принимает вход.
    pub run_failure: Option<String>,
    pub totals: ZoomTotals,
    pub final_state: Option<FinalState>,
}

impl ZoomObservationReport {
    /// Наибольшее время «действие → событие движка», мс.
    pub fn max_switch_ms(&self) -> Option<u64> {
        self.steps
            .iter()
            .filter_map(|step| step.switch.map(|switch| switch.observed_ms))
            .max()
    }

    /// Наибольшее время «действие → первый живой кадр», мс.
    pub fn max_first_live_frame_ms(&self) -> Option<f64> {
        self.steps
            .iter()
            .filter_map(|step| step.first_live_frame_ms)
            .max_by(f64::total_cmp)
    }

    /// Сводка для строки результата: только числа и короткие имена.
    pub fn summary(&self) -> Value {
        let switches = self.steps.iter().filter(|step| step.switch.is_some()).count();
        json!({
            "steps_planned": self.planned_steps,
            "steps_done": self.steps.iter().filter(|step| step.failure.is_none()).count(),
            "unfinished": self.unfinished.len(),
            "switches_observed": switches,
            "max_switch_ms": self.max_switch_ms(),
            "max_first_live_frame_ms": self.max_first_live_frame_ms().map(super::viewfinder_metrics::round2),
            "black_frames_while_picture_expected": self.totals.black_frames_while_picture_expected,
            "held_frames": self.totals.held_frames,
            "switches_failed": self.totals.switches_failed,
            "switches_completed": self.totals.switches_completed,
            "lens_intents_published": self.totals.lens_intents_published,
            "final": self.final_state.map(|state| json!({
                "ratio_milli": state.ratio_milli,
                "facing": format!("{:?}", state.facing).to_lowercase(),
                "role": state.active_role.map(LensRole::name),
            })),
            "failure": self.run_failure,
        })
    }

    /// Подробности: итоги, затем по записи на шаг (укладываются в строки `detail_zoom`).
    pub fn detail_items(&self) -> Vec<Value> {
        let mut items = vec![json!({ "totals": to_value(&self.totals), "unfinished": self.unfinished })];
        items.extend(self.steps.iter().map(to_value));
        items
    }
}

#[cfg(test)]
#[path = "zoom_report_tests.rs"]
mod tests;
