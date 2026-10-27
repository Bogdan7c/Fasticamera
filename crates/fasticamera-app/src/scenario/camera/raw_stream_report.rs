//! Отчёт и вердикт `raw-stream`, `raw-stream-endurance` и `raw-stream-cpu-probe`.
//!
//! Наблюдения собирает сценарий ([`RawStreamObservation`]), вердикт — чистая функция
//! [`evaluate_raw_stream`] (по одному правилу на нарушение, тест на каждое правило). Контракт с
//! `scripts/smoke_checks.py` — имена полей [`RawStreamSummary`].

use camera_android::reader_counters::{ArrivalAgeSnapshot, ReaderSnapshot};
use camera_core::StreamStats;
use raw_frame_stream::{PoolPlan, RawStreamCounters};
use serde::Serialize;

use super::detail::to_value;
use super::engine_watch::RunFailure;
use super::progress::TimedNote;
use super::raw_stream_deltas::RenderCameraDelta;
use super::raw_stream_samples::{LatencyReport, OwnershipReport};
use super::stream_report::ConfiguredEntry;
use crate::raw_stream_scenario_config::{RawStreamRunKind, RawStreamScenarioConfig};
use crate::scenario::render_metrics::ValidationReportPart;

/// Какой режим просили у движка (до всяких откатов).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestedMode {
    pub role: &'static str,
    pub raw_format: &'static str,
    pub readout: &'static str,
    pub frame_rate: u32,
}

/// Честная деградация: движок откатился на запасную частоту (решение 23). Не отказ, но в отчёте.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DegradedNote {
    pub requested_fps: u32,
    pub fallback_fps: u32,
    pub failure: String,
}

/// Тепловое состояние в один момент: статус системы и запас до троттлинга.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThermalMoment {
    /// Имя статуса (`none`, `light`, ...); `None`, если сервис не ответил.
    pub status: Option<&'static str>,
    /// Запас до троттлинга (1.0 — порог `severe`); `None`, если система не дала значения.
    pub headroom: Option<f32>,
    /// Почему статус не прочитан.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Что наблюдалось за прогон.
#[derive(Debug, Clone)]
pub struct RawStreamObservation {
    pub kind: RawStreamRunKind,
    pub failure: Option<RunFailure>,
    pub requested: RequestedMode,
    /// Поток, который бэкенд сконфигурировал (есть, если дошли до `Streaming`).
    pub configured: Option<ConfiguredEntry>,
    /// Откат частоты, если был.
    pub degraded: Option<DegradedNote>,
    pub time_to_streaming_ms: Option<u64>,
    /// До первого кадра камеры на экране (для `raw-stream-cpu-probe` не ждётся).
    pub time_to_live_ms: Option<u64>,
    /// Длина окна, по которому считаны частоты, мс.
    pub window_ms: u64,
    /// Частота по меткам сенсора на конец замера, к/с.
    pub sensor_fps: Option<f64>,
    /// Статистика движка по меткам сенсора.
    pub stats: Option<StreamStats>,
    /// Счётчики потока кадров за прогон.
    pub stream: RawStreamCounters,
    /// Счётчики читателей за прогон.
    pub reader: ReaderSnapshot,
    /// Показ кадров рендером за прогон (`None` — рендер кадры камеры не показывал).
    pub render: Option<RenderCameraDelta>,
    /// Кадров, поставленных на показ с момента «рендер показал первый кадр».
    pub presented_since_live: u64,
    pub display_refresh_hz: Option<f64>,
    /// Потолок плана пула и его сумма (`maxImages`).
    pub pool_plan: PoolPlan,
    pub pool_max_images: u32,
    pub validation: Option<ValidationReportPart>,
    pub thermal_start: ThermalMoment,
    pub thermal_end: ThermalMoment,
    pub ownership: OwnershipReport,
    pub latency: LatencyReport,
    pub state_at_end: &'static str,
    /// Проба была взведена (для `raw-stream-cpu-probe`).
    pub cpu_probe_armed: bool,
    pub events: Vec<TimedNote>,
    pub events_dropped: usize,
}

impl RawStreamObservation {
    /// Копий пикселей через CPU на кадр; `None`, пока кадров не было.
    pub fn cpu_copies_per_frame(&self) -> Option<f64> {
        (self.stream.frames_offered > 0).then(|| self.stream.cpu_copies as f64 / self.stream.frames_offered as f64)
    }

    /// Целевая частота по фактически настроенной сессии (с учётом отката частоты).
    pub fn target_fps(&self) -> u32 {
        self.configured
            .as_ref()
            .map_or(self.requested.frame_rate, |configured| configured.frame_rate)
    }

    /// Доля кадров, дошедших до зрителя с метаданными; `None`, пока кадров не было.
    pub fn paired_fraction(&self) -> Option<f64> {
        let stream = &self.stream;
        let judged = stream.frames_paired + stream.frames_missing_meta_timeout + stream.frames_missing_meta_stopped;
        (judged > 0).then(|| stream.frames_paired as f64 / judged as f64)
    }
}

/// Судит прогон. Пустой список — прошёл.
pub fn evaluate_raw_stream(config: &RawStreamScenarioConfig, observation: &RawStreamObservation) -> Vec<String> {
    let mut violations = Vec::new();
    if let Some(failure) = &observation.failure {
        violations.push(failure.describe());
        return violations;
    }
    match observation.kind {
        RawStreamRunKind::CpuProbe => check_cpu_probe(observation, &mut violations),
        RawStreamRunKind::Standard | RawStreamRunKind::Endurance => {
            check_throughput(config, observation, &mut violations);
            check_zero_copy_and_buffers(observation, &mut violations);
            check_presentation(config, observation, &mut violations);
            check_metadata(config, observation, &mut violations);
        }
    }
    violations
}

/// Проба CPU: счётчик копий обязан вырасти, иначе «0 копий» в других прогонах ничего не доказывает.
fn check_cpu_probe(observation: &RawStreamObservation, violations: &mut Vec<String>) {
    if observation.stream.cpu_copies > 0 {
        return;
    }
    if !observation.cpu_probe_armed {
        violations.push("проба CPU-доступа не была взведена: проверять нечего".to_owned());
    } else if observation.reader.cpu_access_probe_failures > 0 {
        violations.push(format!(
            "платформа отвергла пробу CPU-доступа: сбоев пробы {}, копий 0 (кадров предложено {})",
            observation.reader.cpu_access_probe_failures, observation.stream.frames_offered
        ));
    } else if observation.stream.frames_offered == 0 {
        violations.push("ни одного кадра за прогон: проба не могла сработать".to_owned());
    } else {
        violations.push(format!(
            "счётчик cpu_copies не вырос при {} кадрах и работающей пробе: счётчик не подключён?",
            observation.stream.frames_offered
        ));
    }
}

/// Частота по меткам сенсора и состояние движка.
fn check_throughput(
    config: &RawStreamScenarioConfig,
    observation: &RawStreamObservation,
    violations: &mut Vec<String>,
) {
    let target = f64::from(observation.target_fps());
    match observation.sensor_fps {
        Some(fps) if fps >= config.min_fraction_of_target * target => {}
        Some(fps) => violations.push(format!(
            "частота по меткам сенсора {fps:.2} к/с ниже {:.0}% цели {target:.0} к/с",
            config.min_fraction_of_target * 100.0
        )),
        None => violations.push("частота по меткам сенсора не измерена (нет интервалов)".to_owned()),
    }
    if observation.state_at_end != "streaming" {
        violations.push(format!(
            "к концу замера движок не в Streaming (состояние: {})",
            observation.state_at_end
        ));
    }
}

/// Ноль копий через CPU, ни одного утёкшего буфера, пул не исчерпан и не превышен.
fn check_zero_copy_and_buffers(observation: &RawStreamObservation, violations: &mut Vec<String>) {
    let stream = &observation.stream;
    let reader = &observation.reader;
    let must_be_zero = [
        ("копий пикселей через CPU (cpu_copies)", stream.cpu_copies),
        (
            "кадров, потерянных из-за исчерпания пула (dropped_pool_exhausted)",
            stream.dropped_pool_exhausted,
        ),
        ("исчерпаний пула у читателя (pool_exhausted)", reader.pool_exhausted),
        ("утёкших лизингов (leaked_leases)", stream.leaked_leases),
        ("утёкших кадров потока (frames_leaked)", stream.frames_leaked),
        ("утёкших кадров читателя (images_leaked)", reader.images_leaked),
        (
            "сбоев слияния fence (fence_merge_failures)",
            reader.fence_merge_failures,
        ),
        ("ошибок acquire у читателя (acquire_errors)", reader.acquire_errors),
        (
            "читателей, оставленных до конца процесса (readers_leaked)",
            reader.readers_leaked,
        ),
        ("брака «ни одного молча» (lossless_defects)", stream.lossless_defects),
    ];
    for (name, value) in must_be_zero {
        if value != 0 {
            violations.push(format!("{name}: {value}, ожидалось 0"));
        }
    }
    if stream.max_frames_outstanding > u64::from(observation.pool_max_images) {
        violations.push(format!(
            "пик кадров на руках {} больше плана пула {} (maxImages): расчёт пула неверен или идёт утечка",
            stream.max_frames_outstanding, observation.pool_max_images
        ));
    }
    if stream.frames_offered == 0 {
        violations.push("поток не получил ни одного кадра".to_owned());
    }
}

/// Кадры дошли до экрана, release-fence экспортируются, validation чиста.
fn check_presentation(
    config: &RawStreamScenarioConfig,
    observation: &RawStreamObservation,
    violations: &mut Vec<String>,
) {
    let Some(render) = &observation.render else {
        violations.push("рендер не показал ни одного кадра камеры".to_owned());
        return;
    };
    let window_seconds = observation.window_ms as f64 / 1000.0;
    let camera_fps = f64::from(observation.target_fps());
    // Экран медленнее камеры показывает не все кадры: ждём не больше, чем способен показать экран.
    let expected_rate = match observation.display_refresh_hz {
        Some(hz) if hz > 0.0 => hz.min(camera_fps),
        _ => camera_fps,
    };
    let required = config.min_presented_fraction * window_seconds * expected_rate;
    if (observation.presented_since_live as f64) < required {
        violations.push(format!(
            "на экран поставлено {} кадров, нужно не меньше {required:.0} ({:.0}% от {window_seconds:.1} с x {expected_rate:.1} к/с)",
            observation.presented_since_live,
            config.min_presented_fraction * 100.0
        ));
    }
    for (name, value) in [
        (
            "ошибок экспорта release-fence (release_export_failures)",
            render.release_export_failures,
        ),
        (
            "кадров, не подготовленных к показу (camera_frame_failures)",
            render.camera_frame_failures,
        ),
        (
            "лизингов, брошенных при зависшем GPU (leases_leaked_on_teardown)",
            render.leases_leaked_on_teardown,
        ),
    ] {
        if value != 0 {
            violations.push(format!("{name}: {value}, ожидалось 0"));
        }
    }
    if let Some(validation) = &observation.validation
        && let Some(violation) = validation.violation()
    {
        violations.push(violation);
    }
}

/// Спаривание кадров с метаданными и владение паспортом кадра.
fn check_metadata(config: &RawStreamScenarioConfig, observation: &RawStreamObservation, violations: &mut Vec<String>) {
    if observation.ownership.violations != 0 {
        violations.push(format!(
            "метаданные кадров перепутаны: нарушений владения {} ({})",
            observation.ownership.violations,
            observation
                .ownership
                .first_violation
                .as_deref()
                .unwrap_or("без описания")
        ));
    }
    if !config.expect_paired_metadata {
        return;
    }
    match observation.paired_fraction() {
        Some(fraction) if fraction >= config.min_paired_fraction => {}
        Some(fraction) => violations.push(format!(
            "кадров с метаданными {:.2}%, нужно не меньше {:.0}%",
            fraction * 100.0,
            config.min_paired_fraction * 100.0
        )),
        None => violations.push("ни один кадр не прошёл спаривание с метаданными".to_owned()),
    }
    // Без номеров кадров проверка владения пуста: «нарушений 0» ничего бы не доказывало.
    if observation.ownership.frames_with_frame_number == 0 {
        violations.push(
            "ни у одного показанного кадра нет номера кадра Camera2: владение метаданными не проверено".to_owned(),
        );
    }
}

/// Сводка в строке результата. Полные данные — в строках стадий `detail_stream`.
#[derive(Debug, Serialize)]
pub struct RawStreamSummary<'report> {
    /// `true`: полная сводка не поместилась в строку результата, крупные счётчики вынесены в детали.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub degraded_report: bool,
    pub kind: &'static str,
    pub requested: &'report RequestedMode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configured: Option<&'report ConfiguredEntry>,
    /// Откат частоты движка (не отказ, но виден).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_degraded: Option<&'report DegradedNote>,
    pub time_to_streaming_ms: Option<u64>,
    pub time_to_live_ms: Option<u64>,
    pub window_ms: u64,
    pub sensor_fps: Option<f64>,
    pub target_fps: u32,
    pub cpu_copies_per_frame: Option<f64>,
    pub paired_fraction: Option<f64>,
    pub presented_since_live: u64,
    pub pool_max_images: u32,
    pub ownership: &'report OwnershipReport,
    pub latency: &'report LatencyReport,
    /// Возраст кадра при приходе в reader (измерение, не вердикт).
    pub arrival_age: ArrivalAgeReport,
    pub thermal_start: &'report ThermalMoment,
    pub thermal_end: &'report ThermalMoment,
    pub state_at_end: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub render: Option<&'report RenderCameraDelta>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub validation: Option<&'report ValidationReportPart>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<&'report RawStreamCounters>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reader: Option<&'report ReaderSnapshot>,
}

/// Компактный возраст кадра при приходе в reader (от начала экспозиции до нашего reader — доля HAL
/// в задержке): `CLOCK_BOOTTIME − SENSOR_TIMESTAMP`, миллисекунды.
/// Только измерение: в вердикте не участвует. Поля `None`, пока выборок нет; min/max — за всё время
/// процесса, а не за прогон.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ArrivalAgeReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mean_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_ms: Option<f64>,
    pub samples: u64,
}

impl ArrivalAgeReport {
    pub fn from_snapshot(snapshot: &ArrivalAgeSnapshot) -> Self {
        // Микросекунды -> миллисекунды с точностью до сотых.
        let to_ms = |micros: u64| round_hundredths(micros as f64 / 1_000.0);
        Self {
            mean_ms: snapshot.mean_micros().map(to_ms),
            min_ms: snapshot.min_micros.map(to_ms),
            max_ms: snapshot.max_micros.map(to_ms),
            samples: snapshot.samples,
        }
    }
}

impl RawStreamObservation {
    /// Сводка для строки результата не длиннее `budget_bytes`: сначала полная, при нехватке места —
    /// без больших блоков счётчиков (их полностью несёт `detail_stream`).
    pub fn summary_within(&self, budget_bytes: usize) -> RawStreamSummary<'_> {
        let full = self.summary(true, false);
        let fits = serde_json::to_string(&full).is_ok_and(|json| json.len() <= budget_bytes);
        if fits { full } else { self.summary(false, true) }
    }

    fn summary(&self, with_counters: bool, degraded_report: bool) -> RawStreamSummary<'_> {
        RawStreamSummary {
            degraded_report,
            kind: match self.kind {
                RawStreamRunKind::Standard => "raw-stream",
                RawStreamRunKind::Endurance => "raw-stream-endurance",
                RawStreamRunKind::CpuProbe => "raw-stream-cpu-probe",
            },
            requested: &self.requested,
            configured: self.configured.as_ref(),
            stream_degraded: self.degraded.as_ref(),
            time_to_streaming_ms: self.time_to_streaming_ms,
            time_to_live_ms: self.time_to_live_ms,
            window_ms: self.window_ms,
            sensor_fps: self.sensor_fps.map(round_hundredths),
            target_fps: self.target_fps(),
            cpu_copies_per_frame: self.cpu_copies_per_frame(),
            paired_fraction: self.paired_fraction().map(round_thousandths),
            presented_since_live: self.presented_since_live,
            pool_max_images: self.pool_max_images,
            ownership: &self.ownership,
            latency: &self.latency,
            arrival_age: ArrivalAgeReport::from_snapshot(&self.reader.arrival_age),
            thermal_start: &self.thermal_start,
            thermal_end: &self.thermal_end,
            state_at_end: self.state_at_end,
            render: with_counters.then_some(self.render.as_ref()).flatten(),
            validation: with_counters.then_some(self.validation.as_ref()).flatten(),
            stream: with_counters.then_some(&self.stream),
            reader: with_counters.then_some(&self.reader),
        }
    }

    /// Полные данные для строк стадии `detail_stream`: счётчики, показ, пул, статистика движка, журнал.
    pub fn detail_items(&self) -> Vec<serde_json::Value> {
        vec![
            serde_json::json!({ "stream": to_value(&self.stream), "pool_plan": to_value(&self.pool_plan) }),
            serde_json::json!({ "reader": to_value(&self.reader) }),
            serde_json::json!({
                "render": to_value(&self.render),
                "display_refresh_hz": self.display_refresh_hz,
                "validation": to_value(&self.validation),
            }),
            serde_json::json!({ "engine_stats": to_value(&self.stats) }),
            serde_json::json!({ "events": to_value(&self.events), "events_dropped": self.events_dropped }),
        ]
    }
}

fn round_hundredths(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

fn round_thousandths(value: f64) -> f64 {
    (value * 1000.0).round() / 1000.0
}

#[cfg(test)]
#[path = "raw_stream_report_tests.rs"]
mod tests;
