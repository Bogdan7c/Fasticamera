//! Тесты истории параметров кадров: свой кадр, ближайший того же потока, чужой поток не подходит, ёмкость.

use std::sync::Arc;

use camera_contract::frame_passport::StreamPassport;
use isp_model::color::chromaticity::Chromaticity;
use isp_model::exposure::ExposureGain;

use super::*;
use crate::vulkan_backend::camera::still::test_support::{MAIN, stamp, stream_passport};

fn params(stream: &Arc<StreamPassport>, white_x: f64, gain_ev: f32) -> FrameStillParams {
    FrameStillParams {
        stream: Arc::clone(stream),
        white: Chromaticity { x: white_x, y: 0.33 },
        gain: ExposureGain::from_ev(gain_ev).expect("подтяжка"),
    }
}

fn white_of(lookup: &ParamsLookup) -> Option<f64> {
    match lookup {
        ParamsLookup::Own(params) | ParamsLookup::NearestOfSameStream { params, .. } => Some(params.white.x),
        ParamsLookup::Missing => None,
    }
}

#[test]
fn own_record_wins_over_a_nearer_neighbour_and_carries_its_own_values() {
    let stream = Arc::new(stream_passport(MAIN));
    let mut history = FrameParamsHistory::new(8);
    history.record(stamp(100), params(&stream, 0.30, 0.0));
    history.record(stamp(200), params(&stream, 0.35, 0.5));
    match history.lookup(stamp(200), &stream) {
        ParamsLookup::Own(found) => {
            assert!((found.white.x - 0.35).abs() < 1e-12);
            assert!((found.gain.ev() - 0.5).abs() < 1e-6, "k своего кадра, а не соседнего");
        }
        other => panic!("ожидали запись самого кадра, получили {other:?}"),
    }
}

#[test]
fn a_frame_without_its_own_record_takes_the_nearest_of_the_same_stream() {
    let stream = Arc::new(stream_passport(MAIN));
    let mut history = FrameParamsHistory::new(8);
    history.record(stamp(100), params(&stream, 0.30, 0.0));
    history.record(stamp(300), params(&stream, 0.40, 0.0));
    match history.lookup(stamp(260), &stream) {
        ParamsLookup::NearestOfSameStream { params, distance_nanos } => {
            assert!((params.white.x - 0.40).abs() < 1e-12);
            assert_eq!(distance_nanos, 40);
        }
        other => panic!("ожидали ближайшую запись, получили {other:?}"),
    }
}

#[test]
fn equal_distance_prefers_the_earlier_frame() {
    let stream = Arc::new(stream_passport(MAIN));
    let mut history = FrameParamsHistory::new(8);
    history.record(stamp(100), params(&stream, 0.30, 0.0));
    history.record(stamp(200), params(&stream, 0.40, 0.0));
    assert_eq!(white_of(&history.lookup(stamp(150), &stream)), Some(0.30));
}

#[test]
fn another_stream_never_donates_parameters() {
    let ours = Arc::new(stream_passport(MAIN));
    // Равный по содержимому, но другой поток (другой `Arc`): уровни и матрицы чужие (решение 42).
    let other = Arc::new(stream_passport(MAIN));
    let mut history = FrameParamsHistory::new(8);
    history.record(stamp(100), params(&other, 0.30, 0.0));
    assert!(matches!(history.lookup(stamp(100), &ours), ParamsLookup::Missing));
    assert!(matches!(history.lookup(stamp(110), &ours), ParamsLookup::Missing));
}

#[test]
fn capacity_evicts_the_oldest_records() {
    let stream = Arc::new(stream_passport(MAIN));
    let mut history = FrameParamsHistory::new(2);
    for (index, white) in [(1, 0.31), (2, 0.32), (3, 0.33)] {
        history.record(stamp(index), params(&stream, white, 0.0));
    }
    assert_eq!(history.len(), 2);
    // Запись кадра 1 вытеснена: он получает ближайшую (кадр 2), а не свою.
    assert!(matches!(
        history.lookup(stamp(1), &stream),
        ParamsLookup::NearestOfSameStream { .. }
    ));
    assert_eq!(white_of(&history.lookup(stamp(1), &stream)), Some(0.32));
    history.clear();
    assert!(history.is_empty());
    assert!(matches!(history.lookup(stamp(2), &stream), ParamsLookup::Missing));
}
