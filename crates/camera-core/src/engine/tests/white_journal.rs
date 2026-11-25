//! Журнал белого: ручные правки идут потоком (шкала под пальцем — до 120 раз в секунду) и строк `info` не
//! плодят; редкие события остаются в `info`.

use std::sync::{Arc, Mutex, PoisonError};

use camera_contract::white::{WhiteAdjustment, WhiteSourceId, WhiteTransition};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing::{Event, Level, Metadata, Subscriber};

use super::white_support::*;
use super::*;

/// Что записал перехватчик: уровень и цель каждого события.
type Seen = Arc<Mutex<Vec<(Level, String)>>>;

/// Минимальный перехватчик событий `tracing` (отдельный крейт подписчика ради одного теста не нужен).
struct Capture {
    seen: Seen,
}

struct IgnoreFields;

impl Visit for IgnoreFields {
    fn record_debug(&mut self, _field: &Field, _value: &dyn std::fmt::Debug) {}
}

impl Subscriber for Capture {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _attributes: &Attributes<'_>) -> Id {
        Id::from_u64(1)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, event: &Event<'_>) {
        event.record(&mut IgnoreFields);
        self.seen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((*event.metadata().level(), event.metadata().target().to_owned()));
    }

    fn enter(&self, _span: &Id) {}

    fn exit(&self, _span: &Id) {}
}

/// Сколько событий журнала белого каждого уровня выпустил `scenario`.
fn white_log_levels(scenario: impl FnOnce()) -> Vec<Level> {
    let seen: Seen = Arc::default();
    tracing::subscriber::with_default(
        Capture {
            seen: Arc::clone(&seen),
        },
        scenario,
    );
    let recorded = seen.lock().unwrap_or_else(PoisonError::into_inner);
    recorded
        .iter()
        .filter(|(_, target)| target == "fasticamera::white")
        .map(|(level, _)| *level)
        .collect()
}

#[test]
fn a_stream_of_manual_adjustments_writes_no_info_lines_and_a_source_change_still_does() {
    let (mut rig, outside) = white_rig_streaming();
    rig.frame_after(Duration::ZERO);
    // Заявка открытия получила ответ: «рука отменила ожидание» (одна строка info) в поток правок не попадёт.
    rig.deliver_answer(&outside, ready(1, setting(3_900.0, 0.0)));

    let drag = white_log_levels(|| {
        for step in 0..120_u32 {
            let kelvin = 3_000.0 + f64::from(step) * 10.0;
            rig.send(CameraCommand::AdjustWhite {
                adjustment: WhiteAdjustment::Temperature(setting(kelvin, 0.0).temperature),
                transition: WhiteTransition::Immediate,
            });
            rig.send(CameraCommand::SetWhite {
                setting: setting(kelvin, 0.01),
                transition: WhiteTransition::Immediate,
            });
        }
    });
    assert!(!drag.is_empty(), "правки в журнале есть (в debug)");
    assert!(
        drag.iter().all(|level| *level == Level::DEBUG),
        "под пальцем ни одной строки выше debug: {drag:?}"
    );

    // Редкое событие — ответ источника — остаётся в info.
    rig.send(CameraCommand::TakeWhite {
        source: WhiteSourceId::PHONE,
        transition: WhiteTransition::Immediate,
    });
    let source_change = white_log_levels(|| {
        rig.deliver_answer(&outside, ready(2, setting(3_700.0, 0.0)));
    });
    assert!(
        source_change.contains(&Level::INFO),
        "смена белой от источника видна в info: {source_change:?}"
    );
}
