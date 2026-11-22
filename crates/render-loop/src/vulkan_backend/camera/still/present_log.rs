//! Журнал показов: что было на экране под каждым `present_id` и когда драйвер отчитался о показе.
//!
//! # Как доказывается «кадр, который был на экране в момент нажатия»
//! Нажатие — момент по `MONOTONIC` (`press`). Запись драйвера (`VK_GOOGLE_display_timing`) говорит, когда показ
//! `present_id` реально появился на экране (`actual_present`, тоже `MONOTONIC`). На экране в момент `press` стоял
//! показ `B` — последний с `actual_present ≤ press`. Это ДОКАЗАНО только если известен показ `A` сразу после `B`
//! (`A.present_id = B.present_id + 1`) с `actual_present > press`: без `A` мог существовать ещё не пришедший
//! показ между `B` и `press` (записи приходят на 1–3 кадра позже и иногда теряются). Нет `A` — ждём; `A` есть,
//! но не соседний — дыра в записях драйвера, доказать нельзя.
//!
//! Часы не смешиваются: метка сенсора (`BOOTTIME`) здесь только данные, вычитаний между шкалами нет.

use std::collections::VecDeque;

use camera_contract::capture_result::SensorTimestamp;
use shell_contract::clock::MonotonicNanos;

use super::feed::{ShownContent, ShownRecord};

/// Запись драйвера: показ появился на экране в `actual`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PresentTiming {
    present_id: u32,
    actual: MonotonicNanos,
}

/// Почему момент нажатия доказать нельзя (ждать записей дальше бессмысленно).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unprovable {
    /// Между показом до нажатия и показом после есть дыра: записи драйвера потеряны.
    GapInTimingRecords,
    /// Все известные записи позже нажатия: что было на экране в момент нажатия, не знаем.
    PressBeforeKnownTimings,
    /// Показ, стоявший на экране, есть в записях драйвера, но не в нашем журнале (вытеснен ёмкостью).
    ShownContentForgotten,
}

/// Итог разбора момента нажатия по журналу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressResolution {
    /// Доказано: в момент нажатия на экране стояло `content`.
    Proven {
        content: ShownContent,
        /// Когда ЭТОТ кадр появился на экране впервые (если запись о первом показе потеряна — время показа,
        /// стоявшего на экране при нажатии).
        first_shown_at: MonotonicNanos,
    },
    /// Записей, закрывающих момент нажатия, ещё нет: подождать.
    Waiting,
    /// Доказать нельзя.
    Unprovable(Unprovable),
}

/// Журнал показов бэкенда (живёт между присоединениями surface; цепочку пересоздали — [`Self::restart`]).
#[derive(Debug)]
pub struct PresentLog {
    shown: VecDeque<ShownRecord>,
    timings: VecDeque<PresentTiming>,
    capacity: usize,
}

impl PresentLog {
    /// Журнал на `capacity` последних показов (и столько же записей драйвера).
    pub fn new(capacity: usize) -> Self {
        Self {
            shown: VecDeque::new(),
            timings: VecDeque::new(),
            capacity: capacity.max(1),
        }
    }

    /// Номера показов начались заново (пересоздание цепочки) или surface отпущена: прежнее больше ничьё.
    pub fn restart(&mut self) {
        self.shown.clear();
        self.timings.clear();
    }

    /// Показ отправлен.
    pub fn record_shown(&mut self, record: ShownRecord) {
        self.shown.push_back(record);
        while self.shown.len() > self.capacity {
            self.shown.pop_front();
        }
    }

    /// Драйвер отчитался о показе.
    pub fn record_timing(&mut self, present_id: u32, actual: MonotonicNanos) {
        self.timings.push_back(PresentTiming { present_id, actual });
        while self.timings.len() > self.capacity {
            self.timings.pop_front();
        }
    }

    /// Что показано последним (самый новый показ журнала); `None` — показов нет.
    pub fn latest_shown(&self) -> Option<ShownContent> {
        self.shown.back().map(|record| record.content)
    }

    /// Разбор момента нажатия `press` (см. модуль).
    pub fn resolve(&self, press: MonotonicNanos) -> PressResolution {
        let Some(after_index) = self.timings.iter().position(|timing| timing.actual > press) else {
            // Ни одной записи позже нажатия: либо записей нет вовсе, либо все раньше — ждём закрывающую.
            return PressResolution::Waiting;
        };
        let Some(before_index) = after_index.checked_sub(1) else {
            return PressResolution::Unprovable(Unprovable::PressBeforeKnownTimings);
        };
        let (Some(before), Some(after)) = (self.timings.get(before_index), self.timings.get(after_index)) else {
            return PressResolution::Unprovable(Unprovable::PressBeforeKnownTimings);
        };
        if after.present_id != before.present_id.wrapping_add(1) {
            return PressResolution::Unprovable(Unprovable::GapInTimingRecords);
        }
        let Some(content) = self.content_of(before.present_id) else {
            return PressResolution::Unprovable(Unprovable::ShownContentForgotten);
        };
        let first_shown_at = match content {
            ShownContent::Live(sensor_timestamp) => self.first_shown_at(sensor_timestamp).unwrap_or(before.actual),
            ShownContent::NotLive => before.actual,
        };
        PressResolution::Proven {
            content,
            first_shown_at,
        }
    }

    fn content_of(&self, present_id: u32) -> Option<ShownContent> {
        self.shown
            .iter()
            .rev()
            .find(|record| record.present_id == present_id)
            .map(|record| record.content)
    }

    /// Время первого показа кадра: первый показ с этой меткой, у которого есть запись драйвера.
    fn first_shown_at(&self, sensor_timestamp: SensorTimestamp) -> Option<MonotonicNanos> {
        self.shown
            .iter()
            .filter(|record| record.content == ShownContent::Live(sensor_timestamp))
            .find_map(|record| {
                self.timings
                    .iter()
                    .find(|timing| timing.present_id == record.present_id)
                    .map(|timing| timing.actual)
            })
    }
}

#[cfg(test)]
#[path = "present_log_tests.rs"]
mod tests;
