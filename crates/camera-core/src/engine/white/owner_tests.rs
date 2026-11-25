//! Машина владельца напрямую: случаи, до которых команды движка не доходят (возобновление поверх заявки команды).

use std::time::Instant;

use camera_contract::white::{WhiteRequestId, WhiteSourceId, WhiteTransition};

use super::owner::{WhiteNotice, WhiteOwner, WhiteRemembered};
use crate::command::CommandId;
use crate::white::{
    ResumeRuleChoice, WhiteCancelCause, WhiteConfig, WhiteFollowRecord, WhiteMemoryLoad, WhiteRequestOutcome,
    WhiteRequestState, WhiteResumeRule, WhiteTakeReport, WhiteTakeState,
};

fn owner() -> WhiteOwner {
    WhiteOwner::new(
        WhiteConfig::default(),
        WhiteSourceId::PHONE,
        WhiteRemembered {
            white: WhiteMemoryLoad::NothingRemembered,
            resume_rule: None,
            follow: WhiteFollowRecord::default(),
        },
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

#[test]
fn choosing_a_rule_with_a_pending_request_leaves_the_request_the_white_and_the_transition_alone() {
    let mut owner = owner();
    owner.on_foreground();
    let directive_before = owner.directive();
    let held_before = owner.held();
    assert!(owner.drain_notices().is_empty());

    assert_eq!(
        owner.choose_resume_rule(WhiteResumeRule::KeepLast),
        ResumeRuleChoice::Chosen
    );

    assert_eq!(
        owner.directive(),
        directive_before,
        "указание (белая, переход, заявка) то же"
    );
    assert_eq!(owner.held(), held_before);
    assert_eq!(owner.resume_rule(), WhiteResumeRule::KeepLast);
    assert_eq!(owner.status(0).resume_rule, WhiteResumeRule::KeepLast);
    assert_eq!(
        owner.drain_notices(),
        vec![WhiteNotice::ResumeRuleChosen(WhiteResumeRule::KeepLast)],
        "наружу уходит ровно одно уведомление о выборе"
    );
    assert_eq!(
        owner.choose_resume_rule(WhiteResumeRule::KeepLast),
        ResumeRuleChoice::AlreadyChosen
    );
    assert!(owner.drain_notices().is_empty(), "повтор уведомлений не даёт");
}
