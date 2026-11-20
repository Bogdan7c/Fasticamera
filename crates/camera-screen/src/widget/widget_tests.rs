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
    ScreenGeometry::new(
        &layout,
        &WidgetLayoutConfig::default(),
        &crate::zoom_config::ZoomConfig::default(),
        &UiKitTokens::default(),
    )
}

#[test]
fn widget_config_default_is_valid_and_rejects_nonsense() {
    assert_eq!(WidgetLayoutConfig::default().validate(), Ok(()));
    let mut broken = WidgetLayoutConfig::default();
    broken.sheet.column_pitch_pt = -1.0;
    assert!(matches!(
        broken.validate(),
        Err(WidgetConfigError::NotPositive {
            field: "sheet.column_pitch_pt",
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
    // Наш экран выше эталона на 26,7 pt: всё считается от низа. Затвор 801,9 → 828,6 pt (≈ 2034 px).
    assert!((geometry.shutter().center().y - 828.6).abs() < 0.1);
    assert!((geometry.shutter().center().y * scale - 2034.0).abs() < 1.0);
    // [measured: C t=18.0] капсула 221,4 × 48,9 с центром y 898,0 → 924,7; пилюля 89 × 43 по её центру.
    let capsule = geometry.capsule();
    assert!((capsule.center().y - 924.7).abs() < 0.1);
    assert!((capsule.size.width - 221.4).abs() < 1e-9 && (capsule.size.height - 48.9).abs() < 1e-9);
    let pill = geometry.mode_pill();
    assert!((pill.size.width - 89.0).abs() < 1e-9 && (pill.size.height - 43.0).abs() < 1e-9);
    assert!((pill.center().x - capsule.center().x).abs() < 1e-9 && (pill.center().y - capsule.center().y).abs() < 1e-9);
    // [measured: C t=18.0] кнопка меню Ø 44, центр (383,6; 801,9 → 828,6); миниатюра Ø 48 в (57,7; 924,7).
    let menu = geometry.menu_button();
    assert!((menu.size.width - 44.0).abs() < 1e-9);
    assert!((menu.center().x - 383.6).abs() < 1e-9 && (menu.center().y - 828.6).abs() < 0.1);
    let thumbnail = geometry.thumbnail();
    assert!((thumbnail.size.width - 48.0).abs() < 1e-9);
    assert!((thumbnail.center().x - 57.7).abs() < 1e-9 && (thumbnail.center().y - 924.7).abs() < 0.1);
    // [measured: C t=18.0] верхний ряд: центр y 102,7 → 129,4, шаг 43,6 (три значка: 176,4 / 220,0 / 263,6).
    assert!((geometry.strip_icon(0, 3).center().y - 129.4).abs() < 0.1);
    let xs: Vec<f64> = (0..3).map(|i| geometry.strip_icon(i, 3).center().x).collect();
    for (actual, expected) in xs.iter().zip([176.4, 220.0, 263.6]) {
        assert!((actual - expected).abs() < 1e-9, "{xs:?}");
    }
    // [measured: C t=30.0] шторка 430 × 464 (x 5–435), низ на 8,6 выше низа экрана (982,7 − 8,6 = 974,1).
    let sheet = geometry.sheet();
    assert!((sheet.min_x() - 5.0).abs() < 1e-9 && (sheet.max_x() - 435.0).abs() < 1e-9);
    assert!((sheet.max_y() - 974.1).abs() < 0.1 && (sheet.size.height - 464.0).abs() < 1e-9);
    // Пункты: круги Ø 68, центры x 91 / 220 / 349, центр первого ряда на 83 ниже верха, шаг рядов 124.
    let item = |slot| geometry.menu_item(slot);
    for (slot, expected_x) in [(0, 91.0), (1, 220.0), (2, 349.0), (3, 91.0)] {
        assert!((item(slot).center().x - expected_x).abs() < 1e-9, "слот {slot}");
        assert!((item(slot).size.width - 68.0).abs() < 1e-9);
    }
    assert!((item(0).center().y - (sheet.min_y() + 83.0)).abs() < 1e-9);
    assert!((item(3).center().y - item(0).center().y - 124.0).abs() < 1e-9);
    assert!(sheet.contains(item(0).center()) && sheet.contains(item(8).center()));
    // [measured: C t=30.0] «ручка» 37,6 × 3,7 по центру шторки, верх на 8,9 ниже верха шторки.
    let handle = geometry.sheet_handle();
    assert!((handle.size.width - 37.6).abs() < 1e-9 && (handle.size.height - 3.7).abs() < 1e-9);
    assert!((handle.center().x - 220.0).abs() < 1e-9 && (handle.min_y() - (sheet.min_y() + 8.9)).abs() < 1e-9);
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
    assert_eq!(format_ev(0.0), "0.0");
    assert_eq!(format_ev(5.0 / 3.0), "1.7");
    assert_eq!(format_ev(-2.0), "\u{2212}2.0");
    assert_eq!(format_ev(-0.04), "0.0", "минус перед нулём не рисуется");
}

#[test]
fn ev_text_never_prints_a_silent_zero_for_non_finite_values() {
    // NaN/inf в состоянии невозможны (шаги целые, шаг EV проверен конфигом), поэтому в отладке это ошибка
    // вызывающего, а не тихий «0.0».
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let outcome = std::panic::catch_unwind(|| format_ev(bad));
        if cfg!(debug_assertions) {
            assert!(outcome.is_err(), "format_ev({bad}) должен упасть в отладке");
        }
    }
}

#[test]
fn card_layout_rejects_non_finite_numbers() {
    use crate::widgets_config::CardPoint;
    let nan_point = |point: CardPoint| CardPoint {
        offset_from_center_x_pt: f64::NAN,
        ..point
    };
    let inf_top = |point: CardPoint| CardPoint {
        center_from_top_pt: f64::INFINITY,
        ..point
    };
    type Mutate = fn(&mut WidgetLayoutConfig);
    let cases: [(&str, Mutate); 12] = [
        ("card.bottom_inset_pt NaN", |c| c.card.bottom_inset_pt = f64::NAN),
        ("card.bottom_inset_pt отрицательный", |c| {
            c.card.bottom_inset_pt = -1.0
        }),
        ("card.title_center_from_top_pt inf", |c| {
            c.card.title_center_from_top_pt = f64::INFINITY
        }),
        ("card.value_center_from_top_pt NaN", |c| {
            c.card.value_center_from_top_pt = f64::NAN
        }),
        ("card.ruler_ticks_bottom_from_top_pt inf", |c| {
            c.card.ruler_ticks_bottom_from_top_pt = f64::NEG_INFINITY
        }),
        ("card.raised_shutter_offset_pt NaN", |c| {
            c.card.raised_shutter_offset_pt = f64::NAN
        }),
        ("strip.ev_unit_offset_pt NaN", |c| c.strip.ev_unit_offset_pt = f64::NAN),
        ("strip.ev_unit_offset_pt inf", |c| {
            c.strip.ev_unit_offset_pt = f64::INFINITY
        }),
        ("sheet.open_content_slide_pt NaN", |c| {
            c.sheet.open_content_slide_pt = f64::NAN
        }),
        ("sheet.open_content_slide_pt отрицательный", |c| {
            c.sheet.open_content_slide_pt = -0.5
        }),
        ("sheet.caption_alpha вне 0..1", |c| c.sheet.caption_alpha = 1.5),
        ("card.left_button.center_from_top_pt NaN", |c| {
            c.card.left_button.center_from_top_pt = f64::NAN
        }),
    ];
    for (name, mutate) in cases {
        let mut config = WidgetLayoutConfig::default();
        mutate(&mut config);
        assert!(config.validate().is_err(), "{name}: конфиг принят");
    }
    // Точки карточки: каждая из трёх, по обеим осям.
    for which in 0..3 {
        for (axis, make) in [("x", nan_point as fn(CardPoint) -> CardPoint), ("y", inf_top)] {
            let mut config = WidgetLayoutConfig::default();
            let point = match which {
                0 => &mut config.card.left_button,
                1 => &mut config.card.right_button,
                _ => &mut config.card.options_center,
            };
            *point = make(*point);
            assert!(config.validate().is_err(), "точка {which} по {axis}: конфиг принят");
        }
    }
    // Допустимые крайние значения не отвергаются: нулевой въезд, любой знак у сдвигов подписей и кнопок.
    let mut ok = WidgetLayoutConfig::default();
    ok.sheet.open_content_slide_pt = 0.0;
    ok.strip.ev_unit_offset_pt = -3.0;
    ok.card.left_button.offset_from_center_x_pt = -171.0;
    ok.card.raised_shutter_offset_pt = -10.0;
    assert_eq!(ok.validate(), Ok(()));
}

#[test]
fn card_content_is_placed_from_the_card_centre_on_any_screen_width() {
    let width_pt = 400.0;
    let mut layout_config = ScreenConfig::default().layout;
    layout_config.point_scale.reference_width_pt = width_pt;
    let layout = ScreenLayout::new(
        &layout_config,
        SurfaceSize {
            width_px: 1080,
            height_px: 2412,
        },
    )
    .expect("раскладка");
    let geometry = ScreenGeometry::new(
        &layout,
        &WidgetLayoutConfig::default(),
        &crate::zoom_config::ZoomConfig::default(),
        &UiKitTokens::default(),
    );
    let card = geometry.card();
    assert!(
        (card.center().x - width_pt / 2.0).abs() < 1e-9,
        "карточка по центру экрана"
    );
    // Кнопки симметричны относительно середины карточки, на ±171 pt (49 и 391 на эталоне 440).
    let (left, right) = (
        geometry.card_left_button().center(),
        geometry.card_right_button().center(),
    );
    assert!(
        (left.x - (card.center().x - 171.0)).abs() < 1e-9,
        "левая кнопка {left:?}"
    );
    assert!(
        (right.x - (card.center().x + 171.0)).abs() < 1e-9,
        "правая кнопка {right:?}"
    );
    assert!(
        ((left.x + right.x) / 2.0 - card.center().x).abs() < 1e-9,
        "кнопки симметричны"
    );
    // Ряд вариантов и заголовок стоят на середине карточки.
    let first = geometry.card_option_slot(0, 3).center();
    let last = geometry.card_option_slot(2, 3).center();
    assert!(
        ((first.x + last.x) / 2.0 - card.center().x).abs() < 1e-9,
        "ряд под заголовком"
    );
    assert!((geometry.card_title_center().x - card.center().x).abs() < 1e-9);
    assert!((geometry.card_value_center().x - card.center().x).abs() < 1e-9);
}
