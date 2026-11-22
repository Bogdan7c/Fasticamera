//! Отчёт сценария `capture` (сессия 14): записи снимков и шагов, учёт серии, вид отчёта (сводка в строку результата,
//! подробности — строками `detail_capture`). Вердикт — отдельная чистая функция в `capture_verdict`.
//!
//! Строка результата ≤ 4000 байт (`detail.rs`). Адреса файлов (`uris`) обязаны лежать именно в ней: стадии не
//! повторяются, а скрипт ПК по этим адресам проверяет и потом удаляет файлы. Остальное — компактные записи.

use serde::Serialize;
use serde_json::{Value, json};

use super::capture_burst::BurstRecord;
use super::detail::to_value;
use crate::capture_backend::CaptureMode;
use crate::capture_scenario_config::FullFrameSize;

/// Чем кончился снимок.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ShotOutcome {
    /// Файлы записаны: адрес основной картинки и DNG (если был в плане).
    Saved { primary: String, raw: Option<String> },
    /// Служба сообщила срыв.
    Failed { failure: String },
    /// Итога не дождались: `secured` — «кадр захвачен» пришёл, «сохранено» нет.
    Unfinished { secured: bool },
}

/// Один принятый службой снимок прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ShotRecord {
    /// Сквозной номер снимка прогона (с 1).
    pub number: u32,
    pub step: &'static str,
    /// Номер запроса экрана.
    pub request: u64,
    /// Номер снимка службы (`None` — служба не успела сказать ни одной фазы).
    pub still: Option<u64>,
    /// Что план шага ожидал от RAW.
    pub raw_expected: bool,
    /// Что на самом деле просил экран в запросе (по журналу запросов).
    pub raw_requested: bool,
    /// Формат основной картинки запроса: `heif` или `jpeg`.
    pub format: &'static str,
    /// Соотношение сторон кадра запроса (`4:3`…).
    pub aspect: &'static str,
    /// Зум запроса × 1000 (1000 — 1×).
    pub zoom_milli: u32,
    /// От нажатия до «кадр захвачен», мс.
    pub secured_after_ms: Option<u64>,
    /// От нажатия до «сохранено», мс.
    pub saved_after_ms: Option<u64>,
    pub outcome: ShotOutcome,
    /// Как выбран кадр (`FrameChoice`: точный или замена с причиной). Службы записи пока не отдаёт его приложению
    /// (он живёт в событии проявления), поэтому `null`; причина — `frame_choice_note` отчёта.
    pub frame_choice: Option<Value>,
}

/// Итог одного шага плана.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StepRecord {
    pub name: &'static str,
    /// Номера снимков шага.
    pub shots: Vec<u32>,
    /// Почему шаг не прошёл; `None` — шаг дошёл до конца.
    pub failure: Option<String>,
}

/// Сколько команд снимка принял рендер за прогон (из `RenderStatus.still_commands`, приращение).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct RenderStillDelta {
    pub begins_accepted: u64,
    pub begins_unsupported: u64,
    pub targets_accepted: u64,
    pub targets_dropped: u64,
    pub lost_to_backend_fault: u64,
}

/// Что наблюдали за весь прогон.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CaptureObservationReport {
    pub planned_steps: usize,
    pub steps: Vec<StepRecord>,
    pub shots: Vec<ShotRecord>,
    pub burst: Option<BurstRecord>,
    /// Шаги, до которых дело не дошло.
    pub unfinished: Vec<&'static str>,
    /// Причина провала прогона (сквозной отказ); `None` — прогон дошёл до конца.
    pub run_failure: Option<String>,
    /// Режим сервиса снимка при готовности и в конце: оба обязаны быть настоящими.
    pub mode_at_start: CaptureMode,
    pub mode_at_end: CaptureMode,
    /// Записи журнала итогов, вытесненные до чтения (должно быть 0).
    pub phase_entries_lost: u64,
    /// Приращение учёта команд снимка в рендере; `None` — рендер не прочитан.
    pub render_still: Option<RenderStillDelta>,
    /// Место под подробную диагностику снимков рендера (кольцо кадров, выбор, проявление): её добавит главный
    /// агент, когда рендер начнёт её публиковать. Пока всегда `None`.
    pub render_still_detail: Option<Value>,
    /// Полный кадр основной камеры из конфига: ПК по нему ждёт размеры файлов 4:3 при 1×.
    pub full_frame: FullFrameSize,
}

/// Пояснение к `ShotRecord::frame_choice`.
pub const FRAME_CHOICE_NOTE: &str =
    "служба записи не отдаёт FrameChoice приложению: поле null до диагностики рендера (14c)";

impl CaptureObservationReport {
    /// Сколько снимков сохранено.
    pub fn saved_count(&self) -> usize {
        self.shots
            .iter()
            .filter(|shot| matches!(shot.outcome, ShotOutcome::Saved { .. }))
            .count()
    }

    /// Все адреса файлов, записанных за прогон (основные и DNG): по ним скрипт ПК проверяет и удаляет файлы.
    /// Берутся и у провального прогона: файлы, которые успели появиться, всё равно надо убрать.
    pub fn uris(&self) -> Vec<&str> {
        self.shots
            .iter()
            .filter_map(|shot| match &shot.outcome {
                ShotOutcome::Saved { primary, raw } => Some([Some(primary.as_str()), raw.as_deref()]),
                ShotOutcome::Failed { .. } | ShotOutcome::Unfinished { .. } => None,
            })
            .flatten()
            .flatten()
            .collect()
    }

    /// Сводка для строки результата: числа, короткие имена и адреса.
    pub fn summary(&self) -> Value {
        json!({
            "steps_planned": self.planned_steps,
            "steps_done": self.steps.iter().filter(|step| step.failure.is_none()).count(),
            "unfinished": self.unfinished.len(),
            "mode": to_value(&self.mode_at_start),
            "mode_end": to_value(&self.mode_at_end),
            "shots_total": self.shots.len(),
            "shots_saved": self.saved_count(),
            "shots": self.shots.iter().map(compact_shot).collect::<Vec<_>>(),
            "burst": self.burst.as_ref().map(BurstRecord::summary),
            "phase_entries_lost": self.phase_entries_lost,
            "render_still": self.render_still.map(|delta| to_value(&delta)),
            "full_frame": to_value(&self.full_frame),
            "uris": self.uris(),
        })
    }

    /// Подробности для `detail_capture`: итоги, затем шаги, серия и снимки по порядку.
    pub fn detail_items(&self) -> Vec<Value> {
        let mut items = vec![json!({
            "run_failure": self.run_failure,
            "unfinished": self.unfinished,
            "frame_choice_note": FRAME_CHOICE_NOTE,
            "render_still_detail": self.render_still_detail,
        })];
        items.extend(self.steps.iter().map(to_value));
        items.extend(self.burst.iter().map(to_value));
        items.extend(self.shots.iter().map(to_value));
        items
    }
}

/// Короткая запись снимка для сводки: номер, шаг, исход, времена и размеры запроса.
fn compact_shot(shot: &ShotRecord) -> Value {
    let outcome = match &shot.outcome {
        ShotOutcome::Saved { raw, .. } => {
            if raw.is_some() {
                "saved_with_raw"
            } else {
                "saved"
            }
        }
        ShotOutcome::Failed { .. } => "failed",
        ShotOutcome::Unfinished { .. } => "unfinished",
    };
    json!({
        "n": shot.number,
        "step": shot.step,
        "outcome": outcome,
        "fmt": shot.format,
        "aspect": shot.aspect,
        "zoom_milli": shot.zoom_milli,
        "secured_ms": shot.secured_after_ms,
        "saved_ms": shot.saved_after_ms,
    })
}

#[cfg(test)]
#[path = "capture_report_tests.rs"]
mod tests;
