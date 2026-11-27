//! Итог прогона стенда насыщения: строка на работу и вердикт.
//!
//! Строка — один из четырёх исходов ([`RowOutcome`]): измерена, стенд отработал без единого кадра, провал с
//! типизированной причиной, не запускалась (отмена). Всё сериализуется: уходит в `FASTICAMERA_DETAIL`.

use std::time::Duration;

use gpu_vulkan::saturation::{EndReason, PassTimeSummary, SaturationReport, Throughput};
use serde::Serialize;
use shell_contract::thermal::ThermalStatus;

use super::digest_report::DigestOutcome;
use super::extras::WorkloadExtras;
use crate::power_measurement::ThermalSnapshot;

/// Время проходов одной метки, мс.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PassRow {
    pub label: &'static str,
    pub median_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}

/// Состав сцены кадра показа по списку рисования (план, до рендера): видно, что это настоящий экран, а не пустой.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct SceneSummary {
    pub glass_groups: u64,
    pub glass_members: u64,
    pub shapes: u64,
    pub texts: u64,
    pub icons: u64,
    pub images: u64,
    /// Из `images`: живые картинки (прибор цвета).
    pub live_images: u64,
    /// Сумма площадей прямоугольников (`bounds`) стеклянных групп, px: ровно та область, где рендер рисует
    /// стекло. Каждый прямоугольник обрезан по экрану; пересечения групп считаются дважды (цена идёт за
    /// закрашенный пиксель каждой группы отдельно), поэтому доля может быть больше фактически занятой.
    pub glass_bounds_area_px: u64,
    /// Площадь экрана, px (для долей).
    pub surface_area_px: u64,
    /// `glass_bounds_area_px / surface_area_px`: чтобы цену стекла отнести к числу закрашенных пикселей. Может быть
    /// больше 1: границы групп с запасом под тень и линзу перекрываются.
    pub glass_screen_fraction: f64,
    /// Площадь ОБЪЕДИНЕНИЯ тех же прямоугольников (перекрытия считаются один раз), px: сколько пикселей экрана
    /// затрагивает стекло хотя бы одной группы.
    pub glass_union_area_px: u64,
    /// `glass_union_area_px / surface_area_px` (от 0 до 1).
    pub glass_union_screen_fraction: f64,
}

/// Счётчики рендера UI за прогон работы показа (то, что рендер на самом деле нарисовал).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct UiCountersSummary {
    pub frames_recorded: u64,
    pub glass_groups: u64,
    pub shape_instances: u64,
    pub glyph_instances: u64,
    pub images_drawn: u64,
    pub live_images_drawn: u64,
    pub live_images_missing: u64,
    pub pyramid_rebuilds: u64,
    pub pyramid_reuses: u64,
}

/// Почему стенд закончил прогон.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReasonName {
    Completed,
    Cancelled,
}

impl EndReasonName {
    fn of(reason: EndReason) -> Self {
        match reason {
            EndReason::Completed => Self::Completed,
            EndReason::Cancelled => Self::Cancelled,
        }
    }
}

/// Что намерил стенд по одной работе.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Measurement {
    pub end_reason: EndReasonName,
    pub warmup_frames: u64,
    pub measured_frames: u64,
    pub drained_frames: u64,
    pub window_ms: f64,
    /// `None` — в окне не завершился ни один кадр.
    pub frames_per_second: Option<f64>,
    /// Цена кадра, мс; `None` вместе с `frames_per_second`.
    pub frame_cost_ms: Option<f64>,
    /// Медианы, p95 и максимумы проходов (метки задаёт сама работа); пусто — GPU без timestamp-запросов.
    pub passes: Vec<PassRow>,
    /// Кадров, у которых времена проходов не сняты.
    pub unmeasured_pass_frames: u64,
    /// Кадров, у которых число меток не совпало с ожидаемым.
    pub mismatched_pass_frames: u64,
    /// к/с по шагам шкалы (по порядку): виден разгон DVFS и троттлинг.
    pub timeline_fps: Vec<f64>,
    /// Цена кадра по шагам шкалы: p95 и максимум (по ним видно «хвост», который среднее скрывает). `None` — ни в
    /// одном шаге не было кадров.
    pub cost: Option<CostSummary>,
}

/// Цена кадра, мс, с тремя значащими цифрами: целиком за окно и по шагам шкалы (`1000 / к/с шага`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CostSummary {
    /// Цена кадра за всё окно (как `frame_cost_ms`, округлена до трёх значащих цифр).
    pub frame_cost_ms: f64,
    /// Шагов шкалы, в которых завершился хотя бы один кадр (по ним считаны p95 и максимум).
    pub steps_with_frames: usize,
    /// p95 цены кадра по шагам (метод «ближайшего ранга»).
    pub step_cost_p95_ms: f64,
    /// Самая дорогая цена кадра среди шагов.
    pub step_cost_max_ms: f64,
}

/// Округление до трёх значащих цифр (как в таблице результата).
pub fn round_to_three_significant(value: f64) -> f64 {
    if !value.is_finite() || value == 0.0 {
        return value;
    }
    let magnitude = value.abs().log10().floor() as i32;
    let scale = 10_f64.powi(2 - magnitude);
    (value * scale).round() / scale
}

/// Цена кадра по шагам шкалы; шаги без кадров (0 к/с) в цену не входят.
fn cost_summary(frame_cost_ms: Option<f64>, timeline_fps: &[f64]) -> Option<CostSummary> {
    let frame_cost_ms = frame_cost_ms?;
    let mut step_costs: Vec<f64> = timeline_fps
        .iter()
        .filter(|fps| **fps > 0.0)
        .map(|fps| 1_000.0 / fps)
        .collect();
    if step_costs.is_empty() {
        return None;
    }
    step_costs.sort_by(f64::total_cmp);
    let nearest_rank = (step_costs.len() * 95).div_ceil(100).max(1) - 1;
    Some(CostSummary {
        frame_cost_ms: round_to_three_significant(frame_cost_ms),
        steps_with_frames: step_costs.len(),
        step_cost_p95_ms: round_to_three_significant(step_costs[nearest_rank]),
        step_cost_max_ms: round_to_three_significant(step_costs[step_costs.len() - 1]),
    })
}

impl Measurement {
    /// Собирает измерение из отчёта стенда и сводки проходов работы.
    pub fn of(report: &SaturationReport, passes: &PassTimeSummary) -> Self {
        let (frames_per_second, frame_cost_ms) = match report.throughput {
            Throughput::Measured {
                frames_per_second,
                frame_cost,
            } => (Some(frames_per_second), Some(millis(frame_cost))),
            Throughput::NoFramesCompleted => (None, None),
        };
        let timeline_fps: Vec<f64> = report.timeline.iter().map(|step| step.frames_per_second()).collect();
        Self {
            cost: cost_summary(frame_cost_ms, &timeline_fps),
            end_reason: EndReasonName::of(report.end_reason),
            warmup_frames: report.warmup_frames,
            measured_frames: report.measured_frames,
            drained_frames: report.drained_frames,
            window_ms: millis(report.window),
            frames_per_second,
            frame_cost_ms,
            passes: passes
                .passes
                .iter()
                .map(|pass| PassRow {
                    label: pass.label,
                    median_ms: millis(pass.median),
                    p95_ms: millis(pass.p95),
                    max_ms: millis(pass.max),
                })
                .collect(),
            unmeasured_pass_frames: passes.unmeasured_frames,
            mismatched_pass_frames: passes.mismatched_frames,
            timeline_fps,
        }
    }

    /// Есть ли цифра пропускной способности.
    pub fn is_measured(&self) -> bool {
        self.frames_per_second.is_some()
    }
}

fn millis(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// Тип провала работы (причина типизирована, текст — в `detail`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureKind {
    /// Работу не удалось создать (память, параметры, экран).
    BuildFailed,
    /// Число слотов работы не совпало с `frames_in_flight` стенда.
    SlotsMismatch,
    /// Стенд не удалось подготовить или он остановил прогон ошибкой GPU/работы.
    BenchFailed,
    /// Работа в ходе прогона сообщила ошибку (`take_failure`).
    WorkloadReportedFailure,
    /// Освобождение работы не удалось (ожидание GPU, завершение проявок).
    ReleaseFailed,
    /// После освобождения живых объектов Vulkan больше, чем было до создания работы.
    ObjectsNotReleased,
    /// Фон кадра показа не удалось опубликовать (окно картинки ISP).
    BackdropFailed,
}

/// Провал работы: тип + подробности.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WorkloadFailure {
    pub kind: FailureKind,
    pub detail: String,
}

impl WorkloadFailure {
    pub fn new(kind: FailureKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
        }
    }
}

impl std::fmt::Display for WorkloadFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.detail)
    }
}

/// Почему работа не запускалась.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NotRunReason {
    /// Прогон отменён раньше (уход в фон, конец сценария, срок).
    Cancelled,
}

/// Исход одной работы.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RowOutcome {
    /// Цифра есть.
    Measured { measurement: Measurement },
    /// Стенд отработал, но в окне не завершился ни один кадр: цифры нет.
    NoFramesCompleted { measurement: Measurement },
    /// Провал с типизированной причиной.
    Failed { failure: WorkloadFailure },
    /// Не запускалась.
    NotRun { reason: NotRunReason },
}

/// Строка таблицы: одна работа.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorkloadRow {
    pub name: &'static str,
    /// Номер работы в списке, с 1.
    pub index: usize,
    pub total: usize,
    pub outcome: RowOutcome,
    /// Состав сцены (только работы показа).
    pub scene: Option<SceneSummary>,
    /// Счётчики рендера UI (только работы показа).
    pub ui: Option<UiCountersSummary>,
    /// Дайджест выхода работы (`None` — работа не дошла до дайджеста: провал или не запускалась).
    pub digest: Option<DigestOutcome>,
    /// Дополнительные факты строки (паритет, размер картинки прибора, счётчик пайплайнов).
    pub extras: WorkloadExtras,
    /// Тепло в начале и конце работы (`None` — работа не запускалась).
    pub thermal_start: Option<ThermalSnapshot>,
    pub thermal_end: Option<ThermalSnapshot>,
    /// Смещение начала работы от начала прогона, мс.
    pub started_at_ms: Option<f64>,
}

impl WorkloadRow {
    /// Строка для работы, которая не запускалась.
    pub fn not_run(name: &'static str, index: usize, total: usize, reason: NotRunReason) -> Self {
        Self {
            name,
            index,
            total,
            outcome: RowOutcome::NotRun { reason },
            scene: None,
            ui: None,
            digest: None,
            extras: WorkloadExtras::default(),
            thermal_start: None,
            thermal_end: None,
            started_at_ms: None,
        }
    }

    /// Есть ли у работы цифра пропускной способности.
    pub fn is_measured(&self) -> bool {
        matches!(self.outcome, RowOutcome::Measured { .. })
    }

    /// Измерение, если стенд отработал (с цифрой или без).
    pub fn measurement(&self) -> Option<&Measurement> {
        match &self.outcome {
            RowOutcome::Measured { measurement } | RowOutcome::NoFramesCompleted { measurement } => Some(measurement),
            RowOutcome::Failed { .. } | RowOutcome::NotRun { .. } => None,
        }
    }

    /// Нарушение для вердикта: `None`, если у работы есть цифра.
    pub fn violation(&self) -> Option<String> {
        let reason = match &self.outcome {
            RowOutcome::Measured { measurement } if measurement.end_reason == EndReasonName::Completed => {
                // Цифра есть; у работ паритета её мало: числа линейки обязаны уложиться в допуск.
                return self
                    .extras
                    .parity
                    .as_ref()
                    .and_then(|parity| parity.violation())
                    .map(|reason| format!("{}: {reason}", self.name));
            }
            RowOutcome::Measured { .. } => "прогон прерван отменой: окно неполное".to_owned(),
            RowOutcome::NoFramesCompleted { .. } => "в окне не завершился ни один кадр".to_owned(),
            RowOutcome::Failed { failure } => failure.to_string(),
            RowOutcome::NotRun { reason } => format!("не запускалась: {reason:?}"),
        };
        Some(format!("{}: {reason}", self.name))
    }

    /// Короткий вердикт строки для компактной таблицы результата.
    pub fn verdict_word(&self) -> &'static str {
        match &self.outcome {
            RowOutcome::Measured { measurement } if measurement.end_reason == EndReasonName::Cancelled => "cancelled",
            RowOutcome::Measured { .. } => "ok",
            RowOutcome::NoFramesCompleted { .. } => "no_frames",
            RowOutcome::Failed { failure } => failure_word(failure.kind),
            RowOutcome::NotRun { .. } => "not_run",
        }
    }
}

fn failure_word(kind: FailureKind) -> &'static str {
    match kind {
        FailureKind::BuildFailed => "build_failed",
        FailureKind::SlotsMismatch => "slots_mismatch",
        FailureKind::BenchFailed => "bench_failed",
        FailureKind::WorkloadReportedFailure => "work_failed",
        FailureKind::ReleaseFailed => "release_failed",
        FailureKind::ObjectsNotReleased => "leaked",
        FailureKind::BackdropFailed => "backdrop_failed",
    }
}

/// Тепловой статус для краткой записи.
pub fn thermal_word(snapshot: &ThermalSnapshot) -> &'static str {
    snapshot.status.map_or("unknown", ThermalStatus::name)
}

/// Что делал поток рендера: счётчики из существующей диагностики (`RenderStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct RenderActivity {
    /// Кадров поставлено на показ.
    pub frames_presented: u64,
    /// Кадров камеры прошло через ISP.
    pub isp_frames: u64,
    /// Кадров экрана собрано из прежней картинки без ISP.
    pub recomposed_frames: u64,
    /// Кадров UI записано.
    pub ui_frames_drawn: u64,
}

impl RenderActivity {
    /// Работа GPU, которую поток рендера выполнил сверх показа: ISP, перекомпоновка, UI.
    pub fn product_gpu_frames(&self) -> u64 {
        self.isp_frames
            .saturating_add(self.recomposed_frames)
            .saturating_add(self.ui_frames_drawn)
    }

    /// Прирост относительно снимка «до».
    pub fn since(&self, before: &RenderActivity) -> RenderActivity {
        RenderActivity {
            frames_presented: self.frames_presented.saturating_sub(before.frames_presented),
            isp_frames: self.isp_frames.saturating_sub(before.isp_frames),
            recomposed_frames: self.recomposed_frames.saturating_sub(before.recomposed_frames),
            ui_frames_drawn: self.ui_frames_drawn.saturating_sub(before.ui_frames_drawn),
        }
    }
}

/// Что делал поток рендера за время замера. Не `bool`: «рендера нет» и «рендер молчал» — разные ответы.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RenderSilence {
    /// Рендер недоступен (GPU или vsync не поднялись): делить GPU со стендом некому.
    RenderUnavailable { reason: String },
    /// Рендер жив: снимки до и после и прирост.
    Observed {
        before: RenderActivity,
        after: RenderActivity,
        delta: RenderActivity,
    },
}

impl RenderSilence {
    /// Прирост продуктовой работы GPU за замер; `0` без рендера.
    pub fn product_gpu_frames(&self) -> u64 {
        match self {
            Self::RenderUnavailable { .. } => 0,
            Self::Observed { delta, .. } => delta.product_gpu_frames(),
        }
    }
}

/// Итог всего прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RunSummary {
    pub rows: Vec<WorkloadRow>,
    /// Прогон отменён до конца списка.
    pub cancelled: bool,
}

/// Вердикт сценария: пустой список нарушений — «прошёл».
///
/// Провал, если хоть одна запрошенная работа не дала цифры (с причиной), либо поток рендера выполнил продуктовую
/// работу GPU сверх допуска (`tolerance_frames`).
pub fn violations(summary: &RunSummary, silence: &RenderSilence, tolerance_frames: u64) -> Vec<String> {
    let mut found: Vec<String> = summary.rows.iter().filter_map(WorkloadRow::violation).collect();
    let product_frames = silence.product_gpu_frames();
    if product_frames > tolerance_frames {
        found.push(format!(
            "поток рендера выполнил {product_frames} кадров продуктовой работы GPU за время замера (допуск {tolerance_frames}): цифры стенда делят GPU с продуктом"
        ));
    }
    found
}

#[cfg(test)]
#[path = "report_tests.rs"]
mod tests;
