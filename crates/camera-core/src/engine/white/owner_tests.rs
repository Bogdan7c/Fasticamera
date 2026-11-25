//! Машина владельца напрямую: случаи, до которых команды движка не доходят (возобновление поверх заявки команды).

use std::time::Instant;

use camera_contract::white::{WhiteRequestId, WhiteSourceId, WhiteTransition};

use super::owner::WhiteOwner;
use crate::command::CommandId;
use crate::white::{
    WhiteCancelCause, WhiteConfig, WhiteMemoryLoad, WhiteRequestOutcome, WhiteRequestState, WhiteTakeReport,
    WhiteTakeState,
};

fn owner() -> WhiteOwner {
    WhiteOwner::new(
        WhiteConfig::default(),
        WhiteSourceId::PHONE,
        WhiteMemoryLoad::NothingRemembered,
        1,
    )
}

#[test]
fn a_resumption_that_replaces_the_request_of_a_take_closes_its_report_as_replaced() {
    let mut owner = owner();
    owner.take_from_source(
        CommandId(7),
        WhiteSourceId::PHONE,
        WhiteTransition::Immediate,
        Instant::now(),
    );
    assert_eq!(
        owner.status(0).last_take,
        Some(WhiteTakeReport {
            command: CommandId(7),
            state: WhiteTakeState::Waiting
        })
    );

    owner.on_foreground();

    let status = owner.status(0);
    assert_eq!(
        status.last_take,
        Some(WhiteTakeReport {
            command: CommandId(7),
            state: WhiteTakeState::Finished(WhiteRequestOutcome::Cancelled {
                by: WhiteCancelCause::ReplacedByNewRequest
            })
        })
    );
    assert_eq!(
        status.request,
        WhiteRequestState::Waiting {
            request: WhiteRequestId(2),
            source: WhiteSourceId::PHONE
        },
        "открытие заняло место заявки команды"
    );
}
