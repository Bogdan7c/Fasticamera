//! Граница рендера для снимка (сессия 14): команды снимка и приёмник событий проявления.
//!
//! Рендер владеет кадрами камеры и GPU, поэтому снимок «без задержки затвора» выбирается и
//! проявляется здесь (план — `still_contract::plan::StillPlan`, обмен с записью — `still_contract::develop`).
//! Этот модуль — только словарь границы потока рендера; исполнение — у бэкенда с камерой.
//!
//! Инвариант: каждая команда [`StillCommand::Begin`], дошедшая до потока рендера, кончается РОВНО одним
//! итоговым событием в её приёмник — даже если бэкенд снимок не умеет (тогда `NotSecured`). Цель
//! кодера ([`StillCommand::Target`]) для неизвестного снимка уничтожается (поверхность кодера отпускается),
//! это учитывается счётчиком.

use std::fmt;
use std::sync::Arc;

use serde::Serialize;
use still_contract::develop::{DevelopEvent, PrimaryTargetOffer};
use still_contract::plan::{StillId, StillPlan};

/// Куда рендер отдаёт события проявления. Реализация не блокирует поток рендера (инвариант 4 README):
/// только постановка в очередь; переполнение — забота реализации (учёт), не ожидание.
pub trait StillEventSink: Send + Sync {
    fn deliver(&self, event: DevelopEvent);
}

/// Начать снимок.
pub struct StillBegin {
    pub plan: StillPlan,
    /// Приёмник событий именно этого снимка.
    pub events: Arc<dyn StillEventSink>,
}

impl fmt::Debug for StillBegin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StillBegin")
            .field("plan", &self.plan)
            .finish_non_exhaustive()
    }
}

/// Команда снимка потоку рендера. Не схлопывается: у каждой свой итог.
#[derive(Debug)]
pub enum StillCommand {
    /// Нажатие затвора: выбрать кадр, захватить, проявить.
    Begin(StillBegin),
    /// Ответ записи: куда рисовать основную картинку снимка.
    Target(PrimaryTargetOffer),
}

impl StillCommand {
    pub fn still_id(&self) -> StillId {
        match self {
            Self::Begin(begin) => begin.plan.id,
            Self::Target(offer) => offer.id(),
        }
    }
}

/// Итог команды у бэкенда.
#[derive(Debug)]
pub enum StillCommandOutcome {
    /// Команда принята (итог снимка придёт событием в приёмник).
    Accepted,
    /// Бэкенд снимок не умеет (тестовый паттерн, fake, нет камеры): команда возвращается потоку,
    /// и поток сам завершает снимок честным `NotSecured`.
    Unsupported(StillCommand),
}

/// Учёт команд снимка потоком рендера (для статуса и smoke-сценариев).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct StillCommandLog {
    /// Команды `Begin`, принятые бэкендом.
    pub begins_accepted: u64,
    /// `Begin`, которые бэкенд не умеет: снимок сразу завершён `NotSecured`.
    pub begins_unsupported: u64,
    /// Цели кодера, принятые бэкендом.
    pub targets_accepted: u64,
    /// Цели, которые бэкенд не принял: уничтожены (поверхность кодера отпущена).
    pub targets_dropped: u64,
    /// Команды, потерянные из-за неисправного бэкенда (паника): итог снимка доведёт срок записи.
    pub lost_to_backend_fault: u64,
}

/// Есть ли у бэкенда снимки, которые ещё не получили итог (сессия 14e, решение 150).
///
/// По этому ответу поток рендера решает, как спать без показа: есть недоделанные снимки — просыпаться по
/// таймеру (`still.step_interval_without_display_ms`) и двигать их; нет — спать на ящике без срока, как всегда.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StillWorkload {
    /// Очередь снимков пуста (или бэкенд снимков не умеет): будить поток незачем.
    Idle,
    /// Хотя бы один снимок ждёт итога: его шаги должны идти и без экрана.
    Unfinished,
}
