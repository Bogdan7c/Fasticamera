//! Полоски каналов R, G, B в разделе калибровки (вид BARS, решение 343).
//!
//! Инварианты те же, что у колёс (`wheels.rs`) и ручек (`calibration.rs`):
//! * действие допустимо только в режиме с разделом калибровки, и только когда есть факты о виде;
//! * значение полоски берётся из `LookFacts::current` — экран его не хранит; намерение ТОЧЕЧНОЕ (`SetPrimary`):
//!   один палец тянет R, другой — B, и ни одно намерение не несёт устаревшую копию соседней полоски;
//! * то же значение, что уже в фактах, — `NoChange(ValueUnchanged)`, наружу ничего не уходит.
//!
//! Выбор вида BARS и вкладки зоны (общей с колёсами) — в `wheels.rs`: это состояние экрана, а не полоски.

use look_contract::{LookIntent, LookKnob, PrimaryChannel, WheelZone};

use super::Ctx;
use super::look::{Availability, emit_look_from_facts};
use crate::outcome::{ActionOutcome, NoChangeReason};

/// Полоска потянута: `SetPrimary`, если значение отличается от значения в фактах.
pub(super) fn on_primary_dragged(
    ctx: &mut Ctx<'_>,
    zone: WheelZone,
    channel: PrimaryChannel,
    value: LookKnob,
) -> ActionOutcome {
    emit_look_from_facts(ctx, Availability::CalibrationOnly, |facts| {
        if facts.current.settings.primaries.value(zone, channel) == value {
            return Err(ActionOutcome::NoChange(NoChangeReason::ValueUnchanged));
        }
        Ok(LookIntent::SetPrimary { zone, channel, value })
    })
}
