//! Границы виджетов: конфиг раскладки, геометрия, поворот значков, учёт эха шкал, кривая затемнения, отсчёт.

use shell_contract::haptics::HapticPattern;
use ui_core::scene::{SceneSettings, SurfaceGeometry, UiScene};
use ui_core::spring::Spring;
use ui_core::time::MonotonicNanos;
use ui_kit::scale::ScaleEvent;
use ui_kit::tokens::UiKitTokens;

use super::capture_blink::darkness_after;
use super::countdown::seconds_left;
use super::exposure_link::{FingerDrive, format_ev};
use super::geometry::ScreenGeometry;
use super::kit::SceneKit;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::layout::ScreenLayout;
use crate::settings::AspectRatio;
use crate::state::ScreenState;
use crate::widgets_config::{OverlayLayout, WidgetConfigError, WidgetLayoutConfig};
use shell_contract::surface::SurfaceSize;
use ui_core::geometry::Insets;

fn our_geometry() -> ScreenGeometry {
    let layout = ScreenLayout::new(
        &ScreenConfig::default().layout,
        SurfaceSize {
            width_px: 1080,
            height_px: 2412,
        },
    )
    .expect("раскладка");
    ScreenGeometry::new(&layout, &WidgetLayoutConfig::default(), &UiKitTokens::default())
}

#[test]
fn widget_config_default_is_valid_and_rejects_nonsense() {
    assert_eq!(WidgetLayoutConfig::default().validate(), Ok(()));
    let mut broken = WidgetLayoutConfig::default();
    broken.sheet.item_pitch_pt = -1.0;
    assert!(matches!(
        broken.validate(),
        Err(WidgetConfigError::NotPositive {
            field: "sheet.item_pitch_pt",
            ..
        })
    ));
    let mut no_columns = WidgetLayoutConfig::default();
    no_columns.sheet.columns = 0;
    assert_eq!(no_columns.validate(), Err(WidgetConfigError::NoMenuColumns));
    let mut bad_alpha = WidgetLayoutConfig::default();
    bad_alpha.overlay.grid_line_alpha = 1.5;
    assert!(matches!(
        bad_alpha.validate(),
        Err(WidgetConfigError::NotFraction { .. })
    ));
    // Новые числа экрана и лимиты проверяются так же, как остальные.
    let no_backoff = WidgetLayoutConfig {
        stage_rebuild_backoff_ms: 0.0,
        ..WidgetLayoutConfig::default()
    };
    assert!(matches!(
        no_backoff.validate(),
        Err(WidgetConfigError::NotPositive {
            field: "stage_rebuild_backoff_ms",
            ..
        })
    ));
    let no_text_cache = WidgetLayoutConfig {
        text_run_cache_capacity: 0,
        ..WidgetLayoutConfig::default()
    };
    assert!(no_text_cache.validate().is_err());
    let bad_dot = WidgetLayoutConfig {
        bottom: crate::widgets_config::BottomLayout {
            thumbnail_dot_scale: 1.5,
            ..Default::default()
        },
        ..WidgetLayoutConfig::default()
    };
    assert!(matches!(bad_dot.validate(), Err(WidgetConfigError::NotFraction { .. })));
    let bad_angle = WidgetLayoutConfig {
        strip: crate::widgets_config::StripLayout {
            raw_strike_angle_rad: f32::NAN,
            ..Default::default()
        },
        ..WidgetLayoutConfig::default()
    };
    assert!(matches!(bad_angle.validate(), Err(WidgetConfigError::NotFinite { .. })));
    // Ошибка раскладки виджетов доходит до проверки конфига экрана и не даёт создать состояние.
    let config = ScreenConfig {
        widgets: broken,
        ..ScreenConfig::default()
    };
    assert!(ScreenState::new(&config).is_err());
}

#[test]
fn widget_config_rejects_unknown_fields_and_round_trips() {
    let config = WidgetLayoutConfig::default();
    let json = serde_json::to_string(&config).expect("json");
    assert_eq!(
        serde_json::from_str::<WidgetLayoutConfig>(&json).expect("разбор"),
        config
    );
    assert!(serde_json::from_str::<WidgetLayoutConfig>(r#"{"strip": {"nope": 1}}"#).is_err());
}

#[test]
fn geometry_is_bottom_anchored_with_iphone_numbers() {
    let geometry = our_geometry();
    let scale = 1080.0 / 440.0;
    // `layout.md` 4.3: затвор по центру на y 828,6 pt (≈ 2034 px), капсула 924,5 pt, значки 131,6 pt.
    assert!((geometry.shutter().center().y - 828.6).abs() < 0.1);
    assert!((geometry.shutter().center().y * scale - 2034.0).abs() < 1.0);
    assert!((geometry.capsule().center().y - 924.5).abs() < 0.1);
    assert!((geometry.strip_icon(0, 3).center().y - 131.6).abs() < 0.1);
    // Ряд значков из трёх: центры 169,5 / 220,0 / 272,6 ± 1,3 (измерено, `layout.md` 2.2).
    let xs: Vec<f64> = (0..3).map(|i| geometry.strip_icon(i, 3).center().x).collect();
    for (actual, expected) in xs.iter().zip([169.5, 220.0, 272.6]) {
        assert!((actual - expected).abs() < 1.3, "{xs:?}");
    }
    // Шторка стоит в пределах экрана, пункты сетки внутри шторки, 3 колонки и шаг 115.
    let sheet = geometry.sheet();
    assert!(sheet.min_y() >= 0.0 && sheet.max_y() <= geometry.screen().max_y());
    let (first, second, fourth) = (geometry.menu_item(0), geometry.menu_item(1), geometry.menu_item(3));
    assert!((second.min_x() - first.min_x() - 115.0).abs() < 1e-9);
    assert!((fourth.min_y() - first.min_y() - 115.0).abs() < 1e-9);
    assert!(sheet.contains(first.center()) && sheet.contains(geometry.menu_item(8).center()));
    // Кадр берётся из раскладки (владелец геометрии — `ScreenLayout`).
    assert!(
        geometry.frame(AspectRatio::SixteenByNine).size.height > geometry.frame(AspectRatio::FourByThree).size.height
    );
}

fn empty_kit() -> SceneKit {
    let surface = SurfaceGeometry {
        width_px: 1080,
        height_px: 2412,
        safe_area: Insets::ZERO,
    };
    SceneKit::new(UiScene::new(surface, SceneSettings::default()).expect("сцена"))
}

#[test]
fn icon_rotation_takes_the_shortest_way_around() {
    let mut kit = empty_kit();
    let spring = Spring::from_duration_bounce(0.5, 0.0).expect("пружина");
    let now = MonotonicNanos::from_nanos(0);
    assert!(kit.rotate_icons(-90.0, spring, now).expect("поворот"));
    assert!((kit.rotation_target_rad() + std::f64::consts::FRAC_PI_2).abs() < 1e-9);
    // С −90° на 180°: 90° в ту же сторону (в −180°), а не 270° назад.
    assert!(kit.rotate_icons(180.0, spring, now).expect("поворот"));
    assert!(
        (kit.rotation_target_rad() + std::f64::consts::PI).abs() < 1e-9,
        "{}",
        kit.rotation_target_rad()
    );
    // Ту же ориентацию повторно — без нового движения.
    assert!(!kit.rotate_icons(180.0, spring, now).expect("поворот"));
}

#[test]
fn finger_drive_turns_only_finger_driven_value_changes_into_actions() {
    let state = ScreenState::new(&ScreenConfig::default()).expect("состояние");
    let config = ScreenConfig::default();
    let tokens = UiKitTokens::default();
    let geometry = our_geometry();
    let ctx = Ctx {
        state: &state,
        config: &config,
        tokens: &tokens,
        motion: ui_core::time::MotionPreference::Full,
        geometry: &geometry,
        px: ui_kit::view::PxPerPt::new(2.4545).expect("масштаб"),
        now: MonotonicNanos::from_nanos(5),
    };
    let events = || {
        vec![
            ScaleEvent::ValueChanged { value: 1.0 / 3.0 },
            ScaleEvent::Haptic {
                pattern: HapticPattern::SelectionTick,
            },
        ]
    };
    // Программное движение: значение гасится и считается, хаптика (её даёт только палец) проходит как есть.
    let mut drive = FingerDrive::default();
    let mut out = Outputs::default();
    drive.translate(events(), &ctx, &mut out);
    assert!(out.actions.is_empty());
    assert_eq!(drive.suppressed_echo_events(), 1);
    // Палец взял шкалу: то же событие становится действием с метками времени кадра.
    drive.finger_took_over();
    let mut out = Outputs::default();
    drive.translate(events(), &ctx, &mut out);
    assert!(matches!(
        out.actions[..],
        [ScreenAction::ExposureDragged { requested, at }] if (requested.ev() - 1.0 / 3.0).abs() < 1e-6 && at == ctx.now
    ));
    assert_eq!(out.haptics, [HapticPattern::SelectionTick]);
    // Успокоение отпускает шкалу: следующие события — снова эхо.
    drive.translate(
        vec![ScaleEvent::Settled { value: 1.0 / 3.0 }],
        &ctx,
        &mut Outputs::default(),
    );
    assert!(!drive.is_finger_driven());
    // Значение, которое нельзя превратить в компенсацию, действия не даёт, но не глотается молча: счётчик.
    let mut broken = FingerDrive::default();
    broken.finger_took_over();
    let mut out = Outputs::default();
    broken.translate(vec![ScaleEvent::ValueChanged { value: f64::NAN }], &ctx, &mut out);
    assert!(out.actions.is_empty());
    assert_eq!(broken.invalid_bias_values(), 1);
    assert_eq!(broken.suppressed_echo_events(), 0, "это не эхо программы");
}

#[test]
fn blink_curve_matches_the_measured_shape() {
    let overlay = OverlayLayout::default();
    assert_eq!(darkness_after(&overlay, -0.1), 0.0);
    assert!(
        darkness_after(&overlay, 0.008) < overlay.blink_peak_darkness,
        "нарастает за кадр"
    );
    assert!(
        (darkness_after(&overlay, 0.040) - overlay.blink_peak_darkness).abs() < 1e-9,
        "удержание"
    );
    // Возврат: остаток убывает по экспоненте λ = 18 1/с, без перелёта.
    let after_hold = (overlay.blink_attack_ms + overlay.blink_hold_ms) / 1000.0;
    let half = darkness_after(&overlay, after_hold + 0.1);
    let expected = overlay.blink_peak_darkness * (-1.8_f64).exp();
    assert!((half - expected).abs() < 1e-9);
    assert!(darkness_after(&overlay, after_hold + 1.0) < overlay.blink_end_threshold);
}

#[test]
fn countdown_rounds_up_and_never_goes_negative() {
    let second = 1_000_000_000;
    assert_eq!(seconds_left(3 * second, 0), 3);
    assert_eq!(seconds_left(3 * second, 1), 3);
    assert_eq!(seconds_left(3 * second, 2 * second + 1), 1);
    assert_eq!(seconds_left(3 * second, 3 * second), 0);
    assert_eq!(seconds_left(3 * second, 4 * second), 0);
}

#[test]
fn ev_text_is_zero_or_signed_with_one_decimal() {
    assert_eq!(format_ev(0.0), "0");
    assert_eq!(format_ev(1.0 / 3.0), "+0.3");
    assert_eq!(format_ev(-0.6667), "-0.7");
}
