//! Шов «приложение → поток рендера» для снимка: одна команда за раз, без ожидания.
//!
//! Снимок знает о рендере только этот трейт: настоящий ящик потока рендера ([`InboxStillPort`]), «рендера нет»
//! ([`UnavailableStillPort`]) и подделки тестов подставляются без изменений сервиса снимка. Шов различает «ушло»,
//! «рендер завершён» и «рендер не поднимался» — вызывающему они нужны по-разному (счётчики, причина в диагностике).
//!
//! Тот же шов отвечает рендеру целью основной картинки: [`RenderTargetRoute`] — настоящий `PrimaryTargetSink`
//! службы записи.

use std::sync::Arc;

use photo_output::service::{PrimaryTargetSink, RenderUnavailable};
use render_loop::mailbox::{InboxDelivery, RenderInbox};
use render_loop::still::StillCommand;
use still_contract::develop::PrimaryTargetOffer;

/// Дошла ли команда снимка до потока рендера. Итог СНИМКА придёт позже событием проявления.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StillCommandDelivery {
    /// Команда лежит в ящике потока рендера.
    Delivered,
    /// Поток рендера уже завершён: команда уничтожена вместе с поверхностью кодера, если она в ней была.
    RenderGone,
    /// Рендер не поднимался (GPU или vsync): снимать нечем.
    RenderUnavailable { reason: String },
}

/// Куда приложение отправляет команды снимка потоку рендера. Не блокирует.
pub trait StillRenderPort: Send + Sync {
    fn send_still_command(&self, command: StillCommand) -> StillCommandDelivery;
}

/// Что приложение знает о рендере для снимка к моменту сборки.
pub enum StillRenderLink {
    /// Поток рендера запущен.
    Live(Arc<dyn StillRenderPort>),
    /// Рендер не поднялся (GPU, vsync, поток): снимать нечем; причина — для диагностики.
    Unavailable { reason: String },
}

/// Живой поток рендера: команды идут тем же ящиком, что события поверхности и регулировки.
pub struct InboxStillPort {
    inbox: RenderInbox,
}

impl InboxStillPort {
    pub fn new(inbox: RenderInbox) -> Self {
        Self { inbox }
    }
}

impl StillRenderPort for InboxStillPort {
    fn send_still_command(&self, command: StillCommand) -> StillCommandDelivery {
        match self.inbox.still_command(command) {
            InboxDelivery::Delivered => StillCommandDelivery::Delivered,
            InboxDelivery::ReceiverGone => StillCommandDelivery::RenderGone,
        }
    }
}

/// Рендера нет: каждая команда получает причину отказа.
pub struct UnavailableStillPort {
    reason: String,
}

impl UnavailableStillPort {
    pub fn new(reason: impl Into<String>) -> Self {
        Self { reason: reason.into() }
    }
}

impl StillRenderPort for UnavailableStillPort {
    fn send_still_command(&self, _command: StillCommand) -> StillCommandDelivery {
        StillCommandDelivery::RenderUnavailable {
            reason: self.reason.clone(),
        }
    }
}

/// Ответ службы записи рендеру («рисуй сюда»): настоящий `PrimaryTargetSink`.
///
/// Недоставленное предложение уничтожается вместе с поверхностью кодера; служба узнаёт об этом по `Err` и сама
/// срывает снимок (рендер без цели проявлять не будет).
pub struct RenderTargetRoute {
    render: Arc<dyn StillRenderPort>,
}

impl RenderTargetRoute {
    pub fn new(render: Arc<dyn StillRenderPort>) -> Self {
        Self { render }
    }
}

impl PrimaryTargetSink for RenderTargetRoute {
    fn offer_primary_target(&self, offer: PrimaryTargetOffer) -> Result<(), RenderUnavailable> {
        match self.render.send_still_command(StillCommand::Target(offer)) {
            StillCommandDelivery::Delivered => Ok(()),
            StillCommandDelivery::RenderGone | StillCommandDelivery::RenderUnavailable { .. } => Err(RenderUnavailable),
        }
    }
}

#[cfg(test)]
#[path = "still_render_port_tests.rs"]
mod tests;
