//! Выбор линии кадра и момента позднего захвата (сессия 07).
//!
//! Владелец: поток рендера (через `present_target_stats::PresentTargetTracker`). Вход — линии
//! кадра тика vsync, «сейчас» по `CLOCK_MONOTONIC` и оценка стоимости кадра; выход —
//! [`PresentTarget`] в `FrameTiming`, его читает бэкенд.
//!
//! # Оценка стоимости кадра (CPU + GPU, P13 сессии 07)
//! Стоимость = p95 CPU от захвата кадра камеры до возврата из сабмита (замер бэкенда,
//! `FrameCpuTiming::latch_to_submit`) + p95 времени кадра на GPU (метки `frame_total` видоискателя,
//! `FrameGpuTiming`). Кадр должен не только уйти в очередь до срока линии, но и ДОИСПОЛНИТЬСЯ на
//! GPU: SurfaceFlinger берёт буфер, когда его fence сигнален. На OnePlus 11 GPU под DVFS
//! растягивает кадр до ~13 мс — без этого слагаемого поздний захват метил бы слишком поздно.
//!
//! p95 — по скользящим окнам: медиана опаздывала бы в каждом двадцатом кадре чаще, максимум
//! слишком пуглив к одиночным выбросам. Сумма двух p95 консервативнее p95 суммы — сознательно:
//! промах срока стоит целого кадра, а лишний запас — лишь чуть большей задержки.
//!
//! Время GPU приходит с опозданием на 1–3 кадра (метки слота читаются только после его fence,
//! без ожиданий GPU), поэтому оценка всегда опирается на кадры чуть старше текущего. Пока CPU-
//! замеров меньше `min_cost_samples` — начальная оценка CPU; пока GPU-замеров меньше того же
//! порога (бэкенд не меряет GPU: тестовый паттерн, fake) — консервативная оценка GPU из конфига
//! (`gpu_cost_fallback_us`). Запас (`margin`) покрывает выбросы сверх p95.
//!
//! # Правило выбора (`NearestFeasible`)
//! Выполнима линия, у которой `deadline − сейчас ≥ стоимость + запас` и которая строго позже
//! (больше чем на полпериода) линии прошлого кадра: иначе два кадра подряд метили бы на один
//! vsync, и SurfaceFlinger выбросил бы один из них. Из выполнимых берётся самая ранняя по
//! `expected_present`. Нет выполнимых — предпочитаемая платформой (счётчик отката).
//!
//! # Сход на ближнюю линию (решение 54)
//! Запрет «строго позже прошлой линии» не даёт глубине очереди уменьшаться. Когда ближняя линия
//! устойчиво выполнима, [`PresentTargetPlanner::plan_tick`] один раз пропускает тик (без рисования
//! и опроса камеры) — правила и цена в `present_depth`.
//!
//! # Период vsync тика
//! Период пейсинга сглажен гистерезисом и сразу после смены частоты (60 → 120 Гц) ещё старый.
//! Линии самого тика уже идут с новым шагом, поэтому для решений тика берётся меньшее из
//! периода пейсинга и шага между линиями тика ([`tick_vsync_period`]): со старым периодом
//! «полпериода» защиты от двух кадров на один vsync накрыл бы настоящий следующий vsync.
//!
//! # Предел ожидания позднего захвата
//! Ожидание идёт ПОСЛЕ acquire, с картинкой swapchain на руках. Ждать дольше периода нельзя:
//! поток проспал бы следующий vsync. Поэтому `max_wait = min(latch_max_wait_us, период тика −
//! окно активного ожидания)`; срок дальше предела — отказ ждать (камера опрашивается сразу,
//! счётчик `latch_waits_refused`).

use std::time::Duration;

use shell_contract::clock::MonotonicNanos;
use shell_contract::vsync::FrameTimelines;

use super::present_depth::{DepthOutcome, DepthReducer, DepthStep, PlannedDepth};
use super::rolling_durations::RollingDurations;
use crate::backend::{FrameLatchPlan, PresentTarget, TargetChoice};
use crate::config::{PresentTargetConfig, PresentTimelinePolicy};

/// Перцентиль оценки стоимости кадра.
const FRAME_COST_QUANTILE: f64 = 0.95;

/// Откуда взято слагаемое GPU в оценке стоимости кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GpuCostSource {
    /// p95 замеров времени кадра на GPU (их не меньше `min_cost_samples`).
    Measured,
    /// Замеров мало или нет: консервативная оценка из конфига (`gpu_cost_fallback_us`).
    #[default]
    ConfiguredFallback,
}

/// Оценка стоимости кадра по частям: от захвата кадра камеры до готовности кадра на GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameCostEstimate {
    /// CPU от захвата до сабмита (p95 окна или начальная оценка).
    pub cpu: Duration,
    /// Исполнение кадра на GPU (p95 окна или оценка из конфига).
    pub gpu: Duration,
    /// Откуда взято `gpu`.
    pub gpu_source: GpuCostSource,
}

impl FrameCostEstimate {
    /// Полная стоимость кадра: CPU + GPU (по ней выбирается линия и считается `latch_at`).
    pub fn total(&self) -> Duration {
        self.cpu.saturating_add(self.gpu)
    }
}

/// План тика vsync.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TickPlan {
    /// Рисовать и показывать кадр на эту линию. `depth_outcome` — итог схода, если это первый
    /// кадр после пропуска тика.
    Present {
        target: PresentTarget,
        depth_outcome: Option<DepthOutcome>,
    },
    /// Тик пропускается ради схода на ближнюю линию: ни рисования, ни показа, ни опроса камеры.
    SkipForDepthReduction,
}

#[cfg(test)]
impl TickPlan {
    /// Линия кадра (тест знает, что тик не пропущен).
    pub(super) fn expect_target(self) -> PresentTarget {
        match self {
            Self::Present { target, .. } => target,
            Self::SkipForDepthReduction => panic!("тик неожиданно пропущен ради схода"),
        }
    }
}

/// Планировщик линии кадра: политика из конфига + скользящая оценка стоимости кадра.
#[derive(Debug)]
pub(super) struct PresentTargetPlanner {
    config: PresentTargetConfig,
    cost_samples: RollingDurations,
    /// Время кадров на GPU (приходит на 1–3 кадра позже CPU-замера того же кадра).
    gpu_cost_samples: RollingDurations,
    /// Момент показа линии, выбранной для прошлого кадра (защита от двух кадров на один vsync).
    last_target_expected: Option<MonotonicNanos>,
    /// Сход на ближнюю линию (решение 54).
    depth: DepthReducer,
    /// Итог схода, вычисленный последним `plan` (забирает `plan_tick`).
    last_depth_outcome: Option<DepthOutcome>,
}

impl PresentTargetPlanner {
    pub(super) fn new(config: PresentTargetConfig) -> Self {
        Self {
            cost_samples: RollingDurations::new(config.cost_window_frames),
            gpu_cost_samples: RollingDurations::new(config.cost_window_frames),
            depth: DepthReducer::new(&config),
            config,
            last_target_expected: None,
            last_depth_outcome: None,
        }
    }

    /// План тика: пропуск ради схода на ближнюю линию или кадр на выбранную линию (см. модуль).
    pub(super) fn plan_tick(
        &mut self,
        timelines: &FrameTimelines,
        now: Option<MonotonicNanos>,
        refresh_period: Duration,
    ) -> TickPlan {
        if self.depth.step_for_tick(now) == DepthStep::SkipTick {
            // Линия прошлого кадра не трогается: именно она позволит следующему тику метить ближе.
            return TickPlan::SkipForDepthReduction;
        }
        let target = self.plan(timelines, now, refresh_period);
        TickPlan::Present {
            target,
            depth_outcome: self.last_depth_outcome.take(),
        }
    }

    /// Действующие настройки (политика и поздний захват могут смениться командой).
    pub(super) fn config(&self) -> &PresentTargetConfig {
        &self.config
    }

    /// Новые настройки (команда выбору линии кадра): окно оценки стоимости начинается заново —
    /// замеры прежней фазы (другой захват, другая нагрузка) новую оценку не двигают.
    ///
    /// Линия прошлого кадра тоже забывается. Иначе защита «строго позже прошлой линии» навсегда
    /// сохранила бы глубину очереди прежней политики: после `Preferred` (метим на 3-й vsync)
    /// каждый следующий кадр снова не мог бы метить раньше, и `NearestFeasible` никогда не
    /// вернулся бы к ближайшей линии — A/B в одном прогоне потерял бы смысл. Цена — один
    /// переходный кадр на стыке фаз (может показаться на vsync позже задуманного или вытеснить
    /// соседа); окна диагностики всё равно обнуляются командой, и переход виден как разовый сдвиг.
    pub(super) fn reconfigure(&mut self, config: PresentTargetConfig) {
        self.cost_samples = RollingDurations::new(config.cost_window_frames);
        self.gpu_cost_samples = RollingDurations::new(config.cost_window_frames);
        self.last_target_expected = None;
        // Новая фаза: серия и пауза схода начинаются заново.
        self.depth = DepthReducer::new(&config);
        self.last_depth_outcome = None;
        self.config = config;
    }

    /// Учитывает замер стоимости кадра (CPU от захвата кадра камеры до сабмита).
    pub(super) fn record_frame_cost(&mut self, cost: Duration) {
        self.cost_samples.push(cost);
    }

    /// Учитывает время кадра на GPU (приходит позже CPU-замера того же кадра, см. модуль).
    pub(super) fn record_gpu_cost(&mut self, gpu: Duration) {
        self.gpu_cost_samples.push(gpu);
    }

    /// Текущая оценка стоимости кадра по частям (см. модуль).
    pub(super) fn estimated_frame_cost(&mut self) -> FrameCostEstimate {
        let min_samples = self.config.min_cost_samples;
        let initial_cpu = Duration::from_micros(self.config.initial_frame_cost_us);
        let cpu = if self.cost_samples.len() < min_samples {
            initial_cpu
        } else {
            self.cost_samples.percentile(FRAME_COST_QUANTILE).unwrap_or(initial_cpu)
        };
        let measured_gpu = if self.gpu_cost_samples.len() < min_samples {
            None
        } else {
            self.gpu_cost_samples.percentile(FRAME_COST_QUANTILE)
        };
        let (gpu, gpu_source) = match measured_gpu {
            Some(gpu) => (gpu, GpuCostSource::Measured),
            None => (
                Duration::from_micros(self.config.gpu_cost_fallback_us),
                GpuCostSource::ConfiguredFallback,
            ),
        };
        FrameCostEstimate { cpu, gpu, gpu_source }
    }

    /// Выбирает линию кадра и момент позднего захвата. `now` = `None` — часы не прочитались:
    /// выбор по сроку невозможен, берётся предпочитаемая линия (как откат).
    pub(super) fn plan(
        &mut self,
        timelines: &FrameTimelines,
        now: Option<MonotonicNanos>,
        refresh_period: Duration,
    ) -> PresentTarget {
        let frame_cost = self.estimated_frame_cost().total();
        let period = tick_vsync_period(timelines, refresh_period);
        let preferred_index = timelines.preferred_index();
        let floor = self.distinct_vsync_floor(period);
        let mut observation = None;
        let (timeline_index, choice, margin) = match self.config.policy {
            PresentTimelinePolicy::Preferred => (
                preferred_index,
                TargetChoice::PlatformPreferred,
                Duration::from_micros(self.config.latch_margin_us),
            ),
            PresentTimelinePolicy::NearestFeasible { margin_us } => {
                let margin = Duration::from_micros(margin_us);
                let rule = now.map(|now| FeasibilityRule {
                    now,
                    required_lead: frame_cost.saturating_add(margin),
                    not_before: floor,
                });
                let feasible = rule.and_then(|rule| nearest_feasible_timeline(timelines, rule));
                match (feasible, rule) {
                    (Some(index), Some(rule)) => {
                        observation = Some(observe_depth(timelines, index, rule));
                        (index, TargetChoice::NearestFeasible, margin)
                    }
                    _ => (preferred_index, TargetChoice::FallbackToPreferred, margin),
                }
            }
        };
        self.last_depth_outcome = self.depth.observe_planned(observation);
        // Индекс получен из того же списка (выбор или предпочитаемая), поэтому он в пределах.
        let timeline = timelines.as_slice()[timeline_index];
        self.last_target_expected = Some(timeline.expected_present);
        let spin_window = Duration::from_micros(self.config.latch_spin_us);
        PresentTarget {
            timeline_index,
            preferred_index,
            choice,
            vsync_id: timeline.vsync_id,
            expected_present: timeline.expected_present,
            deadline: timeline.deadline,
            latch: FrameLatchPlan {
                latch_at: subtract(timeline.deadline, frame_cost.saturating_add(margin)),
                spin_window,
                max_wait: self.latch_wait_budget(period, spin_window),
                mode: self.config.late_latch,
            },
        }
    }

    /// Предел ожидания позднего захвата: конфиг, но не дольше периода тика без окна активного
    /// ожидания (см. модуль). Окно шире периода — предел ноль: ждать нельзя вовсе.
    pub(super) fn latch_wait_budget(&self, period: Duration, spin_window: Duration) -> Duration {
        let configured = Duration::from_micros(self.config.latch_max_wait_us);
        configured.min(period.saturating_sub(spin_window))
    }

    /// Самый ранний допустимый момент показа: строго позже линии прошлого кадра больше чем на
    /// полпериода (половина — запас на дрожание меток одного и того же vsync в разных тиках).
    fn distinct_vsync_floor(&self, refresh_period: Duration) -> Option<MonotonicNanos> {
        let previous = self.last_target_expected?;
        Some(add(previous, refresh_period / 2))
    }
}

/// Период vsync для решений тика: меньшее из периода пейсинга и наименьшего положительного шага
/// между линиями тика (см. модуль). Одна линия или вырожденные метки — период пейсинга.
pub(super) fn tick_vsync_period(timelines: &FrameTimelines, refresh_period: Duration) -> Duration {
    let presents = timelines.as_slice();
    // Линий не больше `MAX_FRAME_TIMELINES` (единицы): попарный перебор дешевле сортировки с аллокацией.
    let smallest_spacing = presents
        .iter()
        .enumerate()
        .flat_map(|(index, first)| {
            presents[index + 1..].iter().map(move |second| {
                second
                    .expected_present
                    .as_nanos()
                    .saturating_sub(first.expected_present.as_nanos())
                    .unsigned_abs()
            })
        })
        .filter(|spacing| *spacing > 0)
        .min()
        .map(Duration::from_nanos);
    match smallest_spacing {
        Some(spacing) => spacing.min(refresh_period),
        None => refresh_period,
    }
}

/// Условие выполнимости линии.
#[derive(Debug, Clone, Copy)]
struct FeasibilityRule {
    now: MonotonicNanos,
    /// Сколько времени должно оставаться до срока линии: стоимость кадра + запас.
    required_lead: Duration,
    /// Линии с показом не позже этого момента не берутся (`None` — ограничения нет).
    not_before: Option<MonotonicNanos>,
}

/// Глубина выбранной линии и была ли ближе неё выполнимая линия, запрещённая только правилом
/// «строго позже прошлой линии» (вход схода, решение 54).
fn observe_depth(timelines: &FrameTimelines, chosen: usize, rule: FeasibilityRule) -> PlannedDepth {
    let presents = timelines.as_slice();
    let chosen_present = presents[chosen].expected_present;
    let depth = presents
        .iter()
        .filter(|timeline| timeline.expected_present < chosen_present)
        .count();
    let without_floor = FeasibilityRule {
        not_before: None,
        ..rule
    };
    let shallower_blocked_by_previous_target = nearest_feasible_timeline(timelines, without_floor)
        .is_some_and(|unconstrained| presents[unconstrained].expected_present < chosen_present);
    PlannedDepth {
        depth,
        shallower_blocked_by_previous_target,
    }
}

/// Индекс самой ранней (по `expected_present`) выполнимой линии; `None` — таких нет.
fn nearest_feasible_timeline(timelines: &FrameTimelines, rule: FeasibilityRule) -> Option<usize> {
    let latest_acceptable_start = |deadline: MonotonicNanos| subtract(deadline, rule.required_lead);
    timelines
        .as_slice()
        .iter()
        .enumerate()
        .filter(|(_, timeline)| latest_acceptable_start(timeline.deadline) >= rule.now)
        .filter(|(_, timeline)| rule.not_before.is_none_or(|floor| timeline.expected_present > floor))
        .min_by_key(|(_, timeline)| timeline.expected_present)
        .map(|(index, _)| index)
}

/// `moment − duration` без переполнения.
fn subtract(moment: MonotonicNanos, duration: Duration) -> MonotonicNanos {
    let nanos = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);
    MonotonicNanos::from_nanos(moment.as_nanos().saturating_sub(nanos))
}

/// `moment + duration` без переполнения.
fn add(moment: MonotonicNanos, duration: Duration) -> MonotonicNanos {
    let nanos = i64::try_from(duration.as_nanos()).unwrap_or(i64::MAX);
    MonotonicNanos::from_nanos(moment.as_nanos().saturating_add(nanos))
}

#[cfg(test)]
#[path = "present_target_tests.rs"]
mod tests;
