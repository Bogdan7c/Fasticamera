//! Серия быстрых нажатий сценария `capture`: нажатия по расписанию, привязка принятых запросов к нажатиям, ожидание
//! итогов и учёт «нажатий = принято + отказ сервиса + отказ экрана».
//!
//! # Учёт серии (его судит вердикт)
//! * нажатия экрана = попытки постановки (`submitted`) + отказы самого экрана (затвор занят до «кадр захвачен»,
//!   набор сохраняемых полон — редьюсер даже не просит снимок);
//! * попытки постановки = принятые службой + отвергнутые сервисом;
//! * принятые = сохранённые + сорванные + недождавшиеся.
//!
//! Метка нажатия — время последнего показанного кадра рендера на момент нажатия (так служба выбирает кадр,
//! «будущих» меток нет). По этой метке принятый запрос из журнала привязывается к своему нажатию.

use std::time::{Duration, Instant};

use camera_screen::ScreenAction;
use camera_screen::shutter::ShotRequest;
use serde::Serialize;
use serde_json::{Value, json};
use shell_contract::clock::MonotonicNanos;

use super::capture_observation::{CaptureObservation, LiveCounters, LiveDelta, PhaseBook, SubmitCounts, shot_record};
use super::capture_report::{ShotOutcome, ShotRecord};
use super::detail::to_value;
use crate::capture_scenario_config::BurstPlan;

/// Одно нажатие серии.
#[derive(Debug, Clone, Copy)]
struct PressMark {
    /// Метка нажатия (часы рендера).
    at: MonotonicNanos,
    instant: Instant,
    /// Номер запроса экрана, если нажатие дошло до службы.
    request: Option<u64>,
}

/// Значения счётчиков на момент начала серии.
#[derive(Debug, Clone, Copy)]
pub struct BurstBaselines {
    pub submit: SubmitCounts,
    pub actions_rejected: u64,
    pub live: LiveCounters,
    /// Наибольший номер запроса экрана, уже бывший в журнале: всё новее принадлежит серии.
    pub last_request: u64,
}

/// Как идёт серия.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurstProgress {
    Running,
    /// Все принятые снимки получили итог.
    Settled,
    /// Итоги не пришли за `drain_timeout_ms` после последнего нажатия.
    DrainTimedOut,
}

/// Итог серии в отчёте.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BurstRecord {
    pub presses: u32,
    /// Сколько раз экран дошёл до сервиса снимка.
    pub submitted: u64,
    /// Из них принято службой.
    pub accepted_by_service: u64,
    /// Из них отвергнуто сервисом (занят или недоступен).
    pub rejected_by_service: u64,
    /// Нажатия, до сервиса не дошедшие: экран отказал сам (затвор занят, набор сохраняемых полон).
    pub refused_by_screen: u64,
    /// Принятые запросы, которые не нашлись в журнале запросов (должно быть 0).
    pub accepted_missing_from_journal: u64,
    pub saved: u32,
    pub failed: u32,
    pub unfinished: u32,
    /// Отказы редьюсера экрана за серию (любой причины; в серии это отказы нажатий).
    pub screen_actions_rejected: u64,
    /// Фактические интервалы между нажатиями, мс (расписание кратно тику маршрутизатора).
    pub min_press_gap_ms: Option<u64>,
    pub max_press_gap_ms: Option<u64>,
    /// Приращения счётчиков видоискателя за серию (от первого нажатия до итогов).
    pub viewfinder: LiveDelta,
}

impl BurstRecord {
    /// Для сводки: все числа, без вложенностей.
    pub fn summary(&self) -> Value {
        json!({
            "presses": self.presses,
            "submitted": self.submitted,
            "accepted": self.accepted_by_service,
            "rejected_by_service": self.rejected_by_service,
            "refused_by_screen": self.refused_by_screen,
            "saved": self.saved,
            "failed": self.failed,
            "unfinished": self.unfinished,
            "press_gap_ms": [self.min_press_gap_ms, self.max_press_gap_ms],
            "viewfinder": to_value(&self.viewfinder),
        })
    }
}

/// Автомат серии.
pub struct BurstState {
    plan: BurstPlan,
    /// Наименьшее ожидание итогов после последнего нажатия: ответы экрана и службы приходят с задержкой кадров.
    min_drain: Duration,
    baselines: BurstBaselines,
    marks: Vec<PressMark>,
    next_press_at: Instant,
    started: Instant,
}

impl BurstState {
    pub fn new(plan: BurstPlan, min_drain: Duration, baselines: BurstBaselines, now: Instant) -> Self {
        Self {
            plan,
            min_drain,
            baselines,
            marks: Vec::new(),
            next_press_at: now,
            started: now,
        }
    }

    fn all_pressed(&self) -> bool {
        self.marks.len() >= self.plan.presses as usize
    }

    /// Привязывает новые запросы журнала к нажаткам по метке нажатия.
    fn bind_requests(&mut self, journal: &[ShotRequest]) {
        for request in journal
            .iter()
            .filter(|request| request.request.0 > self.baselines.last_request)
        {
            let already_bound = self.marks.iter().any(|mark| mark.request == Some(request.request.0));
            if already_bound {
                continue;
            }
            if let Some(mark) = self
                .marks
                .iter_mut()
                .find(|mark| mark.request.is_none() && mark.at == request.pressed_at)
            {
                mark.request = Some(request.request.0);
            }
        }
    }

    /// Запросы серии из журнала (по возрастанию номера).
    fn requests_of_run<'a>(&self, journal: &'a [ShotRequest]) -> Vec<&'a ShotRequest> {
        journal
            .iter()
            .filter(|request| self.marks.iter().any(|mark| mark.request == Some(request.request.0)))
            .collect()
    }

    /// Один тик: нажать, если пора, и проверить, всё ли получило итог.
    pub fn tick(&mut self, obs: &CaptureObservation<'_>, book: &PhaseBook) -> (Vec<ScreenAction>, BurstProgress) {
        let mut injections = Vec::new();
        if !self.all_pressed() && obs.now >= self.next_press_at {
            injections.push(ScreenAction::ShutterTapped { at: obs.render_now });
            self.marks.push(PressMark {
                at: obs.render_now,
                instant: obs.now,
                request: None,
            });
            self.next_press_at = obs.now + Duration::from_millis(self.plan.interval_ms);
        }
        self.bind_requests(obs.journal);
        if !self.all_pressed() {
            return (injections, BurstProgress::Running);
        }
        let last_press = self.marks.last().map_or(self.started, |mark| mark.instant);
        let waited = obs.now.saturating_duration_since(last_press);
        let accepted = obs.submit.delta_since(self.baselines.submit).accepted;
        let bound: Vec<u64> = self.marks.iter().filter_map(|mark| mark.request).collect();
        let every_accepted_is_bound = u64::try_from(bound.len()).is_ok_and(|count| count == accepted);
        let every_bound_has_final = bound.iter().all(|request| book.terminal_of(*request).is_some());
        if waited >= self.min_drain && every_accepted_is_bound && every_bound_has_final {
            return (injections, BurstProgress::Settled);
        }
        if waited >= Duration::from_millis(self.plan.drain_timeout_ms) {
            return (injections, BurstProgress::DrainTimedOut);
        }
        (injections, BurstProgress::Running)
    }

    /// Итог серии: учёт и записи снимков (номера — с `first_number`).
    pub fn finish(
        self,
        obs: &CaptureObservation<'_>,
        book: &PhaseBook,
        step: &'static str,
        first_number: u32,
    ) -> (BurstRecord, Vec<ShotRecord>) {
        let submit = obs.submit.delta_since(self.baselines.submit);
        let requests = self.requests_of_run(obs.journal);
        let mut shots = Vec::new();
        for (offset, request) in requests.iter().enumerate() {
            let pressed_at = self
                .marks
                .iter()
                .find(|mark| mark.request == Some(request.request.0))
                .map_or(self.started, |mark| mark.instant);
            let number = first_number.saturating_add(u32::try_from(offset).unwrap_or(u32::MAX));
            // Серия снимает без RAW: экран обязан был вернуть настройку после шага «HEIF + DNG».
            shots.push(shot_record(number, step, false, request, pressed_at, book, &obs.clock));
        }
        let count = |wanted: fn(&ShotOutcome) -> bool| -> u32 {
            u32::try_from(shots.iter().filter(|shot| wanted(&shot.outcome)).count()).unwrap_or(u32::MAX)
        };
        let gaps: Vec<u64> = self
            .marks
            .windows(2)
            .map(|pair| {
                u64::try_from(pair[1].instant.saturating_duration_since(pair[0].instant).as_millis())
                    .unwrap_or(u64::MAX)
            })
            .collect();
        let presses = u32::try_from(self.marks.len()).unwrap_or(u32::MAX);
        let record = BurstRecord {
            presses,
            submitted: submit.submitted(),
            accepted_by_service: submit.accepted,
            rejected_by_service: submit.rejected,
            refused_by_screen: u64::from(presses).saturating_sub(submit.submitted()),
            accepted_missing_from_journal: submit.accepted.saturating_sub(u64::try_from(shots.len()).unwrap_or(0)),
            saved: count(|outcome| matches!(outcome, ShotOutcome::Saved { .. })),
            failed: count(|outcome| matches!(outcome, ShotOutcome::Failed { .. })),
            unfinished: count(|outcome| matches!(outcome, ShotOutcome::Unfinished { .. })),
            screen_actions_rejected: obs.actions_rejected.saturating_sub(self.baselines.actions_rejected),
            min_press_gap_ms: gaps.iter().copied().min(),
            max_press_gap_ms: gaps.iter().copied().max(),
            viewfinder: obs.live.delta_since(self.baselines.live),
        };
        (record, shots)
    }
}

#[cfg(test)]
#[path = "capture_burst_tests.rs"]
mod tests;
