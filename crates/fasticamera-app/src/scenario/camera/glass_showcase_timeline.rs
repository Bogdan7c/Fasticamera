//! Сценарий времени демо-сцены: период цикла, окна «нажато / разведено / раскрыто» и расписание событий.
//!
//! Время цикла — чистая функция абсолютного времени показа (`present_time`): фаза = время по модулю
//! периода. Поэтому два экземпляра сцены и частоты 60/120 Гц в одни и те же моменты видят одно и то же.
//! Событие расписания нумеруется сквозным индексом (`цикл · событий_в_цикле + номер`), так что «следующее
//! событие» — просто следующий индекс, а переход через границу цикла ничем не отличается от обычного шага.

use serde::{Deserialize, Serialize};
use ui_core::time::MonotonicNanos;

use super::config::{require_in_range, require_positive};
use super::error::ShowcaseConfigError;

const NANOS_PER_SECOND: f64 = 1.0e9;

/// Окно «состояние включено»: внутри цикла от `start_s` (вход) до `end_s` (выход), секунды от начала цикла.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveWindow {
    pub start_s: f64,
    pub end_s: f64,
}

impl ActiveWindow {
    fn validate(&self, field: &'static str, latest_end_s: f64) -> Result<(), ShowcaseConfigError> {
        require_in_range(field, self.start_s, 0.0, latest_end_s)?;
        require_in_range(field, self.end_s, 0.0, latest_end_s)?;
        if self.start_s >= self.end_s {
            return Err(ShowcaseConfigError::Inconsistent {
                reason: format!(
                    "окно «{field}»: начало {} с должно быть раньше конца {} с",
                    self.start_s, self.end_s
                ),
            });
        }
        Ok(())
    }
}

/// Что включает окно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowcaseTarget {
    /// Нажатие левой круглой кнопки (вспышка).
    FlashPress,
    /// Нажатие правой круглой кнопки (переворот камеры).
    FlipPress,
    /// Кружки зума разведены (иначе сведены в «капли»).
    ZoomApart,
    /// Панель меню раскрыта (иначе свёрнута в кнопку меню).
    PanelOpen,
}

impl ShowcaseTarget {
    /// Все цели в порядке разбора одновременных событий (детерминизм при совпадении времён).
    pub const ALL: [ShowcaseTarget; 4] = [
        ShowcaseTarget::FlashPress,
        ShowcaseTarget::FlipPress,
        ShowcaseTarget::ZoomApart,
        ShowcaseTarget::PanelOpen,
    ];
}

/// Край окна: вход (состояние включается) или выход (возвращается в покой).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEdge {
    Enter,
    Leave,
}

/// Одно событие расписания.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimelineEvent {
    pub target: ShowcaseTarget,
    pub edge: WindowEdge,
}

/// Сценарий времени из конфига.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShowcaseTimeline {
    /// Период цикла, секунды. `[estimate]` стенда: 8 с — все окна и успокоение пружин укладываются.
    pub cycle_period_s: f64,
    /// Запас на успокоение пружин в конце цикла, секунды: окна должны закончиться не позже
    /// `период − запас`, иначе в начале следующего цикла сцена не в покое. `[estimate]`: самая долгая
    /// пружина (`camera.sheet`, 0,45 с; `snappy`, 0,5 с) успокаивается за ~0,8 с, берём с запасом.
    pub settle_margin_s: f64,
    /// Нажатие вспышки. `[estimate]` стенда, подгонка, не Apple: 0,6 с держим — пружина доходит до упора.
    pub flash_press: ActiveWindow,
    /// Нажатие переворота камеры. `[estimate]`, подгонка, не Apple.
    pub flip_press: ActiveWindow,
    /// Кружки зума разведены. `[estimate]`, подгонка, не Apple.
    pub zoom_apart: ActiveWindow,
    /// Панель раскрыта. `[estimate]`, подгонка, не Apple.
    pub panel_open: ActiveWindow,
    /// Допуск покоя безразмерных величин (давление 0..1): пол допуска положения. `[estimate]` 0,002 —
    /// как `AnimatorConfig::opacity_rest` в `ui-core` (полшага 8-битного канала). Подгонка, не Apple.
    pub unit_rest_floor: f64,
}

impl Default for ShowcaseTimeline {
    fn default() -> Self {
        Self {
            cycle_period_s: 8.0,
            settle_margin_s: 1.0,
            flash_press: ActiveWindow {
                start_s: 0.6,
                end_s: 1.2,
            },
            flip_press: ActiveWindow {
                start_s: 1.4,
                end_s: 2.0,
            },
            zoom_apart: ActiveWindow {
                start_s: 2.4,
                end_s: 4.0,
            },
            panel_open: ActiveWindow {
                start_s: 4.4,
                end_s: 6.4,
            },
            unit_rest_floor: 0.002,
        }
    }
}

impl ShowcaseTimeline {
    /// Окно цели.
    pub fn window(&self, target: ShowcaseTarget) -> ActiveWindow {
        match target {
            ShowcaseTarget::FlashPress => self.flash_press,
            ShowcaseTarget::FlipPress => self.flip_press,
            ShowcaseTarget::ZoomApart => self.zoom_apart,
            ShowcaseTarget::PanelOpen => self.panel_open,
        }
    }

    /// Проверить период, запас и окна.
    pub fn validate(&self) -> Result<(), ShowcaseConfigError> {
        require_positive("timeline.cycle_period_s", self.cycle_period_s)?;
        require_in_range(
            "timeline.settle_margin_s",
            self.settle_margin_s,
            0.0,
            self.cycle_period_s,
        )?;
        require_positive("timeline.unit_rest_floor", self.unit_rest_floor)?;
        let latest_end_s = self.cycle_period_s - self.settle_margin_s;
        self.flash_press.validate("timeline.flash_press", latest_end_s)?;
        self.flip_press.validate("timeline.flip_press", latest_end_s)?;
        self.zoom_apart.validate("timeline.zoom_apart", latest_end_s)?;
        self.panel_open.validate("timeline.panel_open", latest_end_s)
    }
}

/// Расписание событий цикла (по возрастанию времени, при равенстве — по порядку целей).
#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    period_ns: i64,
    /// События одного цикла: (смещение от начала цикла, ns; событие).
    events: Vec<(i64, TimelineEvent)>,
}

fn seconds_to_nanos(seconds: f64) -> i64 {
    (seconds * NANOS_PER_SECOND).round() as i64
}

impl Schedule {
    /// Расписание из проверенного конфига (`ShowcaseTimeline::validate`).
    pub fn from_timeline(timeline: &ShowcaseTimeline) -> Self {
        let mut events: Vec<(i64, TimelineEvent)> = Vec::with_capacity(ShowcaseTarget::ALL.len() * 2);
        for target in ShowcaseTarget::ALL {
            let window = timeline.window(target);
            events.push((
                seconds_to_nanos(window.start_s),
                TimelineEvent {
                    target,
                    edge: WindowEdge::Enter,
                },
            ));
            events.push((
                seconds_to_nanos(window.end_s),
                TimelineEvent {
                    target,
                    edge: WindowEdge::Leave,
                },
            ));
        }
        // Стабильная сортировка: при равных временах остаётся порядок вставки (цели по `ALL`).
        events.sort_by_key(|(offset_ns, _)| *offset_ns);
        Self {
            period_ns: seconds_to_nanos(timeline.cycle_period_s),
            events,
        }
    }

    /// Сколько событий в одном цикле.
    pub fn events_per_cycle(&self) -> i64 {
        self.events.len() as i64
    }

    /// Начало цикла, в который попадает `time`.
    pub fn cycle_start(&self, time: MonotonicNanos) -> MonotonicNanos {
        let nanos = time.as_nanos();
        MonotonicNanos::from_nanos(nanos - nanos.rem_euclid(self.period_ns))
    }

    /// Период цикла, ns.
    pub fn period_ns(&self) -> i64 {
        self.period_ns
    }

    /// Сквозной индекс первого события цикла, начинающегося в `cycle_start`.
    pub fn first_index_of_cycle(&self, cycle_start: MonotonicNanos) -> i64 {
        cycle_start.as_nanos().div_euclid(self.period_ns) * self.events_per_cycle()
    }

    /// Событие по сквозному индексу и его абсолютное время.
    pub fn event(&self, index: i64) -> (MonotonicNanos, TimelineEvent) {
        let per_cycle = self.events_per_cycle();
        let cycle = index.div_euclid(per_cycle);
        // `rem_euclid` неотрицателен и меньше `per_cycle`, так что индекс всегда в границах.
        let (offset_ns, event) = self.events[index.rem_euclid(per_cycle) as usize];
        (MonotonicNanos::from_nanos(cycle * self.period_ns + offset_ns), event)
    }
}
