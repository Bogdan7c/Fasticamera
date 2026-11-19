//! Всё, что сценарий `ae-trace` собрал за прогон, — одной структурой для вердикта и отчёта.
//!
//! Структура ничего не считает и не форматирует: сценарий (`ae_trace.rs`) заполняет её по ходу прогона,
//! вердикт (`ae_trace_verdict`) и отчёт (`ae_trace_report`) читают.

use shell_contract::haptics::{HapticOutcome, HapticPattern};

use super::ae_trace_analysis::RunAnalysis;
use super::ae_trace_plan::Timeline;
use super::ae_trace_series::TraceSample;

/// Журнал подсказок владельцу. Подсказка, до которой не дошёл вибромотор, — не тишина: владелец не знает,
/// когда переключать лампу, и ступеньки получатся пропущенными.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct HapticLog {
    /// Сколько сигналов платформа приняла в очередь.
    pub queued: u32,
    /// Сигналы, которые не приняты: что хотели сыграть и чем кончилось.
    pub failures: Vec<String>,
}

impl HapticLog {
    /// Записывает итог одного сигнала. `moment` — когда он был нужен (для читаемого нарушения).
    pub fn record(&mut self, moment: &str, pattern: HapticPattern, outcome: HapticOutcome) {
        match outcome {
            HapticOutcome::Queued(_) => self.queued += 1,
            HapticOutcome::Unsupported | HapticOutcome::NotAttached | HapticOutcome::Backpressure => {
                self.failures
                    .push(format!("{moment} ({}): {outcome:?}", pattern.name()));
            }
        }
    }
}

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
    pub haptics: HapticLog,
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
