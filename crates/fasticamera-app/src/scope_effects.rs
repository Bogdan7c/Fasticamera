//! Исполнение намерений по цветовым приборам (14h, решения 183, 188–190).
//!
//! Экран шлёт [`ScopeIntent`] эффектом `Effect::Scopes`. Здесь он без правок превращается в
//! `ViewfinderCommand::Scopes` и уходит рендеру: приборы считает и показывает только рендер, приложение ничего не
//! хранит и не проверяет (предел размера картинки знает бэкенд и отвечает `Rejected` в диагностике).
//!
//! Порядок важен: рендер отзывает запрос, когда источник UI ставится, заменяется или снимается, поэтому
//! `Show` обязан прийти ПОСЛЕ установки источника экрана. Экран шлёт намерение из кадра уже установленного
//! источника, так что порядок выполняется сам собой; повторять `Show` после переустановки экрана — дело экрана (14b).
//!
//! Доставка не молчит: каждый исход считается отдельно, а недоставка пишется в лог.

use render_loop::viewfinder::ViewfinderCommand;
use scope_contract::ScopeIntent;

use crate::viewfinder_control::{ControlDelivery, RenderAdjustment, RenderControl};

const LOG_TARGET: &str = "fasticamera::scopes";

/// Куда делись намерения по приборам (исходы доставки не сливаются в один счётчик).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScopeCounters {
    /// Просьба «показать» лежит в ящике потока рендера.
    pub show_delivered: u64,
    /// Просьба «скрыть» лежит в ящике потока рендера.
    pub hide_delivered: u64,
    /// Поток рендера уже завершён: намерение потеряно.
    pub render_gone: u64,
    /// Рендер не поднимался: намерение потеряно.
    pub render_unavailable: u64,
}

/// Исполнитель намерений по приборам: пересылает их рендеру и считает исходы.
#[derive(Debug, Default)]
pub struct ScopeEffects {
    counters: ScopeCounters,
}

impl ScopeEffects {
    /// Счётчики на данный момент.
    pub fn counters(&self) -> ScopeCounters {
        self.counters
    }

    /// Отдаёт намерение рендеру ровно одной командой, без правок.
    pub fn run_intent(&mut self, intent: ScopeIntent, render: &dyn RenderControl) {
        let delivery = render.adjust(RenderAdjustment::Viewfinder(ViewfinderCommand::Scopes(intent)));
        let counters = &mut self.counters;
        match (delivery, intent) {
            (ControlDelivery::Delivered, ScopeIntent::Show(_)) => counters.show_delivered += 1,
            (ControlDelivery::Delivered, ScopeIntent::Hide) => counters.hide_delivered += 1,
            (ControlDelivery::RenderGone, _) => {
                counters.render_gone += 1;
                tracing::warn!(target: LOG_TARGET, ?intent, "поток рендера завершён: намерение по прибору не доставлено");
            }
            (ControlDelivery::RenderUnavailable { reason }, _) => {
                counters.render_unavailable += 1;
                tracing::warn!(target: LOG_TARGET, ?intent, %reason, "рендер недоступен: намерение по прибору не доставлено");
            }
        }
    }
}

#[cfg(test)]
#[path = "scope_effects_tests.rs"]
mod tests;
