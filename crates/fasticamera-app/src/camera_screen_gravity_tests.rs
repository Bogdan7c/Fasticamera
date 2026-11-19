//! Тесты подачи гравитации: прореживание по кадрам, только новые показания, отсутствие показаний, отказ датчика.

use std::sync::Arc;

use shell_contract::clock::BoottimeNanos;
use shell_contract::gravity::{GravitySample, GravitySlot};

use super::*;

fn vector(y: f32) -> GravityVector {
    GravityVector { x: 0.0, y, z: 0.0 }
}

fn publish(slot: &GravitySlot, time_ns: i64, y: f32) {
    slot.publish(GravitySample {
        time: BoottimeNanos::from_nanos(time_ns),
        vector: vector(y),
    });
}

fn feed(every_n: u32) -> (GravityFeed, Arc<GravitySlot>) {
    let slot = Arc::new(GravitySlot::new());
    let subscription = GravitySubscription::new(Arc::clone(&slot), Box::new(()));
    (GravityFeed::new(subscription, every_n), slot)
}

#[test]
fn first_frame_polls_immediately_then_every_nth_frame() {
    let (mut feed, slot) = feed(4);
    publish(&slot, 1, 9.0);
    assert_eq!(feed.poll(), GravityPoll::Fresh(vector(9.0)), "первый кадр — сразу");
    publish(&slot, 2, 8.0);
    for _ in 0..3 {
        assert_eq!(feed.poll(), GravityPoll::NotDue);
    }
    assert_eq!(
        feed.poll(),
        GravityPoll::Fresh(vector(8.0)),
        "на четвёртом кадре после опроса"
    );
}

#[test]
fn an_unchanged_sample_is_not_resent() {
    let (mut feed, slot) = feed(1);
    publish(&slot, 1, 9.0);
    assert_eq!(feed.poll(), GravityPoll::Fresh(vector(9.0)));
    assert_eq!(feed.poll(), GravityPoll::Unchanged);
    publish(&slot, 2, 9.0);
    assert_eq!(
        feed.poll(),
        GravityPoll::Fresh(vector(9.0)),
        "то же значение, но новое показание"
    );
}

#[test]
fn a_silent_sensor_reports_no_sample_yet() {
    let (mut feed, _slot) = feed(1);
    assert_eq!(feed.poll(), GravityPoll::NoSampleYet);
}

#[test]
fn zero_interval_is_treated_as_every_frame() {
    let (mut feed, slot) = feed(0);
    publish(&slot, 1, 1.0);
    assert_eq!(feed.poll(), GravityPoll::Fresh(vector(1.0)));
    publish(&slot, 2, 2.0);
    assert_eq!(feed.poll(), GravityPoll::Fresh(vector(2.0)));
}

#[test]
fn a_failed_sensor_start_is_an_explicit_reason() {
    let error = start_gravity_feed(Err(SensorSourceError::SensorAbsent { sensor: "gravity" }), 4);
    assert!(
        matches!(&error, Err(reason) if reason.contains("gravity")),
        "отказ датчика не превращается в тихую подачу: {:?}",
        error.as_ref().err()
    );
}
