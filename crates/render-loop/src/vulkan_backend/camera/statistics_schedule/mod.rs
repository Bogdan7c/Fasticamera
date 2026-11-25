//! Расписание статистики кадра на GPU: единственный владелец решения «считать ли её на этом кадре».
//!
//! Стадия статистики стоит заметного тока (≈ 55 мА из ≈ 119 мА ISP, 07a), поэтому считается не на каждом кадре.
//! Статистика нужна не «сама по себе», а потребителям (сегодня один — наш AE), и решает здесь один владелец:
//! * потребитель **заявляет потребность** ([`StatisticsDemand`]), а не включает стадию сам;
//! * расписание ([`StatisticsSchedule`]) сводит заявки, период и «кадры скрыты холодным стартом» в решение
//!   кадра ([`StatisticsDecision`]): считать ли на GPU и кому отдать результат;
//! * нет потребителей — стадия не считается вовсе;
//! * новый поток камеры — расписание начинается заново, первый кадр обязан дать статистику.
//!
//! От цвета потока (нейтрали камеры, калибровки) расписание НЕ зависит: поток без цветовой калибровки не должен
//! слепить экспозицию.
//!
//! # Как добавить второго потребителя
//! Новое поле в [`StatisticsNeeds`] (его потребность) и новое поле/метод `*_receives` в [`StatisticsDecision`]
//! (ему отдавать результат); `decide` сводит потребности в одном месте, соседей править не нужно.
//!
//! # Инварианты
//! 1. `Compute` ⇒ хотя бы один потребитель заявил потребность на этом кадре.
//! 2. Потребитель получает результат только на кадрах, где стадия посчитана.
//! 3. Период считается от последнего кадра со статистикой, а требование «каждый кадр» от AE его НЕ сдвигает
//!    (форсирование ради холодного старта замера не меняет установившийся ритм); скрытые кадры — сдвигают.

mod period;

use std::num::NonZeroU32;
use std::sync::Arc;

use camera_contract::frame_passport::{RawFramePassport, StreamPassport};

use self::period::PeriodCounter;
use super::pipelines::IspPresentation;
use crate::viewfinder::StatisticsDiagnostics;

/// Нужна ли статистика на этом кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatisticsPlan {
    /// Включить стадию статистики в проходе ISP.
    Compute,
    /// Пропустить стадию: оценка свежая достаточно.
    Skip,
}

/// Чего хочет AE от статистики на этом кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatisticsDemand {
    /// Нужна на каждом кадре (холодный старт замера, предвспышка): оценка сцены ещё не получена или кадры
    /// считает движок снимков.
    EveryFrame,
    /// Довольствуется установившимся периодом («каждый N-й кадр»).
    Periodic,
}

/// Спрятан ли кадр от показа холодным стартом белой. Пока кадры спрятаны, статистика идёт на каждом из них:
/// иначе скрытие затянулось бы на лишние кадры (замер AE двигает схождение экспозиции, от неё зависит ответ).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColdStartCover {
    /// Кадры показываются.
    Showing,
    /// Кадры обрабатываются, но не показываются.
    HidingFrames,
}

impl ColdStartCover {
    /// Покрытие по решению белой о показе кадра.
    pub fn of_presentation(presentation: IspPresentation) -> Self {
        match presentation {
            IspPresentation::Show => Self::Showing,
            IspPresentation::HideUntilColorKnown => Self::HidingFrames,
        }
    }
}

/// Потребности потребителей на кадре.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatisticsNeeds {
    /// Потребность AE; `None` — AE на потоке рендера выключен (вендорский AE или его нет).
    pub auto_exposure: Option<StatisticsDemand>,
    /// Спрятан ли кадр холодным стартом.
    pub cover: ColdStartCover,
}

/// Решение для одного кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatisticsDecision {
    /// Включать ли стадию статистики в проход ISP этого кадра.
    pub gpu: StatisticsPlan,
}

impl StatisticsDecision {
    /// Отдавать ли посчитанную статистику AE: ему годится любой посчитанный кадр.
    pub fn ae_receives(self) -> bool {
        self.gpu == StatisticsPlan::Compute
    }
}

/// Расписание статистики одной сессии показа (см. модуль).
#[derive(Debug)]
pub struct StatisticsSchedule {
    period: PeriodCounter,
    /// Поток, на котором расписание идёт сейчас: смена потока начинает его заново.
    stream: Option<Arc<StreamPassport>>,
    requested: u64,
    skipped_by_period: u64,
}

impl StatisticsSchedule {
    /// Расписание с установившимся периодом «каждый N-й кадр» (1 — на каждом).
    pub fn new(every_n_frames: NonZeroU32) -> Self {
        Self {
            period: PeriodCounter::new(every_n_frames),
            stream: None,
            requested: 0,
            skipped_by_period: 0,
        }
    }

    /// Решение для нового кадра камеры (вызывать один раз на каждый НОВЫЙ обработанный кадр).
    pub fn decide(&mut self, passport: &RawFramePassport, needs: StatisticsNeeds) -> StatisticsDecision {
        self.follow_stream(passport);
        // Нет потребителей: стадия не считается, период не тикает (отсчитывать нечего).
        let Some(auto_exposure) = needs.auto_exposure else {
            return StatisticsDecision {
                gpu: StatisticsPlan::Skip,
            };
        };
        // Период тикает на каждом кадре с потребителем; скрытые кадры сбрасывают его на «посчитано».
        let by_period = self.period.tick(needs.cover == ColdStartCover::HidingFrames);
        let gpu = match auto_exposure {
            StatisticsDemand::EveryFrame => StatisticsPlan::Compute,
            StatisticsDemand::Periodic => by_period,
        };
        match gpu {
            StatisticsPlan::Compute => self.requested += 1,
            StatisticsPlan::Skip => self.skipped_by_period += 1,
        }
        StatisticsDecision { gpu }
    }

    /// Диагностика расписания.
    pub fn diagnostics(&self) -> StatisticsDiagnostics {
        StatisticsDiagnostics {
            every_n_frames: self.period.every_n_frames().get(),
            requested: self.requested,
            skipped_by_period: self.skipped_by_period,
        }
    }

    /// Новый поток камеры — расписание начинается заново (первый кадр обязан дать статистику).
    fn follow_stream(&mut self, passport: &RawFramePassport) {
        if self
            .stream
            .as_ref()
            .is_some_and(|stream| Arc::ptr_eq(stream, &passport.stream))
        {
            return;
        }
        self.stream = Some(Arc::clone(&passport.stream));
        self.period.restart();
    }
}

#[cfg(test)]
mod tests;
