//! Всё, что сценарий `ae-trace` собрал за прогон, — одной структурой для вердикта и отчёта.
//!
//! Структура ничего не считает и не форматирует: сценарий (`ae_trace.rs`) заполняет её по ходу прогона,
//! вердикт (`ae_trace_verdict`) и отчёт (`ae_trace_report`) читают.

use super::ae_trace_analysis::RunAnalysis;
use super::ae_trace_plan::Timeline;
use super::ae_trace_series::TraceSample;
use super::operator_prompt_log::PromptLog;

/// Итоги запуска видоискателя для отчёта (то, что нужно из `SessionSummary`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StartTimings {
    pub time_to_streaming_ms: Option<u64>,
    pub time_to_live_ms: Option<u64>,
}

/// Наблюдения прогона.
#[derive(Debug, Default)]
pub struct AeTraceObservation {
    /// Поломка стенда или камеры: прогон не дошёл до конца расписания (текст причины).
    pub failure: Option<String>,
    /// Не удалось вернуть профиль «Фото» в конце.
    pub restore_failure: Option<String>,
    /// Подсказки владельцу на экране: сколько принято платформой и какие нет.
    pub prompts: PromptLog,
    /// Отсчётов отброшено из-за полного ряда.
    pub dropped_samples: u64,
    /// Тиков, на которых диагностики AE не было (после её первого появления).
    pub ticks_without_ae: u64,
    /// Предел подсказки ISP из конфига контроллера, EV. Нужен, пока вердикт сверяет с ним подтяжку.
    pub gain_limit_ev: f64,
    pub timings: StartTimings,
    /// Расписание и ряд — для подробностей; `None` до начала записи.
    pub timeline: Option<Timeline>,
    pub series: Vec<TraceSample>,
    /// Итоги разбора; `None`, если прогон не дошёл до конца.
    pub analysis: Option<RunAnalysis>,
}
