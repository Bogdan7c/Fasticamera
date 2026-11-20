//! Частота кадров по камерам: желаемый режим остаётся запрошенным (4:3@60), фронтальная получает
//! свои 30, а возврат на основную снова просит 60 (закрывает ограничение из `results/05.md`).

use camera_contract::ids::LensFacing;

use super::lens_support::LensRig;
use super::*;
use crate::backend::{ConfigureFailure, UnsupportedStream};

fn not_achievable_60() -> ConfigureFailure {
    ConfigureFailure::Unsupported(UnsupportedStream::FrameRateNotAchievable {
        requested: TargetFrameRate::new(60).expect("60 к/с"),
    })
}

fn configured_rates(calls: &[BackendCall]) -> Vec<(String, u32)> {
    calls
        .iter()
        .filter_map(|call| match call {
            BackendCall::ConfigureSession { stream, .. } => {
                Some((stream.camera.to_string(), stream.frame_rate.frames_per_second()))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn front_gets_30_with_a_degraded_event_and_the_back_camera_gets_60_again() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.fake.push_configure_failure(not_achievable_60());
    lens.intent(LensFacing::Front, 1.0);

    let events = lens.rig.fresh_events();
    assert!(events.iter().any(|event| matches!(event,
        CameraEvent::StreamDegraded { requested, fallback, .. }
            if requested.frame_rate.frames_per_second() == 60 && fallback.frame_rate.frames_per_second() == 30)));
    assert_eq!(
        configured_rates(&lens.rig.fake.take_calls()),
        vec![("1".to_owned(), 60), ("1".to_owned(), 30)],
        "фронт: запрошенные 60 не прошли, настроено 30"
    );
    assert_eq!(lens.rig.state_name(), "streaming");
    assert_eq!(
        lens.rig.engine.status().stream.target_frame_duration,
        Some(TargetFrameRate::new(30).expect("30 к/с").frame_duration())
    );

    // Назад на основную: снова 60, без отката.
    lens.intent(LensFacing::Back, 1.0);
    assert_eq!(
        configured_rates(&lens.rig.fake.take_calls()),
        vec![("2".to_owned(), 60)]
    );
    let events = lens.rig.fresh_events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CameraEvent::StreamDegraded { .. }))
    );
    assert_eq!(
        lens.rig
            .engine
            .status()
            .configured
            .expect("сессия")
            .stream
            .frame_rate
            .frames_per_second(),
        60
    );
}

#[test]
fn second_visit_to_the_front_goes_straight_to_its_ceiling_with_an_explicit_event() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.fake.push_configure_failure(not_achievable_60());
    lens.intent(LensFacing::Front, 1.0);
    lens.intent(LensFacing::Back, 1.0);
    lens.rig.fresh_events();
    lens.rig.fake.take_calls();

    lens.intent(LensFacing::Front, 1.0);
    assert_eq!(
        configured_rates(&lens.rig.fake.take_calls()),
        vec![("1".to_owned(), 30)],
        "потолок фронта известен: лишней неудачной попытки на 60 нет"
    );
    let events = lens.rig.fresh_events();
    assert!(events.iter().any(|event| matches!(event,
        CameraEvent::StreamLimitedByCamera { requested, effective, camera }
            if requested.frame_rate.frames_per_second() == 60
                && effective.frame_rate.frames_per_second() == 30
                && camera.as_str() == "1")));
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, CameraEvent::StreamDegraded { .. }))
    );
}

#[test]
fn ceiling_of_one_camera_does_not_leak_to_another_and_a_new_configure_forgets_it() {
    let mut lens = LensRig::streaming_on_wide();
    lens.rig.fake.push_configure_failure(not_achievable_60());
    lens.intent(LensFacing::Front, 1.0);
    lens.intent(LensFacing::Back, 1.0);
    lens.intent(LensFacing::Back, 0.6);
    assert_eq!(
        configured_rates(&lens.rig.fake.take_calls()).last(),
        Some(&("3".to_owned(), 60)),
        "ширик — со своими 60, фронтовые 30 к нему не прилипли"
    );

    // Новая команда режима проверяет потолки заново.
    lens.intent(LensFacing::Front, 1.0);
    lens.rig.send(CameraCommand::Configure {
        mode: mode_raw10_4x3_at(30),
    });
    lens.rig.send(CameraCommand::Configure {
        mode: mode_raw10_4x3_at(60),
    });
    lens.rig.fake.take_calls();
    lens.rig.fresh_events();
    lens.intent(LensFacing::Back, 1.0);
    lens.intent(LensFacing::Front, 1.0);
    assert_eq!(
        configured_rates(&lens.rig.fake.take_calls()).last(),
        Some(&("1".to_owned(), 60)),
        "память потолка сброшена Configure: фронт снова пробует 60 (fake это позволяет)"
    );
}
