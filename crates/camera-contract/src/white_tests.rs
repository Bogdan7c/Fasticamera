//! Тесты словаря белого: ящик ответов и границы уверенности.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;
use crate::auto_exposure::RefreshSignal;
use crate::ids::CameraId;

/// Получатель толчков, считающий их.
#[derive(Default)]
struct CountingReceiver {
    pushes: AtomicU32,
}

impl RefreshSignal for CountingReceiver {
    fn signal(&self) {
        self.pushes.fetch_add(1, Ordering::SeqCst);
    }
}

fn answer(request: u64) -> WhiteAnswer {
    WhiteAnswer {
        request: WhiteRequestId(request),
        camera: CameraTag::of(&CameraId::new("2").unwrap()),
        measured_frame: None,
        outcome: WhiteSourceOutcome::Busy,
    }
}

#[test]
fn an_answer_published_before_the_engine_exists_is_not_lost() {
    let channel = WhiteAnswerChannel::new();
    let (generation, delivery) = channel.publish(answer(7));
    assert_eq!(delivery, SignalDelivery::NotBoundYet);

    // Движок привязался позже: получает досланный толчок и читает лежащий в ящике ответ.
    let receiver = Arc::new(CountingReceiver::default());
    channel
        .refresh_signal()
        .bind(Arc::clone(&receiver) as Arc<dyn RefreshSignal>);
    assert_eq!(receiver.pushes.load(Ordering::SeqCst), 1);
    assert_eq!(channel.newer_than(None), Some((generation, answer(7))));
}

#[test]
fn the_latest_answer_wins_and_is_delivered_once_per_generation() {
    let channel = WhiteAnswerChannel::new();
    channel.publish(answer(1));
    let (second, _) = channel.publish(answer(2));
    assert_eq!(channel.newer_than(None), Some((second, answer(2))));
    assert_eq!(
        channel.newer_than(Some(second)),
        None,
        "прочитанный ответ не отдаётся повторно"
    );
}

#[test]
fn confidence_is_bounded_and_nan_means_no_confidence() {
    assert_eq!(WhiteConfidence::from_fraction(1.7).fraction(), 1.0);
    assert_eq!(WhiteConfidence::from_fraction(-0.2).fraction(), 0.0);
    assert_eq!(WhiteConfidence::from_fraction(f32::NAN).fraction(), 0.0);
    assert_eq!(WhiteConfidence::from_fraction(0.5).fraction(), 0.5);
}

#[test]
fn origin_names_the_source_in_diagnostics() {
    let text = serde_json::to_string(&WhiteOrigin::Source(WhiteSourceId::PHONE)).unwrap();
    assert_eq!(text, r#"{"kind":"source","source":"phone"}"#);
    let text = serde_json::to_string(&WhiteOrigin::Manual).unwrap();
    assert_eq!(text, r#"{"kind":"manual"}"#);
}
