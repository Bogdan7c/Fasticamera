//! Сценарий `render`: ритм показа тестовой картинки на настоящем экране.
//!
//! Ход: ждёт, пока поток рендера покажет первые кадры (стадия `presenting`), затем ждёт, пока
//! реальный период экрана (и экрана, и пейсинга) непрерывно продержится в допуске от желаемой
//! частоты `render_refresh_stable_ms` (стадия `refresh_settled`; всего ждёт не дольше
//! `render_refresh_settle_timeout_ms`), и только потом наблюдает окно `render.pacing_window_s`
//! (10 с по умолчанию) и выносит вердикт по данным потока рендера.
//!
//! Зачем отдельная фаза: частота экрана при подключении нестабильна (переходное значение,
//! затем колпак OxygenOS в 60 Гц до ПЕРВОГО касания при цели 120 Гц). Пока не сошлось, раз в
//! `render_refresh_retry_ms` пишется стадия `awaiting_refresh` с полем
//! `observed_slower_than_expected`; драйвер смока шлёт синтетическое касание (`TAP_SCREEN`)
//! только когда оно `true` (экран медленнее цели). Rust не видит, было ли касание, он лишь
//! ждёт нужного периода. Окно пейсинга, начатое до схождения, мерило бы не ту частоту.
//! Вердикт:
//! * пейсинг пройден (доля пропусков ≤ `render.pacing_max_miss_ratio`);
//! * период обновления экрана известен (`VK_GOOGLE_display_timing`) и отличается от периода
//!   желаемой частоты не больше, чем на `scenarios.render_refresh_tolerance_ratio`;
//! * кадры показаны, показ идёт сейчас, у потока нет неисправности, ошибок бэкенда и (если validation
//!   включена) ошибок validation.
//!
//! Вердикт — чистая функция [`evaluate`]: её проверяют тесты на последовательностях статусов.

use std::time::{Duration, Instant};

use render_loop::pacing::{HISTOGRAM_BUCKETS, PacingVerdict};
use render_loop::thread::{RenderState, RenderStatus};
use serde::Serialize;
use shell_contract::commands::FrameRateOutcome;

use super::observing::{ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields};
use super::render_metrics::{DisplayReportPart, ValidationReportPart, Verdict, millis};
use crate::gpu_services::{RenderObservation, ValidationState};

/// Параметры вердикта (из конфигов, не константы).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderCheckParams {
    /// Период, который ждём от экрана при желаемой частоте.
    pub expected_period: Duration,
    /// Допустимое относительное отклонение реального периода от ожидаемого.
    pub refresh_tolerance_ratio: f64,
    /// Допустимая доля пропусков (из `render.pacing_max_miss_ratio`): вердикт выносит поток рендера,
    /// сюда она попадает только для отчёта, чтобы скрипт мог перепроверить число.
    pub max_miss_ratio: f64,
    /// Сколько после начала показа ждали схождения периода экрана (в отчёт).
    pub refresh_settled_after: Duration,
    /// Минимальная полнота данных пейсинга (из `render.pacing_min_coverage_ratio`): вердикт
    /// выносит поток рендера, сюда она попадает для отчёта — скрипт смока перепроверяет долю.
    pub min_coverage_ratio: f64,
    /// Писал ли сценарий стадию `awaiting_refresh` с `observed_slower_than_expected = true`,
    /// то есть просил ли синтетическое касание (в отчёт, справочно).
    pub synthetic_touch_requested: bool,
}

#[derive(Debug, Serialize)]
struct PacingReportPart {
    histogram: [u64; HISTOGRAM_BUCKETS],
    intervals: u64,
    misses: u64,
    miss_ratio: f64,
    max_miss_ratio: f64,
    observed_span_s: f64,
    /// Потерянные записи времён показа: дыры измерения, в пропуски не входят.
    timing_gaps: u64,
    /// Сколько кадров охватывает окно (метки плюс потерянные записи).
    frames_covered: u64,
    /// Доля кадров окна, давших честный интервал (`intervals / frames_covered`, 0 при пустом окне).
    coverage_ratio: f64,
    /// Порог полноты из конфига: ниже него вердикт пейсинга — «данных мало».
    min_coverage_ratio: f64,
    /// Метки показа, отброшенные из-за обратного хода часов.
    rejected_out_of_order: u64,
}

#[derive(Debug, Serialize)]
struct RefreshReportPart {
    /// Реальный период обновления по данным платформы (`None` — неизвестен).
    measured_period_ms: Option<f64>,
    expected_period_ms: f64,
    /// Период, по которому судил пейсинг.
    pacing_period_ms: f64,
    tolerance_ratio: f64,
    /// Сколько ждали схождения периода после начала показа, мс.
    settled_after_ms: f64,
    /// Сценарий просил синтетическое касание: хотя бы одна стадия `awaiting_refresh` вышла с
    /// `observed_slower_than_expected = true` (OxygenOS держит 60 Гц до первого касания).
    /// Rust не может проверить, что касание дошло; это справка для читателя отчёта.
    synthetic_touch_requested: bool,
}

#[derive(Debug, Serialize)]
struct RenderReport {
    state: String,
    frames_presented: u64,
    acquire_timeouts: u64,
    recreations: u64,
    backend_errors: u64,
    last_backend_error: Option<String>,
    pacing: PacingReportPart,
    refresh: RefreshReportPart,
    display: Option<DisplayReportPart>,
    validation: ValidationReportPart,
    frame_rate_request: Option<FrameRateOutcome>,
}

/// Судит снимок состояния потока рендера по правилам сценария.
pub fn evaluate(status: &RenderStatus, validation: ValidationState, params: RenderCheckParams) -> Verdict {
    let mut violations = Vec::new();

    if let Some(fault) = &status.fault {
        violations.push(format!("поток рендера неисправен: {:?}: {}", fault.kind, fault.message));
    }
    if status.state != RenderState::Presenting {
        violations.push(format!("к концу окна показ не идёт, состояние {:?}", status.state));
    }
    if status.frames_presented == 0 {
        violations.push("ни одного показанного кадра".to_owned());
    }
    match status.pacing.verdict {
        PacingVerdict::Passed => {}
        PacingVerdict::Failed {
            miss_ratio,
            max_miss_ratio,
            misses,
            intervals,
        } => violations.push(format!(
            "пропуски кадров: доля {miss_ratio:.4} ({misses} из {intervals}) выше допустимой {max_miss_ratio:.4}"
        )),
        PacingVerdict::NotEnoughData => {
            violations.push("пейсинг: данных для вердикта нет (платформа не вернула времена показа?)".to_owned())
        }
        PacingVerdict::SparseData {
            coverage_ratio,
            min_coverage_ratio,
            intervals,
            frames_covered,
        } => violations.push(format!(
            "пейсинг: данные разрежены, интервалов {intervals} на {frames_covered} кадров окна \
             (доля {coverage_ratio:.3} ниже допустимой {min_coverage_ratio:.3}): много потерянных записей времён показа"
        )),
    }

    let measured_period = status.display.as_ref().and_then(|info| info.refresh_cycle);
    match measured_period {
        None => violations.push("период обновления экрана неизвестен (нет display_timing)".to_owned()),
        Some(measured) => {
            let deviation = relative_deviation(measured, params.expected_period);
            if deviation > params.refresh_tolerance_ratio {
                violations.push(format!(
                    "период обновления {:.3} мс отличается от ожидаемого {:.3} мс на {:.1} % (допуск {:.1} %)",
                    millis(measured),
                    millis(params.expected_period),
                    deviation * 100.0,
                    params.refresh_tolerance_ratio * 100.0
                ));
            }
        }
    }

    let validation_part = ValidationReportPart::of(status, validation);
    violations.extend(validation_part.violation());
    if status.backend_errors != 0 {
        violations.push(format!(
            "ошибки бэкенда рендера: {} (последняя: {})",
            status.backend_errors,
            status.last_backend_error.as_deref().unwrap_or("?")
        ));
    }

    let report = RenderReport {
        state: format!("{:?}", status.state),
        frames_presented: status.frames_presented,
        acquire_timeouts: status.acquire_timeouts,
        recreations: status.recreations,
        backend_errors: status.backend_errors,
        last_backend_error: status.last_backend_error.clone(),
        pacing: PacingReportPart {
            histogram: status.pacing.histogram,
            intervals: status.pacing.intervals,
            misses: status.pacing.misses,
            miss_ratio: status.pacing.miss_ratio,
            max_miss_ratio: params.max_miss_ratio,
            observed_span_s: status.pacing.observed_span.as_secs_f64(),
            timing_gaps: status.pacing.timing_gaps,
            frames_covered: status.pacing.frames_covered,
            coverage_ratio: coverage_ratio(status.pacing.intervals, status.pacing.frames_covered),
            min_coverage_ratio: params.min_coverage_ratio,
            rejected_out_of_order: status.pacing.rejected_out_of_order,
        },
        refresh: RefreshReportPart {
            measured_period_ms: measured_period.map(millis),
            expected_period_ms: millis(params.expected_period),
            pacing_period_ms: millis(status.pacing_refresh_period),
            tolerance_ratio: params.refresh_tolerance_ratio,
            settled_after_ms: millis(params.refresh_settled_after),
            synthetic_touch_requested: params.synthetic_touch_requested,
        },
        display: status.display.as_ref().map(DisplayReportPart::of),
        validation: validation_part,
        frame_rate_request: status.frame_rate_request,
    };
    Verdict::new(report, violations)
}

/// `intervals / frames_covered`; пустое окно — 0.
fn coverage_ratio(intervals: u64, frames_covered: u64) -> f64 {
    if frames_covered == 0 {
        return 0.0;
    }
    intervals as f64 / frames_covered as f64
}

/// `|измеренный − ожидаемый| / ожидаемый`; нулевой ожидаемый период (невозможный конфиг) — бесконечность.
pub(super) fn relative_deviation(measured: Duration, expected: Duration) -> f64 {
    if expected.is_zero() {
        return f64::INFINITY;
    }
    (measured.as_secs_f64() - expected.as_secs_f64()).abs() / expected.as_secs_f64()
}

/// Где сценарий: ждёт первых кадров, ждёт нужной частоты экрана или наблюдает окно.
#[derive(Debug, Clone, Copy)]
enum Phase {
    AwaitingPresenting { since: Instant },
    SettlingRefresh { since: Instant },
    Observing { since: Instant, settled_after: Duration },
}

#[derive(Default)]
pub struct RenderScenario {
    phase: Option<Phase>,
    /// Отсчёт непрерывного пребывания периода в допуске.
    settle: settle::RefreshSettleTracker,
    /// Когда последний раз писали `awaiting_refresh` и какая это по счёту попытка.
    awaiting_refresh: Option<(Instant, u64)>,
    /// Хотя бы раз писали `awaiting_refresh` с `observed_slower_than_expected = true`:
    /// смок в ответ шлёт синтетическое касание (см. `RefreshReportPart`).
    synthetic_touch_requested: bool,
}

impl RenderScenario {
    pub fn new() -> Self {
        Self::default()
    }

    fn advance(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let phase = *self
            .phase
            .get_or_insert(Phase::AwaitingPresenting { since: context.now });
        let (status, validation) = match context.gpu.render.observe() {
            RenderObservation::Unavailable { reason } => {
                return ScenarioStep::Finished(failed(format!("рендер недоступен: {reason}")));
            }
            RenderObservation::Live { status, validation } => (*status, validation),
        };
        if let Some(fault) = &status.fault {
            return ScenarioStep::Finished(failed(format!(
                "поток рендера неисправен: {:?}: {}",
                fault.kind, fault.message
            )));
        }
        match phase {
            Phase::AwaitingPresenting { since } => self.await_presenting(context, &status, since),
            Phase::SettlingRefresh { since } => self.await_refresh_settle(context, &status, since),
            Phase::Observing { since, settled_after } => {
                self.observe_window(context, &status, validation, since, settled_after)
            }
        }
    }

    fn await_presenting(
        &mut self,
        context: &mut ScenarioContext<'_>,
        status: &RenderStatus,
        since: Instant,
    ) -> ScenarioStep {
        if status.state == RenderState::Presenting && status.frames_presented > 0 {
            self.phase = Some(Phase::SettlingRefresh { since: context.now });
            // На эту стадию скрипт смока отвечает синтетическим касанием (OxygenOS поднимает
            // частоту экрана только после первого касания).
            context.emit_stage(
                "presenting",
                stage_fields([
                    ("frames_presented", status.frames_presented.into()),
                    ("window_s", context.gpu.render_config.pacing_window_s.into()),
                    (
                        "refresh_settle_timeout_ms",
                        context.config.render_refresh_settle_timeout_ms.into(),
                    ),
                ]),
            );
            return ScenarioStep::Continue;
        }
        let timeout = Duration::from_millis(context.config.render_stage_timeout_ms);
        if context.now.saturating_duration_since(since) >= timeout {
            return ScenarioStep::Finished(failed(format!(
                "показ не начался за {} мс: состояние {:?}, кадров {}, ошибок бэкенда {}",
                timeout.as_millis(),
                status.state,
                status.frames_presented,
                status.backend_errors
            )));
        }
        ScenarioStep::Continue
    }

    fn check_params(&self, context: &ScenarioContext<'_>, settled_after: Duration) -> RenderCheckParams {
        RenderCheckParams {
            min_coverage_ratio: context.gpu.render_config.pacing_min_coverage_ratio,
            synthetic_touch_requested: self.synthetic_touch_requested,
            expected_period: context.gpu.render_config.expected_refresh_period(),
            refresh_tolerance_ratio: context.config.render_refresh_tolerance_ratio,
            max_miss_ratio: context.gpu.render_config.pacing_max_miss_ratio,
            refresh_settled_after: settled_after,
        }
    }

    /// Ждёт, пока экран устойчиво выйдет на желаемую частоту; окно пейсинга ещё не идёт.
    fn await_refresh_settle(
        &mut self,
        context: &mut ScenarioContext<'_>,
        status: &RenderStatus,
        since: Instant,
    ) -> ScenarioStep {
        let waited = context.now.saturating_duration_since(since);
        let params = self.check_params(context, waited);
        let stable_for = Duration::from_millis(context.config.render_refresh_stable_ms);
        match self.settle.update(context.now, status, &params, stable_for) {
            settle::SettleDecision::Settled => {
                self.phase = Some(Phase::Observing {
                    since: context.now,
                    settled_after: waited,
                });
                context.emit_stage(
                    "refresh_settled",
                    stage_fields([
                        ("waited_ms", millis(waited).into()),
                        ("period_ms", millis(status.pacing_refresh_period).into()),
                    ]),
                );
                ScenarioStep::Continue
            }
            settle::SettleDecision::Waiting { observed, shortfall } => {
                let timeout = Duration::from_millis(context.config.render_refresh_settle_timeout_ms);
                if waited >= timeout {
                    return ScenarioStep::Finished(failed(Self::settle_failure_reason(
                        context, status, &params, observed, shortfall,
                    )));
                }
                self.emit_awaiting_refresh(context, &params, observed, shortfall);
                ScenarioStep::Continue
            }
        }
    }

    /// Пишет `awaiting_refresh` не чаще `render_refresh_retry_ms` (первый раз — сразу).
    /// Повторы нужны драйверу смока: касание, не снявшее колпак с первого раза, повторяется.
    fn emit_awaiting_refresh(
        &mut self,
        context: &mut ScenarioContext<'_>,
        params: &RenderCheckParams,
        observed: Option<Duration>,
        shortfall: settle::RefreshShortfall,
    ) {
        let retry = Duration::from_millis(context.config.render_refresh_retry_ms);
        let attempt = match self.awaiting_refresh {
            Some((last, _)) if context.now.saturating_duration_since(last) < retry => return,
            Some((_, previous_attempt)) => previous_attempt + 1,
            None => 1,
        };
        self.awaiting_refresh = Some((context.now, attempt));
        if shortfall == settle::RefreshShortfall::DisplaySlower {
            self.synthetic_touch_requested = true;
        }
        context.emit_stage(
            "awaiting_refresh",
            stage_fields([
                ("observed_period_ms", observed.map(millis).into()),
                ("expected_period_ms", millis(params.expected_period).into()),
                ("attempt", attempt.into()),
                // Касание снимает только колпак «экран медленнее цели»: см. шапку модуля.
                (
                    "observed_slower_than_expected",
                    (shortfall == settle::RefreshShortfall::DisplaySlower).into(),
                ),
                ("shortfall", format!("{shortfall:?}").into()),
            ]),
        );
    }

    fn settle_failure_reason(
        context: &ScenarioContext<'_>,
        status: &RenderStatus,
        params: &RenderCheckParams,
        observed: Option<Duration>,
        shortfall: settle::RefreshShortfall,
    ) -> String {
        let observed_text = observed.map_or_else(
            || "неизвестен (нет display_timing)".to_owned(),
            |period| format!("{:.3} мс", millis(period)),
        );
        let hint = match shortfall {
            settle::RefreshShortfall::DisplaySlower => {
                "экран медленнее цели (колпак OxygenOS до первого касания: дошло ли синтетическое касание смока?)"
            }
            settle::RefreshShortfall::DisplayFaster => {
                "экран быстрее цели (платформа не приняла просьбу о более низкой частоте)"
            }
            settle::RefreshShortfall::Unknown => "период экрана неизвестен",
            settle::RefreshShortfall::NotYetStable => {
                "период в допуске, но не продержался достаточно долго или пейсинг отстал"
            }
        };
        format!(
            "частота экрана не сошлась за {} мс: период {observed_text}, ожидали {:.3} мс ±{:.1} % непрерывно {} мс, \
             пейсинг судит по {:.3} мс; {hint}",
            context.config.render_refresh_settle_timeout_ms,
            millis(params.expected_period),
            params.refresh_tolerance_ratio * 100.0,
            context.config.render_refresh_stable_ms,
            millis(status.pacing_refresh_period)
        )
    }

    fn observe_window(
        &mut self,
        context: &mut ScenarioContext<'_>,
        status: &RenderStatus,
        validation: ValidationState,
        since: Instant,
        settled_after: Duration,
    ) -> ScenarioStep {
        let window = context.gpu.render_config.pacing_window();
        let waited = context.now.saturating_duration_since(since);
        if waited < window {
            return ScenarioStep::Continue;
        }
        // Окно пейсинга ещё не набралось (например, показ начался только что): даём ещё одно окно.
        let enough = status.pacing.verdict != PacingVerdict::NotEnoughData;
        if !enough && waited < window * 2 {
            return ScenarioStep::Continue;
        }
        let params = self.check_params(context, settled_after);
        ScenarioStep::Finished(evaluate(status, validation, params).into_outcome())
    }
}

impl ObservingScenario for RenderScenario {
    fn name(&self) -> &'static str {
        "render"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }

    fn on_event(&mut self, _event: ObservedEvent, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        self.advance(context)
    }
}

#[path = "render_settle.rs"]
mod settle;

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
