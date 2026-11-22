#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use super::*;

/// Секция экрана с заданным пределом очереди; сроки — по умолчанию.
fn screen_with_limit(max_saving_shots: usize) -> CaptureConfig {
    CaptureConfig {
        max_saving_shots,
        ..CaptureConfig::default()
    }
}

#[test]
fn defaults_pass_with_default_screen_limit() {
    PhotoConfig::default().validate(&screen_with_limit(10)).unwrap();
}

#[test]
fn partial_section_keeps_other_defaults() {
    let config: PhotoConfig = serde_json::from_str(r#"{"service":{"jpeg_quality":80}}"#).unwrap();
    assert_eq!(config.service.jpeg_quality, 80);
    assert_eq!(
        config.service.max_pending_stills,
        StillServiceConfig::default().max_pending_stills
    );
    assert_eq!(config.journal_capacity, 64);
}

#[test]
fn typo_in_field_name_is_rejected() {
    assert!(serde_json::from_str::<PhotoConfig>(r#"{"service":{"jpeg_qualty":80}}"#).is_err());
}

#[test]
fn invalid_service_values_are_reported_as_service_errors() {
    let config = PhotoConfig {
        service: StillServiceConfig {
            heif_quality: 101,
            ..StillServiceConfig::default()
        },
        ..PhotoConfig::default()
    };
    assert!(matches!(
        config.validate(&screen_with_limit(10)),
        Err(PhotoConfigError::Service(_))
    ));
}

#[test]
fn journal_capacity_bounds_are_checked() {
    for value in [0, 70_000] {
        let config = PhotoConfig {
            journal_capacity: value,
            ..PhotoConfig::default()
        };
        assert_eq!(
            config.validate(&screen_with_limit(10)),
            Err(PhotoConfigError::JournalCapacity { value })
        );
    }
}

#[test]
fn service_limit_below_screen_saving_limit_is_rejected() {
    let config = PhotoConfig {
        service: StillServiceConfig {
            max_pending_stills: 4,
            ..StillServiceConfig::default()
        },
        ..PhotoConfig::default()
    };
    assert_eq!(
        config.validate(&screen_with_limit(10)),
        Err(PhotoConfigError::ServiceLimitBelowScreenLimit {
            service_limit: 4,
            screen_limit: 10
        })
    );
    assert!(config.validate(&screen_with_limit(4)).is_ok(), "равенство допустимо");
}

#[test]
fn ledger_capacity_exceeds_pending_limit() {
    let config = PhotoConfig::default();
    assert!(config.ledger_capacity() > config.service.max_pending_stills);
}

#[test]
fn screen_capture_timeout_must_exceed_the_service_capture_timeout() {
    let screen_ms = CaptureConfig::default().capture_timeout_ms;
    let service = |capture_timeout_ms| PhotoConfig {
        service: StillServiceConfig {
            capture_timeout_ms,
            ..StillServiceConfig::default()
        },
        ..PhotoConfig::default()
    };
    let screen = CaptureConfig::default();
    let equal = service(u64::from(screen_ms));
    assert_eq!(
        equal.validate(&screen),
        Err(PhotoConfigError::ScreenCaptureTimeoutNotAboveService {
            screen_ms: u64::from(screen_ms),
            service_ms: u64::from(screen_ms)
        })
    );
    assert!(service(u64::from(screen_ms) - 1).validate(&screen).is_ok());
}

#[test]
fn screen_saving_timeout_must_exceed_service_develop_plus_heif_finish() {
    let config = PhotoConfig::default();
    let service_total = config.service.develop_timeout_ms + config.service.heif_finish_timeout_ms;
    let screen = |saving_timeout_ms| CaptureConfig {
        saving_timeout_ms,
        ..CaptureConfig::default()
    };
    assert_eq!(
        config.validate(&screen(u32::try_from(service_total).unwrap())),
        Err(PhotoConfigError::ScreenSavingTimeoutNotAboveService {
            screen_ms: service_total,
            service_ms: service_total
        })
    );
    assert!(
        config
            .validate(&screen(u32::try_from(service_total).unwrap() + 1))
            .is_ok()
    );
}

#[test]
fn default_screen_and_service_timeouts_leave_a_margin() {
    // Не просто «больше», а с запасом на такт и накладные расходы: экран не должен срабатывать раньше службы.
    let screen = CaptureConfig::default();
    let service = StillServiceConfig::default();
    assert!(u64::from(screen.capture_timeout_ms) >= service.capture_timeout_ms + 500);
    assert!(u64::from(screen.saving_timeout_ms) >= service.develop_timeout_ms + service.heif_finish_timeout_ms + 5_000);
}

#[test]
fn defaults_pass_the_render_timeout_check() {
    // Умолчания двух crate-ов согласованы: ломается, если кто-то уменьшит срок цели рендера или
    // поднимет очередь службы, не пересчитав второе.
    PhotoConfig::default()
        .validate_against_render(&StillDevelopConfig::default())
        .unwrap();
}

#[test]
fn render_target_timeout_below_queue_wait_is_rejected() {
    let render = StillDevelopConfig {
        target_timeout_ms: 10_000,
        ..StillDevelopConfig::default()
    };
    // 9 снимков впереди × 2 с = 18 с ожидания последнего; 10 с не хватает.
    assert_eq!(
        PhotoConfig::default().validate_against_render(&render),
        Err(PhotoConfigError::RenderTargetTimeoutBelowQueueWait {
            render_ms: 10_000,
            queue_wait_ms: 18_000,
            pending: 10,
        })
    );
}

#[test]
fn shorter_service_queue_needs_a_shorter_render_wait() {
    // Три ждущих снимка: впереди двое, ждать 4 с; 5 с хватает.
    let config = PhotoConfig {
        service: StillServiceConfig {
            max_pending_stills: 3,
            ..StillServiceConfig::default()
        },
        ..PhotoConfig::default()
    };
    let render = StillDevelopConfig {
        target_timeout_ms: 5_000,
        ..StillDevelopConfig::default()
    };
    assert_eq!(config.validate_against_render(&render), Ok(()));
}

#[test]
fn render_target_timeout_not_below_service_develop_timeout_is_rejected() {
    let render = StillDevelopConfig {
        target_timeout_ms: 30_000,
        ..StillDevelopConfig::default()
    };
    assert_eq!(
        PhotoConfig::default().validate_against_render(&render),
        Err(PhotoConfigError::RenderTargetTimeoutNotBelowService {
            render_ms: 30_000,
            service_ms: 30_000,
        })
    );
}
