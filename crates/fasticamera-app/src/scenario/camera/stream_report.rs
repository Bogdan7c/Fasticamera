//! Отчёт и вердикт `camera-stream`: что получилось на каждом шаге и прошёл ли он правила.
//!
//! Данные шага ([`StepObservation`]) собирает сценарий, вердикт — чистая функция
//! [`evaluate_step`], отчёт — [`StepReport`]/[`StreamReport`]. Контракт с `scripts/smoke_checks.py`:
//! имена полей этих структур.

use camera_android::DiagnosticsSnapshot;
use camera_contract::availability::CameraUnavailableReason;
use camera_contract::capture_result::ResultKey;
use camera_contract::stream::ConfiguredStream;
use camera_core::StreamStats;
use serde::Serialize;

use super::backend_summary::BackendSummary;
use super::detail::{EVENTS_IN_REPORT_LIMIT, to_value};
use super::engine_watch::RunFailure;
use super::progress::{TimedNote, lever_name};
use crate::camera_scenario_config::{CaptureResultsExpectation, ExpectedLever, StreamStepConfig};

/// Приращение счётчиков RAW-приёмника бэкенда за шаг.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RawSinkDelta {
    pub frames_acquired: u64,
    pub frames_released: u64,
    pub acquire_errors: u64,
}

impl RawSinkDelta {
    /// Кадров в секунду за окно; `None`, если окно нулевое.
    pub fn frames_per_second(frames: u64, window: std::time::Duration) -> Option<f64> {
        let seconds = window.as_secs_f64();
        (seconds > 0.0).then(|| frames as f64 / seconds)
    }

    /// Разница снимков диагностики. Счётчики только растут; «меньше, чем было» (чужой сброс)
    /// насыщается нулём, а не оборачивается.
    pub fn between(before: &DiagnosticsSnapshot, after: &DiagnosticsSnapshot) -> Self {
        Self {
            frames_acquired: after
                .raw_sink
                .frames_acquired
                .saturating_sub(before.raw_sink.frames_acquired),
            frames_released: after
                .raw_sink
                .frames_released
                .saturating_sub(before.raw_sink.frames_released),
            acquire_errors: after
                .raw_sink
                .acquire_errors
                .saturating_sub(before.raw_sink.acquire_errors),
        }
    }
}

/// Поток, который бэкенд сконфигурировал (в плоском виде для отчёта).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfiguredEntry {
    pub camera: String,
    pub raw_format: &'static str,
    pub buffer_width: u32,
    pub buffer_height: u32,
    pub frame_rate: u32,
    /// Рычаг частоты: `hal_default` / `public_ae_target_range` / `vendor_sensor_frame_duration_override`.
    pub lever: &'static str,
    /// Подробности рычага (диапазон AE либо длительность кадра).
    pub lever_detail: camera_contract::stream::FrameRateMechanism,
}

impl ConfiguredEntry {
    pub fn of(configured: &ConfiguredStream) -> Self {
        let stream = &configured.stream;
        Self {
            camera: stream.camera.to_string(),
            raw_format: stream.raw_format.name(),
            buffer_width: stream.buffer_size.width,
            buffer_height: stream.buffer_size.height,
            frame_rate: stream.frame_rate.frames_per_second(),
            lever: lever_name(&configured.frame_rate_mechanism),
            lever_detail: configured.frame_rate_mechanism,
        }
    }
}

/// Что наблюдалось на шаге.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepObservation {
    pub failure: Option<RunFailure>,
    /// Поток, сконфигурированный бэкендом (есть, если дошли до `Streaming`).
    pub configured: Option<ConfiguredEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub time_to_streaming_ms: Option<u64>,
    /// Статистика сессии по меткам сенсора на конец замера.
    pub stats: Option<StreamStats>,
    /// Состояние движка на конец замера.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_at_end: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_sink: Option<RawSinkDelta>,
    /// Длительность замера (от `Streaming` до снимка), мс.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_ms: Option<u64>,
    /// Частота по RAW-кадрам за замер: `frames_acquired / длительность`. Вторая оценка рядом с измеренной
    /// по меткам сенсора; для камер без финальных результатов — единственная.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_fps: Option<f64>,
    /// Паники колбэков NDK (накопительно за процесс; должно быть 0).
    pub callback_panics: u64,
    /// Результаты кадров без настоящего номера кадра (приращение за шаг).
    pub unmatched_frame_numbers: u64,
    /// Причины недоступности камеры, пришедшие за шаг.
    pub unavailable: Vec<CameraUnavailableReason>,
    /// Журнал событий движка за шаг (первые `EVENTS_IN_REPORT_LIMIT`).
    pub events: Vec<TimedNote>,
    /// Сколько записей журнала не поместилось в `events`.
    pub events_dropped: usize,
}

impl StepObservation {
    /// Журнал для отчёта: первые записи, остальное — счётчиком (журнал целого шага может быть длинным).
    pub fn capped_events(notes: &[TimedNote]) -> (Vec<TimedNote>, usize) {
        let kept = notes.len().min(EVENTS_IN_REPORT_LIMIT);
        (notes[..kept].to_vec(), notes.len() - kept)
    }
}

/// Ключи результата, которые HAL обязан отдавать всегда: без них темп и паспорт кадра не посчитать.
const REQUIRED_RESULT_KEYS: [ResultKey; 3] = [
    ResultKey::SensorTimestamp,
    ResultKey::ExposureTime,
    ResultKey::Sensitivity,
];

/// Судит шаг. `raw_in_flight_limit` — сколько кадров RAW-приёмник вправе держать «в полёте»
/// (`raw_reader_max_images`): разница принятых и отпущенных не должна его превышать.
pub fn evaluate_step(
    config: &StreamStepConfig,
    observation: &StepObservation,
    raw_in_flight_limit: u32,
) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(failure) = &observation.failure {
        violations.push(failure.describe());
        return violations;
    }
    let Some(configured) = &observation.configured else {
        violations.push("нет данных о сконфигурированном потоке".to_owned());
        return violations;
    };
    check_lever(config.expected_lever, configured, &mut violations);
    match (&observation.stats, config.capture_results) {
        (Some(stats), CaptureResultsExpectation::Required) => check_stats(config, stats, &mut violations),
        (Some(stats), CaptureResultsExpectation::NotSentByHal) => {
            check_without_capture_results(config, stats, observation.raw_fps, &mut violations);
        }
        (None, _) => violations.push("нет статистики потока".to_owned()),
    }
    if observation.state_at_end != Some("streaming") {
        violations.push(format!(
            "к концу замера поток не идёт (состояние: {})",
            observation.state_at_end.unwrap_or("неизвестно")
        ));
    }
    if !observation.unavailable.is_empty() {
        violations.push(format!(
            "камера становилась недоступной во время шага: {:?}",
            observation.unavailable
        ));
    }
    match observation.raw_sink {
        Some(sink) => check_raw_sink(sink, raw_in_flight_limit, &mut violations),
        None => violations.push("нет счётчиков RAW-приёмника".to_owned()),
    }
    if observation.callback_panics != 0 {
        violations.push(format!("паники колбэков NDK: {}", observation.callback_panics));
    }
    violations
}

fn check_lever(expected: ExpectedLever, configured: &ConfiguredEntry, violations: &mut Vec<String>) {
    let Some(expected_name) = expected.name() else {
        return;
    };
    if configured.lever != expected_name {
        violations.push(format!("рычаг частоты {}, ожидался {expected_name}", configured.lever));
    }
}

/// Режим «HAL не шлёт финальных результатов»: частота — по RAW-кадрам, ключи и интервалы не проверяются
/// (их нет). `results_completed == 0` — сторож факта: появились финальные результаты, значит прошивка
/// изменила поведение HAL, и ожидание шага (и отчёт сессии 04) надо пересмотреть, а не проходить молча.
fn check_without_capture_results(
    config: &StreamStepConfig,
    stats: &StreamStats,
    raw_fps: Option<f64>,
    violations: &mut Vec<String>,
) {
    if stats.results_completed != 0 {
        violations.push(format!(
            "факт изменился (прошивка?) — пересмотреть ожидание и отчёт 04: HAL камеры {} прислал {} финальных результатов, \
             а шаг ждал только частичные (capture_results = not_sent_by_hal)",
            config.camera_id, stats.results_completed
        ));
    }
    match raw_fps {
        Some(rate) if rate >= config.min_measured_fps => {}
        Some(rate) => violations.push(format!(
            "частота по RAW-кадрам {rate:.2} к/с ниже порога {:.1} к/с (цель {})",
            config.min_measured_fps, config.frame_rate
        )),
        None => violations.push("частота по RAW-кадрам не измерена: нулевое окно замера".to_owned()),
    }
}

fn check_stats(config: &StreamStepConfig, stats: &StreamStats, violations: &mut Vec<String>) {
    if stats.results_completed == 0 {
        violations.push("ни одного результата кадра за замер".to_owned());
    }
    match stats.measured_frames_per_second {
        Some(measured) if measured >= config.min_measured_fps => {}
        Some(measured) => violations.push(format!(
            "частота {measured:.2} к/с ниже порога {:.1} к/с (цель {})",
            config.min_measured_fps, config.frame_rate
        )),
        None => violations.push("частота не измерена: интервалов между кадрами нет".to_owned()),
    }
    if stats.results_without_timestamp != 0 {
        violations.push(format!(
            "результатов без SENSOR_TIMESTAMP: {}",
            stats.results_without_timestamp
        ));
    }
    for missing in &stats.missing_keys {
        if missing.results > 0 && REQUIRED_RESULT_KEYS.contains(&missing.key) {
            violations.push(format!(
                "HAL не отдал ключ {:?} в {} результатах",
                missing.key, missing.results
            ));
        }
    }
}

fn check_raw_sink(sink: RawSinkDelta, in_flight_limit: u32, violations: &mut Vec<String>) {
    if sink.frames_acquired == 0 {
        violations.push("RAW-приёмник не получил ни одного кадра".to_owned());
    }
    let in_flight = sink.frames_acquired.abs_diff(sink.frames_released);
    if in_flight > u64::from(in_flight_limit) {
        violations.push(format!(
            "принято кадров {}, отпущено {}: разница {in_flight} больше допуска {in_flight_limit} (утечка буферов)",
            sink.frames_acquired, sink.frames_released
        ));
    }
    if sink.acquire_errors != 0 {
        violations.push(format!("ошибок acquire у RAW-приёмника: {}", sink.acquire_errors));
    }
}

/// Отчёт одного шага: цель, наблюдения и нарушения.
#[derive(Debug, Clone, Serialize)]
pub struct StepReport {
    pub index: usize,
    pub target: StreamStepConfig,
    #[serde(flatten)]
    pub observation: StepObservation,
    pub violations: Vec<String>,
}

impl StepReport {
    pub fn judge(
        index: usize,
        target: StreamStepConfig,
        observation: StepObservation,
        raw_in_flight_limit: u32,
    ) -> Self {
        let violations = evaluate_step(&target, &observation, raw_in_flight_limit);
        Self {
            index,
            target,
            observation,
            violations,
        }
    }
}

/// Отчёт шага в строке результата: ключевые числа и нарушения (всё остальное — в `detail_steps`).
///
/// Строка результата вмещает 4000 байт на все шаги сразу, поэтому сводка шага тесная: пустые и нулевые
/// «хвосты» (`failure`, `unavailable`, `unmatched_frame_numbers`) в JSON не пишутся, цель — короткая ASCII-подпись,
/// частота округлена до сотых; пороги и ожидаемый рычаг шага в сводку не входят (их судит Rust, они есть в
/// `target` полного отчёта `detail_steps`). Нарушений шага здесь нет: они уже в поле `error` результата (с номером шага),
/// а полный список — в `detail_steps`. Паники колбэков — в сводке бэкенда на весь прогон.
#[derive(Debug, Serialize)]
pub struct StepSummary<'report> {
    pub index: usize,
    /// Короткая подпись цели: `2/raw10/16x9@60` (камера/формат/вырезка@частота). Скрипт берёт из неё целевую частоту.
    pub target: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: &'report Option<RunFailure>,
    /// Рычаг частоты, которым бэкенд просил цель.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lever: Option<&'static str>,
    pub time_to_streaming_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub measured_fps: Option<f64>,
    /// Частота по RAW-кадрам (вторая оценка), к/с, до сотых.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw_fps: Option<f64>,
    /// Режим ожиданий результатов; пишется только `not_sent_by_hal` (обычный `required` — по умолчанию).
    #[serde(skip_serializing_if = "CaptureResultsExpectation::is_required")]
    pub capture_results: CaptureResultsExpectation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub results_completed: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intervals: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gaps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frames_missed_in_gaps: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_interval_ns: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_interval_ns: Option<u64>,
    /// Последняя длительность кадра, которую сообщил HAL (в режиме CamX она ложная — это факт для отчёта).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_reported_frame_duration_ns: Option<u64>,
    #[serde(skip_serializing_if = "<[camera_core::status::MissingKeyCount]>::is_empty")]
    pub missing_keys: &'report [camera_core::status::MissingKeyCount],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capture_failures: Option<u64>,
    pub raw_sink: Option<RawSinkDelta>,
    #[serde(skip_serializing_if = "is_zero")]
    pub unmatched_frame_numbers: u64,
    pub state_at_end: Option<&'static str>,
    #[serde(skip_serializing_if = "<[CameraUnavailableReason]>::is_empty")]
    pub unavailable: &'report [CameraUnavailableReason],
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

impl<'report> StepSummary<'report> {
    /// Урезанная сводка шага на случай, когда полная не помещается в строку результата: остаются
    /// цель, отказ, рычаг, частота, состояние и счётчики RAW-приёмника (по ним судит скрипт).
    pub fn minimal(report: &'report StepReport) -> Self {
        let full = Self::of(report);
        Self {
            index: full.index,
            target: full.target,
            failure: full.failure,
            lever: full.lever,
            time_to_streaming_ms: full.time_to_streaming_ms,
            measured_fps: full.measured_fps,
            raw_fps: full.raw_fps,
            capture_results: full.capture_results,
            results_completed: None,
            intervals: None,
            gaps: None,
            frames_missed_in_gaps: None,
            min_interval_ns: None,
            max_interval_ns: None,
            last_reported_frame_duration_ns: None,
            missing_keys: &[],
            capture_failures: None,
            raw_sink: full.raw_sink,
            unmatched_frame_numbers: 0,
            state_at_end: full.state_at_end,
            unavailable: &[],
        }
    }

    pub fn of(report: &'report StepReport) -> Self {
        let observation = &report.observation;
        let stats = observation.stats.as_ref();
        Self {
            index: report.index,
            target: report.target.short_label(),
            failure: &observation.failure,
            lever: observation.configured.as_ref().map(|configured| configured.lever),
            time_to_streaming_ms: observation.time_to_streaming_ms,
            measured_fps: stats
                .and_then(|stats| stats.measured_frames_per_second)
                .map(|fps| (fps * 100.0).round() / 100.0),
            raw_fps: observation.raw_fps.map(|fps| (fps * 100.0).round() / 100.0),
            capture_results: report.target.capture_results,
            results_completed: stats.map(|stats| stats.results_completed),
            intervals: stats.map(|stats| stats.intervals),
            gaps: stats.map(|stats| stats.gaps),
            frames_missed_in_gaps: stats.map(|stats| stats.frames_missed_in_gaps),
            min_interval_ns: stats.and_then(|stats| stats.min_interval_nanos),
            max_interval_ns: stats.and_then(|stats| stats.max_interval_nanos),
            last_reported_frame_duration_ns: stats
                .and_then(|stats| stats.last_reported_frame_duration)
                .map(|duration| duration.as_nanos()),
            missing_keys: stats.map_or(&[], |stats| stats.missing_keys.as_slice()),
            capture_failures: stats.map(|stats| stats.capture_failures),
            raw_sink: observation.raw_sink,
            unmatched_frame_numbers: observation.unmatched_frame_numbers,
            state_at_end: observation.state_at_end,
            unavailable: &observation.unavailable,
        }
    }
}

/// Отчёт `camera-stream` в строке результата.
#[derive(Debug, Serialize)]
pub struct StreamSummary<'report> {
    /// `true`: полная сводка не поместилась в строку результата, шаги урезаны (полные данные — в `detail_steps`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub degraded: bool,
    pub measure_ms: u64,
    /// Допуск «кадров в полёте» RAW-приёмника (`camera.android.raw_reader_max_images`).
    pub raw_in_flight_limit: u32,
    pub steps: Vec<StepSummary<'report>>,
    pub backend: BackendSummary<'report>,
}

/// Полный отчёт `camera-stream`: что получилось на каждом шаге и диагностика бэкенда в конце.
#[derive(Debug)]
pub struct StreamReport {
    pub measure_ms: u64,
    pub raw_in_flight_limit: u32,
    pub steps: Vec<StepReport>,
    pub backend: DiagnosticsSnapshot,
}

impl StreamReport {
    /// Нарушения всех шагов с подписью шага.
    pub fn violations(&self) -> Vec<String> {
        self.steps
            .iter()
            .flat_map(|step| {
                step.violations
                    .iter()
                    .map(move |violation| format!("шаг {} ({}): {violation}", step.index + 1, step.target.label()))
            })
            .collect()
    }

    /// Сводка для строки результата, не длиннее `budget_bytes` в JSON: полная, а если она не помещается
    /// (много шагов с длинными числами и пропущенными ключами) — урезанная с пометкой `degraded`.
    pub fn summary_within(&self, budget_bytes: usize) -> StreamSummary<'_> {
        let full = self.summary(false);
        let fits = serde_json::to_string(&full).is_ok_and(|json| json.len() <= budget_bytes);
        if fits { full } else { self.summary(true) }
    }

    fn summary(&self, degraded: bool) -> StreamSummary<'_> {
        StreamSummary {
            degraded,
            measure_ms: self.measure_ms,
            raw_in_flight_limit: self.raw_in_flight_limit,
            steps: self
                .steps
                .iter()
                .map(|step| {
                    if degraded {
                        StepSummary::minimal(step)
                    } else {
                        StepSummary::of(step)
                    }
                })
                .collect(),
            backend: BackendSummary::of(&self.backend),
        }
    }

    /// Полные отчёты шагов для строк стадии `detail_steps`.
    pub fn detail_items(&self) -> Vec<serde_json::Value> {
        self.steps.iter().map(to_value).collect()
    }
}

#[cfg(test)]
#[path = "stream_report_tests.rs"]
mod tests;
