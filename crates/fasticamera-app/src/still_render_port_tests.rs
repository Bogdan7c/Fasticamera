//! Тесты шва к рендеру: настоящий ящик потока рендера (команда лежит в нём), завершённый рендер, «рендера нет»,
//! ответ службы целью основной картинки.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use render_loop::mailbox::{InboxEvent, render_mailbox};
use render_loop::still::StillBegin;
use still_contract::develop::{PrimaryTarget, PrimaryTargetOffer};
use still_contract::plan::{FrameSelection, StillId};

use super::*;
use crate::still_test_support::{FakeRenderPort, RenderMode, shot};

/// Приёмник событий, который ничего не делает (план в этих тестах не проявляется).
struct IdleSink;

impl render_loop::still::StillEventSink for IdleSink {
    fn deliver(&self, _event: still_contract::develop::DevelopEvent) {}

    // Тест порта рендера миниатюры не проверяет: снимки здесь до проявления не доходят.
    fn deliver_thumbnail(&self, _thumbnail: still_contract::thumbnail::StillThumbnail) {}
}

fn begin_command(id: u64) -> StillCommand {
    StillCommand::Begin(StillBegin {
        plan: crate::still_capture_service::plan_from_shot(
            StillId(id),
            &shot(1),
            crate::look_test_support::neutral_recipe(),
            FrameSelection::ShownAtPress,
        ),
        events: Arc::new(IdleSink),
    })
}

#[test]
fn live_inbox_receives_the_command_for_the_render_thread() {
    let (inbox, receiver) = render_mailbox();
    let port = InboxStillPort::new(inbox);

    let delivery = port.send_still_command(begin_command(42));

    assert_eq!(delivery, StillCommandDelivery::Delivered);
    let batch = receiver.take_pending();
    assert_eq!(batch.events.len(), 1);
    match &batch.events[0].event {
        InboxEvent::Still(command) => assert_eq!(command.still_id(), StillId(42)),
        other => panic!("в ящике лежит не команда снимка: {other:?}"),
    }
}

#[test]
fn finished_render_thread_is_reported_as_gone() {
    let (inbox, receiver) = render_mailbox();
    drop(receiver);
    let port = InboxStillPort::new(inbox);
    assert_eq!(
        port.send_still_command(begin_command(1)),
        StillCommandDelivery::RenderGone
    );
}

#[test]
fn unavailable_render_answers_with_its_reason() {
    let port = UnavailableStillPort::new("GPU не поднялся");
    assert_eq!(
        port.send_still_command(begin_command(1)),
        StillCommandDelivery::RenderUnavailable {
            reason: "GPU не поднялся".to_owned()
        }
    );
}

#[test]
fn service_answer_reaches_the_render_as_a_target_command() {
    let render = FakeRenderPort::new(RenderMode::Accepting);
    let route = RenderTargetRoute::new(render.clone());

    let result = route.offer_primary_target(PrimaryTargetOffer::Ready {
        id: StillId(3),
        target: PrimaryTarget::Readback,
    });
    route
        .offer_primary_target(PrimaryTargetOffer::Unavailable { id: StillId(4) })
        .unwrap();

    assert!(result.is_ok());
    assert_eq!(render.unavailable_offers(), vec![StillId(4)]);
    render.wait_readback_target(StillId(3));
}

#[test]
fn target_for_a_dead_render_is_an_error_the_service_can_act_on() {
    for mode in [RenderMode::Gone, RenderMode::Unavailable] {
        let route = RenderTargetRoute::new(FakeRenderPort::new(mode));
        let result = route.offer_primary_target(PrimaryTargetOffer::Unavailable { id: StillId(1) });
        assert_eq!(result, Err(RenderUnavailable), "{mode:?}");
    }
}
