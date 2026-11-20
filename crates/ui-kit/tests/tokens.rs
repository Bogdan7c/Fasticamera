#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Токены: Reduce Motion, таблица хаптики по `haptics.md`, значения по `motion.md`, serde.

use shell_contract::haptics::HapticPattern;
use ui_core::scroll::{AxisGeometry, ReleaseOutcome, ScrollAxis, ScrollTransition};
use ui_core::spring::{Spring, SpringAnimation, SpringStart};
use ui_core::time::MotionPreference;
use ui_kit::tokens::{
    DensityTokens, HapticEvent, HapticSetting, HapticTable, MotionToken, MotionTokens, SnapRole, TokenError,
    UiKitTokens,
};

mod common;
use common::ms;

#[test]
fn measured_and_doc_springs_match_the_specification_numbers() {
    let tokens = MotionTokens::default();
    // motion.md 4.1 [measured]
    assert_eq!(tokens.token(MotionToken::CameraSnap).duration_s, 0.85);
    assert_eq!(tokens.token(MotionToken::CameraSnap).bounce, -0.13);
    assert_eq!(tokens.token(MotionToken::CameraSheet).duration_s, 0.45);
    assert_eq!(tokens.token(MotionToken::CameraSheet).bounce, -0.11);
    // motion.md 1.5 [doc]
    assert_eq!(tokens.token(MotionToken::Smooth).bounce, 0.0);
    assert_eq!(tokens.token(MotionToken::Snappy).bounce, 0.15);
    assert_eq!(tokens.token(MotionToken::Bouncy).bounce, 0.3);
    for token in [MotionToken::Smooth, MotionToken::Snappy, MotionToken::Bouncy] {
        assert_eq!(tokens.token(token).duration_s, 0.5, "{token:?}");
    }
}

#[test]
fn card_motion_tokens_match_the_measured_ios27_durations_and_never_overshoot() {
    let tokens = MotionTokens::default();
    // [measured: C t=47.93–48.12] растворение карточки 0,2 с; [measured: C t=37.8–38.1] морф ≈ 0,3 с.
    assert_eq!(tokens.token(MotionToken::CardAppear).duration_s, 0.2);
    assert_eq!(tokens.token(MotionToken::CardAppear).bounce, 0.0);
    assert_eq!(tokens.token(MotionToken::CardMorph).duration_s, 0.3);
    assert_eq!(tokens.token(MotionToken::CardMorph).bounce, 0.0);
    assert_eq!(MotionToken::CardAppear.name(), "card.appear");
    assert_eq!(MotionToken::CardMorph.name(), "card.morph");
    // Обе пружины строятся в обоих режимах движения (Reduce Motion не ломает их).
    for motion in [MotionPreference::Full, MotionPreference::Reduced] {
        tokens.spring(MotionToken::CardAppear, motion).unwrap();
        tokens.spring(MotionToken::CardMorph, motion).unwrap();
    }
}

#[test]
fn reduced_motion_removes_overshoot_from_every_spring() {
    let tokens = MotionTokens::default();
    for token in MotionToken::ALL {
        let full = tokens.spring(token, MotionPreference::Full).unwrap();
        let reduced = tokens.spring(token, MotionPreference::Reduced).unwrap();
        let (full_shape, reduced_shape) = (full.perceptual().unwrap(), reduced.perceptual().unwrap());
        assert!(
            reduced_shape.bounce <= 1e-12,
            "{token:?}: bounce {}",
            reduced_shape.bounce
        );
        // Плавность не пропала: длительность прежняя.
        assert!(
            (reduced_shape.duration_s - full_shape.duration_s).abs() < 1e-12,
            "{token:?}"
        );
        // Пружина без перелёта вообще не меняется (измеренные пружины камеры).
        if full_shape.bounce <= 0.0 {
            assert_eq!(reduced, full, "{token:?}: без перелёта — без изменений");
        }
    }
}

#[test]
fn reduced_spring_really_does_not_overshoot_but_full_bouncy_does() {
    let tokens = MotionTokens::default();
    let peak = |motion: MotionPreference| {
        let spring = tokens.spring(MotionToken::Bouncy, motion).unwrap();
        let animation = SpringAnimation::start(SpringStart {
            spring,
            from: 0.0,
            to: 100.0,
            initial_velocity: 0.0,
            start_time: ms(0),
            rest: Default::default(),
        })
        .unwrap();
        (0..2000)
            .map(|step| animation.sample(ms(step)).value)
            .fold(f64::MIN, f64::max)
    };
    assert!(peak(MotionPreference::Full) > 103.0, "bouncy обязан перелетать");
    assert!(
        peak(MotionPreference::Reduced) <= 100.0 + 1e-9,
        "Reduced перелетать не должен"
    );
}

/// Роль задаёт и пружину прилипания, и политику выбора точки. Проверяется на реальной прокрутке:
/// траектория программной прокрутки совпадает с независимо построенной пружиной токена роли
/// (карусель — `camera.snap`, шкала — `scale.snap`), а бросок выбирает точки по политике роли.
#[test]
fn scroll_config_follows_the_role() {
    let tokens = MotionTokens::default();
    let geometry = AxisGeometry {
        min_offset: 0.0,
        max_offset: 300.0,
        viewport_extent: 300.0,
    };
    let axis_for = |role: SnapRole| {
        let config = tokens.scroll_config(role).unwrap();
        let mut axis = ScrollAxis::new(&config, geometry, 0.0).unwrap();
        axis.set_snap_points(&[0.0, 100.0, 200.0, 300.0], ms(0)).unwrap();
        axis
    };
    let expected_trajectory = |token: MotionToken| {
        let spring = Spring::from_duration_bounce(tokens.token(token).duration_s, tokens.token(token).bounce).unwrap();
        SpringAnimation::start(SpringStart {
            spring,
            from: 0.0,
            to: 100.0,
            initial_velocity: 0.0,
            start_time: ms(0),
            rest: Default::default(),
        })
        .unwrap()
    };

    let mut trajectories = Vec::new();
    for (role, token) in [
        (SnapRole::Carousel, MotionToken::CameraSnap),
        (SnapRole::Scale, MotionToken::ScaleSnap),
    ] {
        let mut axis = axis_for(role);
        axis.scroll_to(100.0, ScrollTransition::Animated, ms(0)).unwrap();
        let expected = expected_trajectory(token);
        let mut samples = Vec::new();
        for at in [100, 250, 400, 700] {
            let got = axis.sample(ms(at)).offset;
            assert!(
                (got - expected.sample(ms(at)).value).abs() < 1e-9,
                "{role:?} @ {at} мс: {got}"
            );
            samples.push(got);
        }
        trajectories.push(samples);
    }
    assert_ne!(trajectories[0], trajectories[1], "у ролей разные пружины прилипания");

    // Сильный бросок: карусель листает на соседний (индекс 1), шкала летит к дальнему упору (индекс 3).
    for (role, expected_index) in [(SnapRole::Carousel, 1), (SnapRole::Scale, 3)] {
        let mut axis = axis_for(role);
        axis.begin_drag(ms(0));
        let outcome = axis.release(5_000.0, ms(10)).unwrap();
        assert!(
            matches!(outcome, ReleaseOutcome::SnapsToPoint { index, .. } if index == expected_index),
            "{role:?}: {outcome:?}"
        );
    }
}

#[test]
fn haptic_table_matches_haptics_md_section_3() {
    let table = HapticTable::default();
    let expected = [
        (HapticEvent::ScaleTick, Some(HapticPattern::SelectionTick)),
        (HapticEvent::ScaleReference, Some(HapticPattern::RigidImpact)),
        (HapticEvent::CarouselSelection, Some(HapticPattern::SelectionTick)),
        (HapticEvent::LockEngaged, Some(HapticPattern::MediumImpact)),
        (HapticEvent::OperationFailed, Some(HapticPattern::Error)),
        // Кнопки и затвор — без вибрации.
        (HapticEvent::ButtonTap, None),
        (HapticEvent::ShutterRelease, None),
        (HapticEvent::MenuToggle, None),
        (HapticEvent::TimerCountdown, None),
    ];
    assert_eq!(
        expected.len(),
        HapticEvent::ALL.len(),
        "таблица теста покрывает все события"
    );
    for (event, pattern) in expected {
        assert_eq!(table.pattern_for(event), pattern, "{event:?}");
    }
}

#[test]
fn haptic_setting_reads_shell_patterns_from_config() {
    // Любой паттерн оболочки задаётся в конфиге своим serde-именем — копии списка нет.
    for pattern in HapticPattern::ALL {
        let json = serde_json::json!({ "play": pattern.name() });
        let setting: HapticSetting = serde_json::from_value(json).expect("паттерн из конфига");
        assert_eq!(setting.pattern(), Some(pattern));
    }
    let silent: HapticSetting = serde_json::from_value(serde_json::json!("silent")).expect("тишина");
    assert_eq!(silent.pattern(), None);
    assert!(serde_json::from_value::<HapticSetting>(serde_json::json!({ "play": "no_such" })).is_err());
}

#[test]
fn tokens_roundtrip_through_json_and_reject_unknown_fields() {
    let tokens = UiKitTokens::default();
    tokens.validate().unwrap();
    let json = serde_json::to_string(&tokens).unwrap();
    let restored: UiKitTokens = serde_json::from_str(&json).unwrap();
    assert_eq!(restored, tokens);
    assert!(serde_json::from_str::<UiKitTokens>(r#"{"nonsense": 1}"#).is_err());
    // Пустой конфиг = умолчания.
    assert_eq!(serde_json::from_str::<UiKitTokens>("{}").unwrap(), tokens);
}

#[test]
fn validate_rejects_broken_tokens() {
    let mut tokens = UiKitTokens::default();
    tokens.motion.camera_snap.duration_s = 0.0;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::InvalidSpring {
            name: "camera.snap",
            ..
        })
    ));
    let mut tokens = UiKitTokens::default();
    tokens.density = DensityTokens {
        regular_points_per_unit: -1.0,
        ..DensityTokens::default()
    };
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::NonPositiveSize {
            name: "regular_points_per_unit",
            ..
        })
    ));
    let mut tokens = UiKitTokens::default();
    tokens.density.magnet_capture_points = f64::NAN;
    assert!(tokens.validate().is_err());
    let mut tokens = UiKitTokens::default();
    tokens.density.press_tracking_slop_points = 0.0;
    assert!(matches!(
        tokens.validate(),
        Err(TokenError::NonPositiveSize {
            name: "press_tracking_slop_points",
            ..
        })
    ));
}
