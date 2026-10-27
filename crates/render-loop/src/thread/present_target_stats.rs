//! Учёт выбора линии кадра и замеры, по которым A/B на телефоне сравнивается с сегодняшними
//! числами (сессия 07): какие линии выбирались, сколько раз был откат, на сколько vsync кадр
//! реально опоздал относительно задуманного, сколько поток стоит в `acquire_frame` и сколько
//! стоит кадр от захвата камеры до сабмита.
//!
//! Владелец — поток рендера: [`PresentTargetTracker`] держит планировщик и счётчики; наружу —
//! только снимок [`PresentTargetDiagnostics`] в `RenderStatus` (обновляется при опросе
//! диагностики, перцентили на каждом кадре не считаются).

use std::collections::VecDeque;
use std::time::Duration;

use shell_contract::clock::MonotonicNanos;
use shell_contract::vsync::{FrameTimelines, MAX_FRAME_TIMELINES};

use super::present_depth::DepthOutcome;
use super::present_target::{FrameCostEstimate, PresentTargetPlanner, TickPlan};
use super::present_target_control::{PresentTargetCommand, PresentTargetCommandOutcome, PresentTargetSelection};
use super::rolling_durations::{DurationSummary, RollingDurations};
use crate::backend::{FrameCpuTiming, FrameGpuTiming, LatchWaitResult, PastPresentation, PresentTarget, TargetChoice};
use crate::config::{LateLatchMode, PresentTargetConfig};

/// Корзин опоздания: 0 (на задуманном vsync), 1, 2 и «3 и больше».
pub const VSYNC_SHIFT_BUCKETS: usize = 4;

/// На сколько vsync кадр показан позже задуманного (`actual − expected` выбранной линии,
/// округлено до целых периодов). Это то же, что `actual − desired` в целых периодах за вычетом
/// упреждения `desiredPresentTime`: желаемое время стоит на долю периода раньше целевого vsync.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct VsyncShiftHistogram {
    /// `late_by_periods[k]` — кадров, показанных на k vsync позже задуманного; последняя корзина — k и больше.
    pub late_by_periods: [u64; VSYNC_SHIFT_BUCKETS],
    /// Показаны раньше задуманного vsync (не должно случаться; ненулевое — сбой меток).
    pub early: u64,
    /// Записи о показе, для которых поток не знает задуманной линии (вытеснены, чужие).
    pub unmatched: u64,
}

/// Снимок учёта выбора линии кадра (часть `RenderStatus`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresentTargetDiagnostics {
    /// Действующие политика и поздний захват (из конфига, дальше — командами).
    pub selection: PresentTargetSelection,
    /// Сколько кадров спланировано (отдано бэкенду). Как и остальные счётчики и окна снимка,
    /// обнуляется применённой командой выбора линии (новая фаза замера).
    pub frames_planned: u64,
    /// `chosen_timeline[i]` — сколько раз выбрана линия с индексом i в списке тика.
    pub chosen_timeline: [u64; MAX_FRAME_TIMELINES],
    /// Сколько раз выбрана линия раньше предпочитаемой платформой.
    pub chosen_earlier_than_preferred: u64,
    /// Сколько раз выбрана линия позже предпочитаемой.
    pub chosen_later_than_preferred: u64,
    /// `NearestFeasible` не нашла выполнимой линии (или часы не прочитались) — взята предпочитаемая.
    pub fallbacks_to_preferred: u64,
    /// Момент позднего захвата уже прошёл к моменту планирования (кадр заведомо «в обрез»).
    pub latch_already_passed: u64,
    /// Кадры, у которых предел ожидания позднего захвата урезан периодом vsync тика (конфиг
    /// `latch_max_wait_us` длиннее периода без окна активного ожидания). Только при включённом
    /// позднем захвате.
    pub latch_wait_capped_to_period: u64,
    /// Бэкенд отказался ждать момент захвата: срок дальше предела ожидания (камера опрошена сразу).
    pub latch_waits_refused: u64,
    /// Тики, в которых платформа прислала больше линий, чем вмещает тик.
    pub ticks_with_dropped_timelines: u64,
    /// Сколько линий всего отброшено по ёмкости.
    pub dropped_timelines: u64,
    /// Сколько раз не прочитались монотонные часы при планировании.
    pub clock_read_failures: u64,
    /// Тики, пропущенные ради схода на ближнюю линию (решение 54): кадр на экране повторён.
    pub skipped_ticks_for_depth: u64,
    /// Сходы, после которых кадр действительно выбрал линию ближе (глубина очереди уменьшилась).
    pub depth_reductions: u64,
    /// Пропуски, после которых глубина не уменьшилась (оценка стоимости выросла за тик).
    pub depth_skips_without_reduction: u64,
    /// Опоздание показа относительно выбранной линии.
    pub vsync_shift: VsyncShiftHistogram,
    /// Время в `acquire_frame` по окну `stats_window_frames` (`None` — замеров нет).
    pub acquire_wait: Option<DurationSummary>,
    /// Ожидание момента позднего захвата после acquire (нули, пока `late_latch` выключен).
    pub latch_wait: Option<DurationSummary>,
    /// CPU от захвата кадра камеры до сабмита по окну `stats_window_frames`.
    pub latch_to_submit: Option<DurationSummary>,
    /// Текущая оценка стоимости кадра целиком: CPU + GPU (по ней выбирается линия и `latch_at`).
    pub estimated_frame_cost: Duration,
    /// Та же оценка по частям (CPU, GPU и откуда взят GPU).
    pub estimated_cost_parts: FrameCostEstimate,
    /// Время кадра на GPU по окну `stats_window_frames` (`None` — бэкенд GPU не меряет или замеров
    /// ещё нет; приходит на 1–3 кадра позже кадра).
    pub frame_gpu_time: Option<DurationSummary>,
}

/// Учёт и планировщик линии кадра потока рендера.
#[derive(Debug)]
pub(super) struct PresentTargetTracker {
    planner: PresentTargetPlanner,
    acquire_waits: RollingDurations,
    latch_waits: RollingDurations,
    latch_costs: RollingDurations,
    gpu_times: RollingDurations,
    /// Задуманные линии показанных кадров: `(frame_index, expected_present)` по возрастанию
    /// номера; ждут записи о реальном показе.
    awaiting_presentation: VecDeque<(u64, MonotonicNanos)>,
    awaiting_capacity: usize,
    /// Записи о показе кадров с номером не больше этого спланированы при прежней политике:
    /// после команды они в новую гистограмму не идут (`None` — команд не было).
    ignore_presentations_up_to: Option<u64>,
    /// Номер последнего показанного кадра (граница для `ignore_presentations_up_to`).
    last_presented_frame: Option<u64>,
    /// Времена GPU кадров с номером не больше этого — от прежней фазы (`None` — команд не было).
    ignore_gpu_timings_up_to: Option<u64>,
    /// Номер последнего спланированного кадра (граница для `ignore_gpu_timings_up_to`).
    last_planned_frame: Option<u64>,
    counters: PresentTargetDiagnostics,
}

impl PresentTargetTracker {
    /// `awaiting_capacity` — сколько задуманных линий ждать записей о показе (как история показов).
    pub(super) fn new(config: &PresentTargetConfig, awaiting_capacity: usize) -> Self {
        Self {
            planner: PresentTargetPlanner::new(config.clone()),
            acquire_waits: RollingDurations::new(config.stats_window_frames),
            latch_waits: RollingDurations::new(config.stats_window_frames),
            latch_costs: RollingDurations::new(config.stats_window_frames),
            gpu_times: RollingDurations::new(config.stats_window_frames),
            awaiting_presentation: VecDeque::with_capacity(awaiting_capacity.min(1024)),
            awaiting_capacity,
            ignore_presentations_up_to: None,
            last_presented_frame: None,
            ignore_gpu_timings_up_to: None,
            last_planned_frame: None,
            counters: PresentTargetDiagnostics {
                selection: PresentTargetSelection::of_config(config),
                ..PresentTargetDiagnostics::default()
            },
        }
    }

    /// Применяет команду выбору линии кадра (см. `present_target_control`): проверка тем же правилом,
    /// что конфиг; применённая команда начинает новую фазу — окна оценки и диагностики, счётчики
    /// выбора и гистограмма опоздания обнуляются, записи о показах прежней фазы отбрасываются.
    pub(super) fn apply_command(&mut self, command: PresentTargetCommand) -> PresentTargetCommandOutcome {
        let current = self.planner.config();
        let updated = match command.applied_to(current) {
            Ok(updated) => updated,
            Err(error) => {
                return PresentTargetCommandOutcome::Rejected {
                    reason: error.to_string(),
                };
            }
        };
        if updated == *current {
            return PresentTargetCommandOutcome::AlreadyInEffect;
        }
        let selection = PresentTargetSelection::of_config(&updated);
        let window = updated.stats_window_frames;
        self.planner.reconfigure(updated);
        self.acquire_waits = RollingDurations::new(window);
        self.latch_waits = RollingDurations::new(window);
        self.latch_costs = RollingDurations::new(window);
        self.gpu_times = RollingDurations::new(window);
        self.awaiting_presentation.clear();
        self.ignore_presentations_up_to = self.last_presented_frame;
        self.ignore_gpu_timings_up_to = self.last_planned_frame;
        self.counters = PresentTargetDiagnostics {
            selection,
            ..PresentTargetDiagnostics::default()
        };
        PresentTargetCommandOutcome::Applied
    }

    /// Планирует тик: кадр `frame_index` на выбранную линию или пропуск ради схода на ближнюю
    /// линию (решение 54). Номер кадра при пропуске не расходуется — кадра нет.
    pub(super) fn plan_frame(
        &mut self,
        frame_index: u64,
        timelines: &FrameTimelines,
        now: Option<MonotonicNanos>,
        refresh_period: Duration,
    ) -> TickPlan {
        if now.is_none() {
            self.counters.clock_read_failures += 1;
        }
        let dropped = timelines.dropped_count();
        if dropped > 0 {
            self.counters.ticks_with_dropped_timelines += 1;
            self.counters.dropped_timelines += u64::try_from(dropped).unwrap_or(u64::MAX);
        }
        let plan = self.planner.plan_tick(timelines, now, refresh_period);
        match plan {
            TickPlan::SkipForDepthReduction => self.counters.skipped_ticks_for_depth += 1,
            TickPlan::Present { target, depth_outcome } => {
                self.note_planned(frame_index);
                self.count_choice(&target, now);
                self.count_latch_budget(&target);
                match depth_outcome {
                    Some(DepthOutcome::Reduced) => self.counters.depth_reductions += 1,
                    Some(DepthOutcome::NotReduced) => self.counters.depth_skips_without_reduction += 1,
                    None => {}
                }
            }
        }
        plan
    }

    /// Учитывает урезание предела ожидания периодом (только при включённом позднем захвате).
    fn count_latch_budget(&mut self, target: &PresentTarget) {
        let config = self.planner.config();
        let configured = Duration::from_micros(config.latch_max_wait_us);
        if config.late_latch == LateLatchMode::BeforeCameraPoll && target.latch.max_wait < configured {
            self.counters.latch_wait_capped_to_period += 1;
        }
    }

    fn count_choice(&mut self, target: &PresentTarget, now: Option<MonotonicNanos>) {
        let counters = &mut self.counters;
        counters.frames_planned += 1;
        if let Some(bucket) = counters.chosen_timeline.get_mut(target.timeline_index) {
            *bucket += 1;
        }
        if target.timeline_index < target.preferred_index {
            counters.chosen_earlier_than_preferred += 1;
        } else if target.timeline_index > target.preferred_index {
            counters.chosen_later_than_preferred += 1;
        }
        if target.choice == TargetChoice::FallbackToPreferred {
            counters.fallbacks_to_preferred += 1;
        }
        if now.is_some_and(|now| target.latch.latch_at <= now) {
            counters.latch_already_passed += 1;
        }
    }

    /// Учитывает замер CPU кадра от бэкенда: окно диагностики и оценка стоимости кадра.
    pub(super) fn record_cpu_timing(&mut self, timing: FrameCpuTiming) {
        self.acquire_waits.push(timing.acquire_wait);
        self.latch_waits.push(timing.latch_wait);
        if timing.latch_result == LatchWaitResult::RefusedBeyondBudget {
            self.counters.latch_waits_refused += 1;
        }
        if let Some(cost) = timing.latch_to_submit {
            self.latch_costs.push(cost);
            self.planner.record_frame_cost(cost);
        }
    }

    /// Учитывает время кадра на GPU (приходит позже кадра): окно диагностики и оценка стоимости.
    /// Замеры кадров, спланированных до последней команды, в новую фазу не идут.
    pub(super) fn record_gpu_timing(&mut self, timing: FrameGpuTiming) {
        if self
            .ignore_gpu_timings_up_to
            .is_some_and(|last_of_previous_phase| timing.frame_index <= last_of_previous_phase)
        {
            return;
        }
        self.gpu_times.push(timing.gpu_duration);
        self.planner.record_gpu_cost(timing.gpu_duration);
    }

    /// Запоминает номер последнего спланированного кадра (граница фаз для замеров GPU).
    fn note_planned(&mut self, frame_index: u64) {
        self.last_planned_frame = Some(frame_index);
    }

    /// Запоминает задуманную линию показанного кадра (ждёт записи о реальном показе).
    pub(super) fn record_presented(&mut self, frame_index: u64, target: &PresentTarget) {
        self.last_presented_frame = Some(frame_index);
        if self.awaiting_capacity == 0 {
            return;
        }
        if self.awaiting_presentation.len() == self.awaiting_capacity {
            self.awaiting_presentation.pop_front();
        }
        self.awaiting_presentation
            .push_back((frame_index, target.expected_present));
    }

    /// Сопоставляет реальный показ с задуманной линией и пополняет гистограмму опоздания.
    pub(super) fn record_presentation(&mut self, presentation: &PastPresentation, refresh_period: Duration) {
        if self
            .ignore_presentations_up_to
            .is_some_and(|last_of_previous_phase| presentation.frame_index <= last_of_previous_phase)
        {
            // Кадр спланирован прежней политикой: в замер новой фазы не идёт.
            return;
        }
        // Записи приходят по возрастанию номера кадра: всё, что старше, уже не придёт
        // (запись потеряна драйвером) — такие кадры учитывает пейсинг как дыры.
        while self
            .awaiting_presentation
            .front()
            .is_some_and(|&(frame_index, _)| frame_index < presentation.frame_index)
        {
            self.awaiting_presentation.pop_front();
        }
        let shift = &mut self.counters.vsync_shift;
        match self.awaiting_presentation.front() {
            Some(&(frame_index, expected)) if frame_index == presentation.frame_index => {
                self.awaiting_presentation.pop_front();
                match shift_in_periods(expected, presentation.actual_present, refresh_period) {
                    Some(periods) => {
                        let bucket = usize::try_from(periods)
                            .unwrap_or(usize::MAX)
                            .min(VSYNC_SHIFT_BUCKETS - 1);
                        shift.late_by_periods[bucket] += 1;
                    }
                    None => shift.early += 1,
                }
            }
            _ => shift.unmatched += 1,
        }
    }

    /// Снимок для статуса (перцентили окон считаются здесь, не на каждом кадре).
    pub(super) fn snapshot(&mut self) -> PresentTargetDiagnostics {
        let cost = self.planner.estimated_frame_cost();
        PresentTargetDiagnostics {
            acquire_wait: self.acquire_waits.summary(),
            latch_wait: self.latch_waits.summary(),
            latch_to_submit: self.latch_costs.summary(),
            frame_gpu_time: self.gpu_times.summary(),
            estimated_frame_cost: cost.total(),
            estimated_cost_parts: cost,
            ..self.counters.clone()
        }
    }
}

/// Опоздание показа в целых периодах (округление к ближайшему: метки дрожат на доли мс);
/// `None` — показан раньше задуманного vsync.
fn shift_in_periods(expected: MonotonicNanos, actual: MonotonicNanos, refresh_period: Duration) -> Option<u64> {
    let period_nanos = i64::try_from(refresh_period.as_nanos()).unwrap_or(i64::MAX).max(1);
    let difference = actual.as_nanos().saturating_sub(expected.as_nanos());
    // Округление к ближайшему целому для любых знаков: floor((d + p/2) / p).
    let periods = difference.saturating_add(period_nanos / 2).div_euclid(period_nanos);
    u64::try_from(periods).ok()
}

#[cfg(test)]
#[path = "present_target_stats_tests.rs"]
mod tests;
