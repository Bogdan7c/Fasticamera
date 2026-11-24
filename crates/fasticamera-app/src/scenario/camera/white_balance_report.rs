//! Отчёт о белой для сценариев `viewfinder` и `3a` (план white-balance, сессия 01).
//!
//! Две стороны одного факта: **что решил владелец белого** (движок: значение, происхождение, исход последней
//! заявки источнику) и **что делает рендер** (какую белую считает ISP, сколько кадров скрыто на холодном старте).
//! Старый оценщик продукт больше не определяет, поэтому его «оценки света» в отчёт не входят. Белая HAL здесь
//! только справка (`hal_delta_*`): в ISP она не попадает.
//!
//! Числа округлены: строка результата ограничена 4000 байтами (`result_budget`).

use camera_contract::white::WhiteOrigin;
use camera_core::white::{WhiteRequestOutcome, WhiteRequestState, WhiteStatus};
use render_loop::viewfinder::{ColdStartState, WhiteBalanceDiagnostics, WhiteReading};
use serde::Serialize;

use super::viewfinder_metrics::round2;

/// Белая точка: температура и Duv.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhitePointReport {
    pub kelvin: f64,
    pub duv: f64,
}

impl WhitePointReport {
    fn of_reading(reading: WhiteReading) -> Self {
        Self {
            kelvin: reading.temperature_kelvin.round(),
            duv: round4(reading.duv),
        }
    }
}

/// Заявка источнику в отчёте.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WhiteRequestReport {
    /// `none` (заявок не было), `waiting` или вид исхода последней закрытой: `applied`, `busy`, `unknown`,
    /// `unavailable`, `failed`, `not_understood`, `timed_out`, `cancelled`.
    pub state: &'static str,
    /// Номер заявки (ожидаемой или последней закрытой).
    pub id: Option<u64>,
    /// Источник заявки.
    pub source: Option<&'static str>,
}

/// Состояние белой на момент снимка.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WhiteBalanceReport {
    /// Белая владельца (движок).
    pub white: WhitePointReport,
    /// Чья она: имя источника (`phone`), `manual` или `fallback`.
    pub origin: String,
    /// Белая, которой считает ISP (`None` — указаний рендер ещё не получил).
    pub applied: Option<WhitePointReport>,
    pub request: WhiteRequestReport,
    /// Применённая белая минус белая HAL, майреды (плюс — применённая теплее). Только справка.
    pub hal_delta_mired: Option<f64>,
    /// То же по Duv.
    pub hal_delta_duv: Option<f64>,
    /// Холодный старт идёт сейчас (кадры скрываются).
    pub cold_start_pending: bool,
    /// Скрыто кадров на холодных стартах за сессию показа.
    pub hidden_frames: u64,
    /// Предел скрытых кадров на одно открытие (конфиг рендера `viewfinder.white_balance.max_hidden_frames`).
    pub max_hidden_frames: u32,
    /// Сколько раз скрытие кончилось принудительным показом (предел исчерпан без ответа). Ноль в строку
    /// результата не пишется: она ограничена 4000 байтами, а здоровый прогон «нет поля» = «ноль».
    #[serde(skip_serializing_if = "is_zero")]
    pub cold_start_forced: u64,
    /// Каждый N-й кадр считает статистику (конфиг рендера); на ней стоит AE.
    pub statistics_every_n_frames: u32,
    /// Ответов источника отброшено владельцем как чужих (гонки). Ноль в строку результата не пишется (см. выше).
    #[serde(skip_serializing_if = "is_zero")]
    pub stale_answers: u64,
}

impl WhiteBalanceReport {
    pub fn of(render: &WhiteBalanceDiagnostics, owner: &WhiteStatus, max_hidden_frames: u32) -> Self {
        Self {
            white: WhitePointReport {
                kelvin: owner.held.setting.temperature.kelvin().round(),
                duv: round4(owner.held.setting.tint.duv()),
            },
            origin: origin_name(owner.held.origin),
            applied: render.applied_white.map(WhitePointReport::of_reading),
            request: request_report(&owner.request),
            hal_delta_mired: render.hal_delta.map(|delta| round2(delta.mired)),
            hal_delta_duv: render.hal_delta.map(|delta| round4(delta.duv)),
            cold_start_pending: matches!(render.cold_start, ColdStartState::Pending { .. }),
            hidden_frames: render.hidden_frames_cold_start,
            max_hidden_frames,
            cold_start_forced: render.cold_start_forced,
            statistics_every_n_frames: render.statistics_every_n_frames,
            stale_answers: owner.stale_answers,
        }
    }

    /// Заявка закрыта исходом «принято»: белая взята у источника.
    pub fn request_applied(&self) -> bool {
        self.request.state == "applied"
    }
}

/// Нарушения «белая взята у телефона», общие для сценариев `viewfinder` и `3a` (зеркало — в
/// `scripts/smoke_checks_*.py`): белая должна быть взята у источника «телефон», заявка закрыта, холодный
/// старт кончился. Принудительные показы и предел скрытых кадров судит сценарий `3a` отдельно.
pub fn white_taken_from_phone_violations(report: &WhiteBalanceReport) -> Vec<String> {
    let mut violations = Vec::new();
    let phone = camera_contract::white::WhiteSourceId::PHONE.name();
    if report.origin != phone {
        violations.push(format!(
            "белая не взята у телефона: происхождение «{}», заявка {} ({})",
            report.origin,
            report.request.state,
            report.request.id.map_or_else(|| "-".to_owned(), |id| id.to_string())
        ));
    } else if !report.request_applied() {
        violations.push(format!(
            "заявка белого не закрыта исходом applied: {}",
            report.request.state
        ));
    }
    if report.cold_start_pending {
        violations.push("холодный старт не кончился: кадры всё ещё скрыты".to_owned());
    }
    violations
}

/// Нарушение «кадров скрыто больше предела» (сценарий `viewfinder`: за прогон одно открытие, и сумма скрытых кадров
/// сессии показа не должна превышать предел одного открытия). Сценарий `3a` судит то же по счётчику «с начала».
pub fn hidden_within_limit_violations(report: &WhiteBalanceReport) -> Vec<String> {
    if report.hidden_frames > u64::from(report.max_hidden_frames) {
        return vec![format!(
            "холодный старт скрыл {} кадров, предел {}",
            report.hidden_frames, report.max_hidden_frames
        )];
    }
    Vec::new()
}

fn origin_name(origin: WhiteOrigin) -> String {
    match origin {
        WhiteOrigin::Source(source) => source.name().to_owned(),
        WhiteOrigin::Manual => "manual".to_owned(),
        WhiteOrigin::Fallback => "fallback".to_owned(),
    }
}

fn request_report(state: &WhiteRequestState) -> WhiteRequestReport {
    match state {
        WhiteRequestState::Idle { last: None } => WhiteRequestReport {
            state: "none",
            id: None,
            source: None,
        },
        WhiteRequestState::Idle { last: Some(result) } => WhiteRequestReport {
            state: outcome_kind(&result.result),
            id: Some(result.request.0),
            source: Some(result.source.name()),
        },
        WhiteRequestState::Waiting { request, source } => WhiteRequestReport {
            state: "waiting",
            id: Some(request.0),
            source: Some(source.name()),
        },
    }
}

/// Вид исхода (совпадает с serde-тегом `WhiteRequestOutcome`).
fn outcome_kind(outcome: &WhiteRequestOutcome) -> &'static str {
    match outcome {
        WhiteRequestOutcome::Applied { .. } => "applied",
        WhiteRequestOutcome::Busy => "busy",
        WhiteRequestOutcome::Unknown { .. } => "unknown",
        WhiteRequestOutcome::Unavailable { .. } => "unavailable",
        WhiteRequestOutcome::Failed { .. } => "failed",
        WhiteRequestOutcome::NotUnderstood => "not_understood",
        WhiteRequestOutcome::TimedOut => "timed_out",
        WhiteRequestOutcome::Cancelled { .. } => "cancelled",
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

#[cfg(test)]
impl WhiteBalanceReport {
    /// Здоровый отчёт для тестов вердиктов: белая 3200 К взята у телефона, заявка закрыта исходом «принято»,
    /// холодный старт кончился, статистика считается на каждом 4-м кадре.
    pub(crate) fn taken_from_phone_for_tests() -> Self {
        let point = WhitePointReport {
            kelvin: 3_200.0,
            duv: 0.0,
        };
        Self {
            white: point,
            origin: "phone".to_owned(),
            applied: Some(point),
            request: WhiteRequestReport {
                state: "applied",
                id: Some(1),
                source: Some("phone"),
            },
            hal_delta_mired: Some(5.0),
            hal_delta_duv: Some(0.001),
            cold_start_pending: false,
            hidden_frames: 3,
            max_hidden_frames: 20,
            cold_start_forced: 0,
            statistics_every_n_frames: 4,
            stale_answers: 0,
        }
    }
}

#[cfg(test)]
#[path = "white_balance_report_tests.rs"]
mod tests;
