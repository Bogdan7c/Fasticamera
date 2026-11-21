use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use super::*;
use crate::three_a::{MeteringRegion, SensorPoint};

fn product(seconds_iso: f64) -> ExposureProduct {
    ExposureProduct::from_seconds_iso(seconds_iso).unwrap()
}

fn proposal(nanos: i64, seconds_iso: f64) -> ExposureProposal {
    ExposureProposal {
        measured_frame: SensorTimestamp::from_nanos(nanos),
        measured_on: CameraTag::of(&crate::fixtures::camera_id("0")),
        target: product(seconds_iso),
        metered_area: MeteredArea::WholeFrame,
        convergence: ProposalConvergence::Searching,
        quality: MeasurementQuality::Measured,
    }
}

/// Считающий получатель толчков.
#[derive(Default)]
struct CountingReceiver {
    signals: AtomicU32,
}

impl RefreshSignal for CountingReceiver {
    fn signal(&self) {
        self.signals.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn product_rejects_zero_negative_and_non_finite_values() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(
            ExposureProduct::from_seconds_iso(bad).is_err(),
            "{bad} должен быть отвергнут"
        );
    }
    assert_eq!(ExposureProduct::from_result(Duration::ZERO, 100), None);
    assert_eq!(ExposureProduct::from_result(Duration::from_millis(10), 0), None);
}

#[test]
fn product_from_sensor_is_seconds_times_iso_and_ev_is_a_log2_ratio() {
    let ten_ms_iso_400 = ExposureProduct::from_sensor(
        ExposureTime::new(Duration::from_millis(10)).unwrap(),
        Iso::new(400).unwrap(),
    );
    assert!((ten_ms_iso_400.seconds_iso() - 4.0).abs() < 1e-12);
    let doubled = ten_ms_iso_400.scaled_by_ev(1.0).unwrap();
    assert!((doubled.seconds_iso() - 8.0).abs() < 1e-12);
    assert!((doubled.ev_above(ten_ms_iso_400) - 1.0).abs() < 1e-12);
    assert!((ten_ms_iso_400.ev_above(doubled) + 1.0).abs() < 1e-12);
    // Выход за конечные числа — явный отказ, а не бесконечность в запросе.
    assert_eq!(ten_ms_iso_400.scaled_by_ev(2000.0), None);
}

#[test]
fn metered_area_follows_the_point_of_interest() {
    let region = MeteringRegion::new(SensorPoint::new(0.2, 0.7).unwrap(), 0.18).unwrap();
    let tap = PointOfInterest::Point {
        region,
        request: PointRequestId(7),
    };
    assert_eq!(MeteredArea::for_point(tap), MeteredArea::Point(PointRequestId(7)));
    assert_eq!(
        MeteredArea::for_point(PointOfInterest::WholeFrame),
        MeteredArea::WholeFrame
    );
}

#[test]
fn signal_before_binding_is_counted_and_not_lost_from_the_slot() {
    let channel = ExposureProposalChannel::new();
    let (generation, delivery) = channel.publish(proposal(1_000, 4.0));
    assert_eq!(delivery, SignalDelivery::NotBoundYet);
    assert_eq!(channel.refresh_signal().missed_before_bind(), 1);
    // Значение уже в ящике; толчок пропущен, его дошлёт `bind`.
    assert_eq!(channel.latest(), Some((generation, proposal(1_000, 4.0))));
}

#[test]
fn bound_receiver_gets_every_signal_and_clones_share_the_binding() {
    let channel = ExposureProposalChannel::new();
    let writer = channel.clone();
    let receiver = Arc::new(CountingReceiver::default());
    channel.refresh_signal().bind(receiver.clone());

    assert_eq!(writer.publish(proposal(1_000, 4.0)).1, SignalDelivery::Delivered);
    assert_eq!(writer.publish(proposal(2_000, 8.0)).1, SignalDelivery::Delivered);
    assert_eq!(receiver.signals.load(Ordering::Relaxed), 2);
    assert_eq!(writer.refresh_signal().missed_before_bind(), 0);

    // Читатель, видевший первое поколение, получает только второе.
    let first = Some(DirectiveGeneration(1));
    assert_eq!(
        channel.newer_than(first).map(|(_, value)| value),
        Some(proposal(2_000, 8.0))
    );
    assert_eq!(channel.newer_than(Some(DirectiveGeneration(2))), None);
}

#[test]
fn unbinding_stops_delivery_without_touching_the_slot() {
    let channel = ExposureProposalChannel::new();
    let receiver = Arc::new(CountingReceiver::default());
    channel.refresh_signal().bind(receiver.clone());
    channel.refresh_signal().unbind();
    assert!(!channel.refresh_signal().is_bound());
    assert_eq!(channel.publish(proposal(1_000, 4.0)).1, SignalDelivery::NotBoundYet);
    assert_eq!(receiver.signals.load(Ordering::Relaxed), 0);
    assert!(channel.latest().is_some());
}

#[test]
fn default_directive_is_automatic_whole_frame_without_an_applied_request() {
    let directive = ExposureDirective::DEFAULT;
    assert_eq!(directive.authority, ExposureAuthority::Automatic);
    assert_eq!(directive.metering, PointOfInterest::WholeFrame);
    assert_eq!(directive.lock, AeAfLock::Unlocked);
    assert_eq!(directive.applied, None);
}

#[test]
fn bind_after_missed_signals_sends_exactly_one_signal_and_bind_without_misses_sends_none() {
    let channel = ExposureProposalChannel::new();
    channel.publish(proposal(1_000, 4.0));
    channel.publish(proposal(2_000, 8.0));
    let receiver = Arc::new(CountingReceiver::default());
    channel.refresh_signal().bind(receiver.clone());
    assert_eq!(
        receiver.signals.load(Ordering::Relaxed),
        1,
        "два пропуска — один досланный толчок"
    );
    assert_eq!(
        channel.refresh_signal().missed_before_bind(),
        2,
        "общий счётчик остался для диагностики"
    );

    // Повторная привязка без новых пропусков ничего не досылает.
    let second = Arc::new(CountingReceiver::default());
    channel.refresh_signal().bind(second.clone());
    assert_eq!(second.signals.load(Ordering::Relaxed), 0);

    // Пропуск после отвязки снова даёт ровно один толчок следующему получателю.
    channel.refresh_signal().unbind();
    channel.publish(proposal(3_000, 2.0));
    let third = Arc::new(CountingReceiver::default());
    channel.refresh_signal().bind(third.clone());
    assert_eq!(third.signals.load(Ordering::Relaxed), 1);
}

#[test]
fn product_reach_clamps_log2_into_its_segment_and_unbounded_leaves_it() {
    let reach = ProductReach::Between {
        darkest: product(0.25),
        brightest: product(4.0),
    };
    assert_eq!(reach.clamp_log2(-5.0), -2.0, "темнее предела — на предел");
    assert_eq!(reach.clamp_log2(7.0), 2.0, "светлее предела — на предел");
    assert_eq!(reach.clamp_log2(0.5), 0.5, "внутри отрезка — без изменений");
    assert_eq!(ProductReach::Unbounded.clamp_log2(123.0), 123.0);
}
