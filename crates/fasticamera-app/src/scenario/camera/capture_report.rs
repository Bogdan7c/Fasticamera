//! Отчёт сценария `capture` (сессия 14): записи снимков и шагов, учёт серии, вид отчёта (сводка в строку результата,
//! подробности — строками `detail_capture`). Вердикт — отдельная чистая функция в `capture_verdict`.
//!
//! Строка результата ≤ 4000 байт (`detail.rs`). Адреса файлов (`uris`) обязаны лежать именно в ней: стадии не
//! повторяются, а скрипт ПК по этим адресам проверяет и потом удаляет файлы. Остальное — компактные записи.

use serde::Serialize;
use serde_json::{Value, json};

use super::capture_burst::BurstRecord;
use super::capture_flash_record::FlashShotRecord;
use super::capture_frame_timing::{FrameSummary, FrameTiming};
use super::capture_plan::STEP_BURST;
use super::capture_render_still::StillDiagnosticsDelta;
use super::detail::to_value;
use crate::capture_backend::CaptureMode;
use crate::capture_scenario_config::FullFrameSize;
use crate::still_capture_service::FlashModeName;

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
    /// Режим вспышки, который план шага ожидал, и который экран на самом деле просил в запросе (14d).
    pub flash_expected: FlashModeName,
    pub flash_requested: FlashModeName,
    /// Решение сервиса и итог камеры по вспышке; `None` — журналы о снимке ничего не знают.
    pub flash: Option<FlashShotRecord>,
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
    /// От нажатия до первого кадра, на котором в миниатюре нарисована картинка этого снимка (14g), мс. Это
    /// НАБЛЮДЕНИЕ, не условие прохождения: `None` — картинку так и не нарисовали (её вытеснил более поздний
    /// снимок, миниатюра пришла без картинки или прогон кончился раньше следующего кадра).
    pub press_to_thumbnail_ms: Option<u64>,
    pub outcome: ShotOutcome,
    /// Какой кадр взят и с какой задержкой (`frame_choice`, `press_to_frame_ms`, `shown_before_press_ms`).
    #[serde(flatten)]
    pub frame: FrameTiming,
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
    /// Приращение диагностики снимков рендера за прогон (`backend.still`): выбор кадра по причинам, потеря GPU,
    /// отказы. `None` — рендер недоступен или не публикует диагностику снимков.
    pub render_still_detail: Option<StillDiagnosticsDelta>,
    /// Полный кадр основной камеры из конфига: ПК по нему ждёт размеры файлов 4:3 при 1×.
    pub full_frame: FullFrameSize,
}

impl CaptureObservationReport {
    /// Сводка «снимок без задержки» по снимкам прогона.
    pub fn frame_summary(&self) -> FrameSummary {
        FrameSummary::of(self.shots.iter().map(|shot| &shot.frame))
    }

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
            "frames": self.frame_summary().compact(),
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
            "frame_summary": to_value(&self.frame_summary()),
            "render_still_detail": self.render_still_detail.as_ref().map(to_value),
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
    let mut compact = json!({
        "n": shot.number,
        "step": shot.step,
        "outcome": outcome,
        "secured_ms": shot.secured_after_ms,
        "saved_ms": shot.saved_after_ms,
    });
    // Строка результата ограничена 4000 байтами, поэтому умолчания не пишутся (скрипт ПК читает отсутствие как
    // умолчание): нет `fmt` — `heif`, нет `aspect` — `4:3`, нет `zoom_milli` — 1000, нет `flash` — вспышка
    // выключена, нет `choice` — взят ровно показанный кадр (`exact`). Остальное (режим, кадр вспышки, замена,
    // неизвестный выбор, итог камеры: выдержка, ISO, доля света) называется явно.
    if let Some(object) = compact.as_object_mut() {
        // Короткий ключ (`thumb_ms`) — по той же причине, что и умолчания: строка результата ограничена 4000 байтами,
        // а худший случай (`result_budget`) занимает почти весь запас. Поэтому в сводке замер только у одиночных
        // снимков; у снимков серии (их десять) он лежит в подробностях `detail_capture` под полным именем поля
        // `press_to_thumbnail_ms` — у каждого снимка по порядку.
        if let Some(thumbnail_ms) = shot.press_to_thumbnail_ms.filter(|_| shot.step != STEP_BURST) {
            object.insert("thumb_ms".to_owned(), json!(thumbnail_ms));
        }
        if shot.format != "heif" {
            object.insert("fmt".to_owned(), json!(shot.format));
        }
        if shot.aspect != "4:3" {
            object.insert("aspect".to_owned(), json!(shot.aspect));
        }
        if shot.zoom_milli != 1_000 {
            object.insert("zoom_milli".to_owned(), json!(shot.zoom_milli));
        }
        if shot.flash_requested != FlashModeName::Off {
            object.insert("flash".to_owned(), to_value(&shot.flash_requested));
        }
        if shot.frame.choice_name() != "exact" {
            object.insert("choice".to_owned(), json!(shot.frame.choice_name()));
        }
        if let Some(camera) = shot.flash.as_ref().and_then(|flash| flash.camera.as_ref()) {
            object.insert("flash_cam".to_owned(), to_value(camera));
        }
    }
    compact
}

#[cfg(test)]
#[path = "capture_report_tests.rs"]
mod tests;
