//! «Снимок без задержки затвора» в числах: какой кадр взял рендер и сколько прошло от нажатия до начала его
//! экспозиции. Чистые функции над выбором кадра ([`FrameChoice`]) и мостом часов, без экрана и рендера.
//!
//! # Часы
//! Нажатие и показ кадра — `MONOTONIC`, метка сенсора — `BOOTTIME`. Сравнивать их напрямую нельзя: перевод идёт
//! через измеренное смещение ([`ClockOffset`]). Нет смещения — задержка НЕ выдумывается: поле `null` и причина рядом.
//!
//! # Что значит «задержка»
//! `press_to_frame_ms` = нажатие − начало экспозиции выбранного кадра (`SENSOR_TIMESTAMP` — начало выдержки).
//! Отрицательное значение — выбранный кадр снят ПОСЛЕ нажатия (замена на ближайший по оценке): задержки нет вовсе.

use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use still_contract::frame_choice::{FrameChoice, SubstitutionReason};

use crate::clock_offset::ClockOffset;

/// Мост часов на момент наблюдения.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClockBridge {
    Available(ClockOffset),
    /// Смещение прочитать не удалось: причина идёт в отчёт.
    Unavailable {
        reason: String,
    },
}

/// Как выбран кадр, в виде для отчёта.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FrameChoiceView {
    /// Ровно показанный кадр.
    Exact,
    /// Показанного взять нельзя; причина — из рендера.
    Substituted { reason: SubstitutionReason },
    /// Выбор неизвестен: снимок не дошёл до захвата, журнал его вытеснил или служба не назвала номер снимка.
    Unavailable { why: &'static str },
}

/// Доказательство задержки одного снимка.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrameTiming {
    pub frame_choice: FrameChoiceView,
    /// Нажатие − начало экспозиции выбранного кадра, мс (0,1 мс). `None` — см. `press_to_frame_unavailable`.
    pub press_to_frame_ms: Option<f64>,
    /// Почему задержку не посчитали.
    pub press_to_frame_unavailable: Option<String>,
    /// Нажатие − момент показа кадра на экране, мс. Только для точного выбора (у замены момента показа нет).
    pub shown_before_press_ms: Option<f64>,
}

/// Миллисекунды из наносекунд, округление до 0,1 мс (дальше шума часов нет смысла).
fn millis_from_nanos(nanos: i64) -> f64 {
    ((nanos as f64) / 100_000.0).round() / 10.0
}

impl FrameTiming {
    /// Задержка снимка. `choice` — запись журнала выбора (`None` — нет), `unknown_why` — почему её нет.
    pub fn of(
        choice: Option<FrameChoice>,
        unknown_why: &'static str,
        pressed_at: MonotonicNanos,
        clock: &ClockBridge,
    ) -> Self {
        let Some(choice) = choice else {
            return Self {
                frame_choice: FrameChoiceView::Unavailable { why: unknown_why },
                press_to_frame_ms: None,
                press_to_frame_unavailable: Some(format!("выбор кадра неизвестен: {unknown_why}")),
                shown_before_press_ms: None,
            };
        };
        let (view, shown_before_press_ms) = match choice {
            FrameChoice::ShownAtPress { shown_at, .. } => (
                FrameChoiceView::Exact,
                Some(millis_from_nanos(
                    pressed_at.as_nanos().saturating_sub(shown_at.as_nanos()),
                )),
            ),
            FrameChoice::Substituted { reason, .. } => (FrameChoiceView::Substituted { reason }, None),
        };
        let (press_to_frame_ms, press_to_frame_unavailable) = match clock {
            ClockBridge::Available(offset) => {
                let pressed_boottime = offset.monotonic_to_boottime(pressed_at);
                let delay = pressed_boottime
                    .as_nanos()
                    .saturating_sub(choice.sensor_timestamp().as_nanos());
                (Some(millis_from_nanos(delay)), None)
            }
            ClockBridge::Unavailable { reason } => (None, Some(format!("нет смещения часов: {reason}"))),
        };
        Self {
            frame_choice: view,
            press_to_frame_ms,
            press_to_frame_unavailable,
            shown_before_press_ms,
        }
    }

    /// Взят ровно показанный кадр.
    pub fn is_exact(&self) -> bool {
        self.frame_choice == FrameChoiceView::Exact
    }

    /// Выбор известен (точный или замена).
    pub fn choice_known(&self) -> bool {
        !matches!(self.frame_choice, FrameChoiceView::Unavailable { .. })
    }

    /// Выбран кадр, снятый из-за вытеснения показанного из кольца (проверка глубины кольца, решение 139).
    pub fn is_evicted_substitution(&self) -> bool {
        matches!(
            self.frame_choice,
            FrameChoiceView::Substituted {
                reason: SubstitutionReason::ShownFrameEvicted
            }
        )
    }
}

/// Сводка по снимкам прогона.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrameSummary {
    /// Снимки, у которых выбор известен / неизвестен.
    pub choices_known: u32,
    pub choices_unknown: u32,
    pub exact: u32,
    pub substituted: u32,
    /// Из замен — из-за вытеснения показанного кадра из кольца.
    pub substituted_evicted: u32,
    /// Доля точных среди известных; `None` — известных нет.
    pub exact_share: Option<f64>,
    /// Снимки, у которых задержку посчитали.
    pub delays_measured: u32,
    pub max_press_to_frame_ms: Option<f64>,
    pub p50_press_to_frame_ms: Option<f64>,
    /// Первая причина, по которой задержку не посчитали (одна на прогон: причины совпадают).
    pub delay_unavailable_reason: Option<String>,
}

impl FrameSummary {
    pub fn of<'a>(timings: impl IntoIterator<Item = &'a FrameTiming>) -> Self {
        let timings: Vec<&FrameTiming> = timings.into_iter().collect();
        let count = |wanted: fn(&FrameTiming) -> bool| -> u32 {
            u32::try_from(timings.iter().filter(|timing| wanted(timing)).count()).unwrap_or(u32::MAX)
        };
        let choices_known = count(FrameTiming::choice_known);
        let exact = count(FrameTiming::is_exact);
        let mut delays: Vec<f64> = timings.iter().filter_map(|timing| timing.press_to_frame_ms).collect();
        delays.sort_by(f64::total_cmp);
        Self {
            choices_known,
            choices_unknown: count(|timing| !timing.choice_known()),
            exact,
            substituted: choices_known.saturating_sub(exact),
            substituted_evicted: count(FrameTiming::is_evicted_substitution),
            exact_share: (choices_known > 0).then(|| f64::from(exact) / f64::from(choices_known)),
            delays_measured: u32::try_from(delays.len()).unwrap_or(u32::MAX),
            max_press_to_frame_ms: delays.last().copied(),
            // Нижняя медиана (индекс (n-1)/2): для чётного числа берётся меньшая из двух средних.
            p50_press_to_frame_ms: delays.get(delays.len().saturating_sub(1) / 2).copied(),
            delay_unavailable_reason: timings
                .iter()
                .find_map(|timing| timing.press_to_frame_unavailable.clone()),
        }
    }
}

impl FrameSummary {
    /// Короткий вид для строки результата (она ≤ 4000 байт): только числа, по которым судит перепроверка ПК.
    /// Полный вид — в `detail_capture`.
    pub fn compact(&self) -> serde_json::Value {
        serde_json::json!({
            "exact": self.exact,
            "subst": self.substituted,
            "evicted": self.substituted_evicted,
            "unknown": self.choices_unknown,
            "share": self.exact_share.map(|share| (share * 1000.0).round() / 1000.0),
            "p2f_max_ms": self.max_press_to_frame_ms,
            "p2f_p50_ms": self.p50_press_to_frame_ms,
        })
    }
}

#[cfg(test)]
#[path = "capture_frame_timing_tests.rs"]
mod tests;
