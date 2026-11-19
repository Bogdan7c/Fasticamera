#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! K8 «Плашка»: показ, срок жизни, скрытие, события ровно один раз, детерминизм по меткам времени.

use ui_core::time::{MonotonicNanos, MotionPreference, UiDuration};
use ui_kit::banner::{
    Banner, BannerError, BannerEvent, BannerHideOutcome, BannerLifetime, BannerPhase, BannerShowOutcome, HideCause,
};
use ui_kit::picker::LabelId;
use ui_kit::tokens::UiKitTokens;

mod common;
use common::*;

const LOCK: LabelId = LabelId(7);
const TIMER: LabelId = LabelId(8);

fn banner(tokens: &UiKitTokens) -> Banner {
    Banner::new(&env(tokens, MotionPreference::Full), ms(0)).unwrap()
}

fn run(banner: &mut Banner, from_ms: i64, until_ms: i64, frame_ms: f64) -> Vec<BannerEvent> {
    let mut events = Vec::new();
    let mut t = from_ms as f64;
    while t <= until_ms as f64 + 1e-9 {
        banner
            .advance(MonotonicNanos::from_nanos(1_000_000_000 + (t * 1e6) as i64))
            .unwrap();
        events.extend(banner.drain_events());
        t += frame_ms;
    }
    events
}

#[test]
fn shown_banner_fades_in_and_reports_shown_once_with_its_label() {
    let tokens = UiKitTokens::default();
    let mut banner = banner(&tokens);
    assert_eq!(banner.snapshot(ms(0)).label, None);
    assert_eq!(
        banner.show(LOCK, BannerLifetime::UntilHidden, ms(0)).unwrap(),
        BannerShowOutcome::Started
    );
    let early = banner.snapshot(ms(40));
    assert_eq!(early.phase, BannerPhase::Showing);
    assert_eq!(early.label, Some(LOCK));
    assert!(early.opacity > 0.0 && early.opacity < 1.0);
    assert!(early.scale > tokens.banner.appear_scale && early.scale < 1.0);

    assert_eq!(
        run(&mut banner, 0, 2000, 16.0),
        vec![BannerEvent::Shown { label: LOCK }]
    );
    let done = banner.snapshot(ms(2000));
    assert_eq!((done.phase, done.opacity, done.scale), (BannerPhase::Visible, 1.0, 1.0));
}

#[test]
fn timed_banner_hides_itself_and_reports_expiry_once() {
    let tokens = UiKitTokens::default();
    let mut banner = banner(&tokens);
    banner
        .show(LOCK, BannerLifetime::For(UiDuration::from_millis(2000)), ms(0))
        .unwrap();
    let events = run(&mut banner, 0, 5000, 16.0);
    assert_eq!(
        events,
        vec![
            BannerEvent::Shown { label: LOCK },
            BannerEvent::Hidden {
                label: LOCK,
                cause: HideCause::Expired
            }
        ]
    );
    let end = banner.snapshot(ms(5000));
    assert_eq!((end.phase, end.label, end.opacity), (BannerPhase::Hidden, None, 0.0));
}

#[test]
fn hiding_of_an_expired_banner_starts_at_the_deadline_not_at_the_frame() {
    let tokens = UiKitTokens::default();
    let lifetime = BannerLifetime::For(UiDuration::from_millis(2000));
    let mut slow = banner(&tokens);
    let mut fast = banner(&tokens);
    slow.show(LOCK, lifetime, ms(0)).unwrap();
    fast.show(LOCK, lifetime, ms(0)).unwrap();
    run(&mut slow, 0, 2300, 1000.0 / 60.0);
    run(&mut fast, 0, 2300, 1000.0 / 120.0);
    // Кадр, заметивший истечение, у частот разный, а положение на 2,1 с и 2,3 с — одно и то же.
    for at_ms in [2100, 2200, 2300] {
        assert_eq!(slow.snapshot(ms(at_ms)), fast.snapshot(ms(at_ms)), "на {at_ms} мс");
    }
    assert!(slow.snapshot(ms(2100)).opacity < 1.0, "скрытие уже идёт");
}

#[test]
fn until_hidden_banner_stays_until_asked_and_then_reports_requested() {
    let tokens = UiKitTokens::default();
    let mut banner = banner(&tokens);
    banner.show(LOCK, BannerLifetime::UntilHidden, ms(0)).unwrap();
    run(&mut banner, 0, 60_000, 100.0);
    assert_eq!(banner.phase(ms(60_000)), BannerPhase::Visible);
    assert_eq!(banner.hide(ms(60_000)).unwrap(), BannerHideOutcome::Started);
    assert_eq!(banner.hide(ms(60_010)).unwrap(), BannerHideOutcome::AlreadyThere);
    let events = run(&mut banner, 60_000, 62_000, 16.0);
    assert_eq!(
        events,
        vec![BannerEvent::Hidden {
            label: LOCK,
            cause: HideCause::Requested
        }]
    );
}

#[test]
fn hiding_during_the_appearance_reverses_without_a_shown_event() {
    let tokens = UiKitTokens::default();
    let mut banner = banner(&tokens);
    banner.show(LOCK, BannerLifetime::UntilHidden, ms(0)).unwrap();
    let before = banner.snapshot(ms(80)).opacity;
    assert_eq!(banner.hide(ms(80)).unwrap(), BannerHideOutcome::Reversed);
    assert!((banner.snapshot(ms(80)).opacity - before).abs() < 1e-6, "скачка нет");
    let events = run(&mut banner, 80, 3000, 16.0);
    assert_eq!(
        events,
        vec![BannerEvent::Hidden {
            label: LOCK,
            cause: HideCause::Requested
        }]
    );
}

#[test]
fn showing_again_refreshes_text_and_restarts_the_lifetime_without_restarting_the_animation() {
    let tokens = UiKitTokens::default();
    let mut banner = banner(&tokens);
    let two_seconds = BannerLifetime::For(UiDuration::from_millis(2000));
    banner.show(LOCK, two_seconds, ms(0)).unwrap();
    run(&mut banner, 0, 1500, 16.0);
    assert_eq!(
        banner.show(TIMER, two_seconds, ms(1500)).unwrap(),
        BannerShowOutcome::Refreshed
    );
    // Исходный срок (2 с) прошёл, но срок отсчитан заново: плашка ещё на экране с новым текстом.
    run(&mut banner, 1500, 3000, 16.0);
    let snapshot = banner.snapshot(ms(3000));
    assert_eq!((snapshot.phase, snapshot.label), (BannerPhase::Visible, Some(TIMER)));
    let events = run(&mut banner, 3000, 6000, 16.0);
    assert_eq!(
        events,
        vec![BannerEvent::Hidden {
            label: TIMER,
            cause: HideCause::Expired
        }]
    );
}

#[test]
fn zero_lifetime_is_an_error_and_leaves_the_banner_hidden() {
    let tokens = UiKitTokens::default();
    let mut banner = banner(&tokens);
    let result = banner.show(LOCK, BannerLifetime::For(UiDuration::ZERO), ms(0));
    assert_eq!(result, Err(BannerError::NonPositiveLifetime));
    assert_eq!(banner.phase(ms(100)), BannerPhase::Hidden);
    assert_eq!(banner.hide(ms(100)).unwrap(), BannerHideOutcome::AlreadyThere);
}

#[test]
fn relocking_right_after_full_hide_still_reports_hidden_for_the_old_label() {
    let tokens = UiKitTokens::default();
    let mut banner = banner(&tokens);
    banner.show(LOCK, BannerLifetime::UntilHidden, ms(0)).unwrap();
    run(&mut banner, 0, 1000, 16.0);
    banner.hide(ms(1000)).unwrap();
    // Скрытие давно закончилось, но ни один кадр (`advance`) его не заметил — и тут новый `show`.
    assert_eq!(
        banner.show(TIMER, BannerLifetime::UntilHidden, ms(3000)).unwrap(),
        BannerShowOutcome::Started
    );
    assert_eq!(
        run(&mut banner, 3000, 4500, 16.0),
        vec![
            BannerEvent::Hidden {
                label: LOCK,
                cause: HideCause::Requested
            },
            BannerEvent::Shown { label: TIMER },
        ]
    );
}

#[test]
fn hide_after_the_deadline_but_before_a_frame_reports_expired_not_requested() {
    let tokens = UiKitTokens::default();
    let mut banner = banner(&tokens);
    banner
        .show(LOCK, BannerLifetime::For(UiDuration::from_millis(2000)), ms(0))
        .unwrap();
    run(&mut banner, 0, 1000, 16.0);
    // Срок истёк в 2000 мс; кадров с тех пор не было, а `hide` пришёл в 3000 мс.
    assert_eq!(banner.hide(ms(3000)).unwrap(), BannerHideOutcome::AlreadyThere);
    assert_eq!(
        run(&mut banner, 3000, 4000, 16.0),
        vec![BannerEvent::Hidden {
            label: LOCK,
            cause: HideCause::Expired
        }]
    );
}
