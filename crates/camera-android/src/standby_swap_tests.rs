use super::*;

/// Что порт делал, в порядке вызовов.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Promote(StreamEpoch),
    FlipRoles(StandbySlotId),
    Exchange(StandbySlotId),
}

struct FakePorts {
    standby_epoch: Option<StreamEpoch>,
    primary_is_streaming: bool,
    promotion: StandbyPromotion,
    steps: Vec<Step>,
}

impl FakePorts {
    fn ready() -> Self {
        Self {
            standby_epoch: Some(StreamEpoch(7)),
            primary_is_streaming: true,
            promotion: StandbyPromotion::Promoted {
                shown: StreamEpoch(7),
                demoted_to_standby: Some(StreamEpoch(5)),
            },
            steps: Vec::new(),
        }
    }
}

impl SwapPorts for FakePorts {
    fn standby_epoch(&self, _slot: StandbySlotId) -> Option<StreamEpoch> {
        self.standby_epoch
    }
    fn primary_is_streaming(&self) -> bool {
        self.primary_is_streaming
    }
    fn promote_in_frame_stream(&mut self, standby: StreamEpoch) -> StandbyPromotion {
        self.steps.push(Step::Promote(standby));
        self.promotion
    }
    fn flip_result_roles(&mut self, slot: StandbySlotId) {
        self.steps.push(Step::FlipRoles(slot));
    }
    fn exchange_resources(&mut self, slot: StandbySlotId) {
        self.steps.push(Step::Exchange(slot));
    }
}

const SLOT: StandbySlotId = StandbySlotId::new(0);

#[test]
fn successful_swap_runs_frame_stream_then_roles_then_resources() {
    let mut ports = FakePorts::ready();
    assert_eq!(swap_with_primary(&mut ports, SLOT), Ok(()));
    assert_eq!(
        ports.steps,
        vec![
            Step::Promote(StreamEpoch(7)),
            Step::FlipRoles(SLOT),
            Step::Exchange(SLOT)
        ]
    );
}

#[test]
fn slot_not_streaming_fails_without_touching_anything() {
    let mut ports = FakePorts::ready();
    ports.standby_epoch = None;
    assert_eq!(
        swap_with_primary(&mut ports, SLOT),
        Err(StandbySwapFailure::StandbyNotStreaming)
    );
    assert!(ports.steps.is_empty());
}

#[test]
fn primary_not_streaming_fails_without_touching_anything() {
    let mut ports = FakePorts::ready();
    ports.primary_is_streaming = false;
    assert_eq!(
        swap_with_primary(&mut ports, SLOT),
        Err(StandbySwapFailure::PrimaryNotStreaming)
    );
    assert!(ports.steps.is_empty());
}

#[test]
fn slot_check_wins_when_both_sides_are_missing() {
    let mut ports = FakePorts::ready();
    ports.standby_epoch = None;
    ports.primary_is_streaming = false;
    assert_eq!(
        swap_with_primary(&mut ports, SLOT),
        Err(StandbySwapFailure::StandbyNotStreaming)
    );
}

#[test]
fn hub_refusal_keeps_roles_and_resources_untouched() {
    let mut ports = FakePorts::ready();
    ports.promotion = StandbyPromotion::NoSuchStandby;
    assert_eq!(
        swap_with_primary(&mut ports, SLOT),
        Err(StandbySwapFailure::StandbyNotStreaming)
    );
    // Хаб спрошен, но роли и ресурсы не тронуты: откат не нужен.
    assert_eq!(ports.steps, vec![Step::Promote(StreamEpoch(7))]);
}

#[test]
fn swap_without_previous_shown_epoch_still_succeeds() {
    let mut ports = FakePorts::ready();
    ports.promotion = StandbyPromotion::Promoted {
        shown: StreamEpoch(7),
        demoted_to_standby: None,
    };
    assert_eq!(swap_with_primary(&mut ports, SLOT), Ok(()));
    assert_eq!(ports.steps.len(), 3);
}
