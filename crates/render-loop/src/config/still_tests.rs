//! Тесты секции `still`: умолчания проходят проверку, JSON без ключей не ломается, лишние ключи отвергаются,
//! каждая граница диапазона ловится именно своим полем.

use super::*;

#[test]
fn defaults_are_valid_and_carry_the_owner_decisions() {
    let config = StillDevelopConfig::default();
    assert_eq!(config.validate(), Ok(()));
    // Решение 140: очередь ждущих снимков — 10; решение 137: одна полоса на кадр видоискателя.
    assert_eq!(config.queue_capacity, 10);
    assert_eq!(config.slices_per_frame, 1);
}

#[test]
fn json_without_keys_is_the_default_and_partial_json_keeps_the_rest() {
    let empty: StillDevelopConfig = serde_json::from_str("{}").expect("пустая секция");
    assert_eq!(empty, StillDevelopConfig::default());
    let partial: StillDevelopConfig = serde_json::from_str(r#"{"queue_capacity": 4}"#).expect("частичная секция");
    assert_eq!(partial.queue_capacity, 4);
    assert_eq!(
        partial.target_timeout_ms,
        StillDevelopConfig::default().target_timeout_ms
    );
}

#[test]
fn unknown_keys_are_rejected() {
    assert!(serde_json::from_str::<StillDevelopConfig>(r#"{"queue_capasity": 4}"#).is_err());
}

#[test]
fn every_field_is_checked_with_its_own_name() {
    type Break = fn(&mut StillDevelopConfig);
    let cases: [(&str, Break); 11] = [
        ("queue_capacity", |c| c.queue_capacity = 0),
        ("presentation_wait_ms", |c| {
            c.presentation_wait_ms = MAX_STILL_WAIT_MS + 1
        }),
        ("target_timeout_ms", |c| c.target_timeout_ms = 0),
        ("encoder_present_timeout_ms", |c| c.encoder_present_timeout_ms = 0),
        ("capture_timeout_ms", |c| c.capture_timeout_ms = 0),
        ("develop_timeout_ms", |c| c.develop_timeout_ms = MAX_STILL_WAIT_MS + 1),
        ("max_slices_in_flight", |c| c.max_slices_in_flight = 0),
        ("retired_present_capacity", |c| c.retired_present_capacity = 0),
        ("slices_per_frame", |c| c.slices_per_frame = 0),
        ("frame_params_capacity", |c| c.frame_params_capacity = 0),
        ("present_log_capacity", |c| {
            c.present_log_capacity = MAX_STILL_LOG_CAPACITY + 1
        }),
    ];
    for (field, break_it) in cases {
        let mut config = StillDevelopConfig::default();
        break_it(&mut config);
        match config.validate() {
            Err(StillConfigError::OutOfRange { field: reported, .. }) => assert_eq!(reported, field),
            other => panic!("{field}: ожидали отказ по полю, получили {other:?}"),
        }
    }
}

#[test]
fn broken_photo_settings_are_reported_as_photo() {
    let mut config = StillDevelopConfig::default();
    config.photo.slice_rows = 3;
    assert!(matches!(config.validate(), Err(StillConfigError::Photo(_))));
}

/// Верхняя оценка «снимок впереди держит кодер», мс (та же оценка в `fasticamera-app::photo_config`).
const ESTIMATED_MS_PER_QUEUED_STILL: u32 = 2_000;

#[test]
fn default_target_timeout_covers_the_wait_of_the_last_queued_still() {
    let config = StillDevelopConfig::default();
    // Цель кодера выдаётся по мере освобождения мест: последний снимок очереди ждёт всех впереди.
    let worst_wait_ms = (config.queue_capacity - 1) * ESTIMATED_MS_PER_QUEUED_STILL;
    assert!(
        config.target_timeout_ms >= worst_wait_ms,
        "срок цели {} мс короче худшего ожидания очереди {worst_wait_ms} мс",
        config.target_timeout_ms
    );
}
