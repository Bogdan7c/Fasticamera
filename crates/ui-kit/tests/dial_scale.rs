#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Дуговая шкала (колесо зума, сессия 13): риски по значениям, хаптика рисок и упоров, магнит, вид по дуге,
//! ведение долгим тапом, пресет чистого размытия. Проверяются итоги — нарисованное, события, значения.

use shell_contract::haptics::HapticPattern;
use ui_core::geometry::{Point, Vector};
use ui_core::gesture::{GestureData, GestureState, PanTranslation};
use ui_core::time::MotionPreference;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, IconItem};
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::icons::IconId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::drive::{DragOrigin, GrowthDirection, ScrollTransition};
use ui_kit::pan::{PanUpdate, PanUpdateError};
use ui_kit::scale::{ScaleEvent, ScaleSpec, SnapPolicy, SnapScale, TickVisibility};
use ui_kit::tokens::{ArcScaleTokens, GlassRole, ScaleDensity, TextRoleKind, UiKitTokens};
use ui_kit::value::{Emphasis, LogLimits, Logarithmic, LogarithmicSpec, ValueError, ValueModel};
use ui_kit::view::{ArcLabel, LabelPainter, PxPerPt, arc_scale_items};

mod common;
use common::*;

const SCALE: f32 = 3.0;

/// Модель колеса: октава = единица, упоры 0,6 / 1 / 2, риски через 0,1× до 4,4× и через 1× дальше.
fn dial_model() -> Logarithmic {
    let mut ticks: Vec<f64> = (1..=44).map(|i| f64::from(i) / 10.0).collect();
    ticks.extend([5.0, 6.0, 7.0, 8.0, 9.0, 10.0]);
    Logarithmic::new(
        LogarithmicSpec {
            units_per_octave: 1.0,
            tick_spacing_units: 0.1,
            anchors: vec![0.6, 1.0, 2.0],
        },
        LogLimits { min: 0.6, max: 10.0 },
    )
    .unwrap()
    .with_tick_values(ticks)
    .unwrap()
}

fn dial_scale(value: f64, tokens: &UiKitTokens) -> SnapScale<Logarithmic> {
    let spec = ScaleSpec {
        direction: GrowthDirection::FingerLeft,
        snap: SnapPolicy::ContinuousWithMagnets,
        density: ScaleDensity::Dial,
        ticks: TickVisibility::Visible,
        drag_origin: DragOrigin::FromRecognition,
    };
    SnapScale::new(dial_model(), value, spec, &env(tokens, MotionPreference::Full), ms(0)).unwrap()
}

// ---- модель: риски по значениям ----

#[test]
fn tick_values_put_marks_at_those_values_and_anchors_win_over_coinciding_ticks() {
    let model = dial_model();
    let marks = model.marks();
    // Упоры 1 и 2 совпали с рисками 1,0 и 2,0: остались упорами, а не дубликатами.
    let references: Vec<f64> = marks
        .iter()
        .filter(|mark| mark.emphasis == Emphasis::Reference)
        .map(|mark| 2.0_f64.powf(mark.position))
        .collect();
    assert_eq!(references.len(), 3);
    for (found, expected) in references.iter().zip([0.6, 1.0, 2.0]) {
        assert!((found - expected).abs() < 1e-9, "{references:?}");
    }
    // Риска 1,3 на месте log2(1,3), а не на равномерной сетке по октаве.
    let target = 1.3_f64.log2();
    assert!(marks.iter().any(|mark| (mark.position - target).abs() < 1e-9));
    // Каждая риска есть ровно один раз; число рисок = 44 мелких + 6 крупных + упор 0,6 (риски 0,6 нет в списке 0,1 шага? есть).
    let positions: Vec<f64> = marks.iter().map(|mark| mark.position).collect();
    let mut sorted = positions.clone();
    sorted.sort_by(f64::total_cmp);
    assert_eq!(positions, sorted, "риски отсортированы");
    sorted.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    assert_eq!(sorted.len(), positions.len(), "дубликатов нет");
}

#[test]
fn tick_values_outside_the_limits_do_not_appear_and_come_back_with_wider_limits() {
    let mut model = dial_model();
    assert!(
        model
            .marks()
            .iter()
            .all(|mark| 2.0_f64.powf(mark.position) >= 0.6 - 1e-9)
    );
    model.set_limits(LogLimits { min: 1.0, max: 3.0 }).unwrap();
    let values: Vec<f64> = model.marks().iter().map(|mark| 2.0_f64.powf(mark.position)).collect();
    assert!(
        values.iter().all(|value| (1.0 - 1e-9..=3.0 + 1e-9).contains(value)),
        "{values:?}"
    );
    assert!(values.iter().any(|value| (value - 1.5).abs() < 1e-9));
}

#[test]
fn invalid_tick_values_are_rejected() {
    for bad in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let result = dial_model().with_tick_values(vec![1.0, bad]);
        assert!(result.is_err(), "{bad}");
    }
    let too_many = dial_model().with_tick_values(vec![1.0; 200_000]);
    assert!(matches!(too_many, Err(ValueError::TooManyMarks { .. })));
}

// ---- шкала: события, хаптика, магнит ----

#[test]
fn moving_from_one_x_to_two_x_crosses_each_tick_and_clicks_once_on_the_anchor() {
    let tokens = UiKitTokens::default();
    let mut scale = dial_scale(1.0, &tokens);
    // Палец влево на октаву (57 pt) за 1,2 с, шагами по 16 мс.
    scale.catch(ms(0));
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    let steps = 75;
    let mut patterns = Vec::new();
    let mut ticks = 0;
    let mut references = 0;
    for step in 1..=steps {
        let dx = -57.0 * f64::from(step) / f64::from(steps);
        scale.drag_moved(ms(i64::from(step) * 16), pan_same(dx)).unwrap();
        for event in scale.drain_events() {
            match event {
                ScaleEvent::TickCrossed { .. } => ticks += 1,
                ScaleEvent::ReferenceCrossed { .. } => references += 1,
                ScaleEvent::Haptic { pattern } => patterns.push(pattern),
                _ => {}
            }
        }
    }
    assert!((scale.value(ms(1200)) - 2.0).abs() < 0.03, "{}", scale.value(ms(1200)));
    // Риски 1,1…1,9 — девять тиков, упор 2,0 — один.
    assert_eq!(ticks, 9);
    assert_eq!(references, 1);
    assert_eq!(patterns.last(), Some(&HapticPattern::RigidImpact));
    assert_eq!(
        patterns.iter().filter(|p| **p == HapticPattern::SelectionTick).count(),
        9
    );
}

#[test]
fn programmatic_value_set_is_silent_and_moves_the_scale_there() {
    let tokens = UiKitTokens::default();
    let mut scale = dial_scale(1.0, &tokens);
    scale.set_value(3.0, ScrollTransition::Immediate, ms(10)).unwrap();
    assert!((scale.value(ms(10)) - 3.0).abs() < 1e-9);
    let events = scale.drain_events();
    assert!(
        events.iter().all(|event| !matches!(event, ScaleEvent::Haptic { .. })),
        "{events:?}"
    );
}

#[test]
fn release_close_to_an_anchor_is_pulled_to_it_but_in_between_values_stay() {
    let tokens = UiKitTokens::default();
    // Рядом с упором «2»: 1,95× — в пределах 6 pt (≈ 0,1 октавы), притягивается.
    let mut near_anchor = dial_scale(1.95, &tokens);
    near_anchor.catch(ms(0));
    near_anchor.release_without_drag(ms(5)).unwrap();
    assert!(
        (near_anchor.value(ms(2000)) - 2.0).abs() < 1e-3,
        "{}",
        near_anchor.value(ms(2000))
    );
    // 1,7× — между упорами (13 pt от «2»): остаётся где остановили.
    let mut between = dial_scale(1.7, &tokens);
    between.catch(ms(0));
    between.release_without_drag(ms(5)).unwrap();
    assert!(
        (between.value(ms(2000)) - 1.7).abs() < 1e-3,
        "{}",
        between.value(ms(2000))
    );
}

// ---- ведение долгим тапом ----

#[test]
fn a_long_press_drives_the_scale_like_a_pan() {
    let tokens = UiKitTokens::default();
    let mut scale = dial_scale(1.0, &tokens);
    let began = PanUpdate::from_long_press(
        GestureState::Began,
        GestureData::LongPress {
            position: Point::new(0.0, 0.0),
            translation: pan_same(0.0),
        },
    )
    .unwrap()
    .unwrap();
    scale.apply_pan(ms(0), began).unwrap();
    let changed = PanUpdate::from_long_press(
        GestureState::Changed,
        GestureData::LongPress {
            position: Point::new(-28.5, 0.0),
            translation: pan_same(-28.5),
        },
    )
    .unwrap()
    .unwrap();
    scale.apply_pan(ms(300), changed).unwrap();
    assert!(
        (scale.value(ms(300)) - std::f64::consts::SQRT_2).abs() < 0.02,
        "{}",
        scale.value(ms(300))
    );
    let ended = PanUpdate::from_long_press(
        GestureState::Ended,
        GestureData::LongPress {
            position: Point::new(-28.5, 0.0),
            translation: pan_same(-28.5),
        },
    )
    .unwrap();
    assert_eq!(ended, Some(PanUpdate::Ended { velocity: Vector::ZERO }));
}

#[test]
fn long_press_updates_reject_foreign_data_and_ignore_failed_and_possible() {
    let pan_data = GestureData::Pan {
        position: Point::new(0.0, 0.0),
        translation: PanTranslation {
            from_touch_down: Vector::ZERO,
            from_recognition: Vector::ZERO,
        },
        velocity: Vector::ZERO,
    };
    assert_eq!(
        PanUpdate::from_long_press(GestureState::Changed, pan_data),
        Err(PanUpdateError::NotLongPressData {
            state: GestureState::Changed
        })
    );
    assert_eq!(
        PanUpdate::from_long_press(GestureState::Failed, GestureData::None),
        Ok(None)
    );
    assert_eq!(
        PanUpdate::from_long_press(GestureState::Possible, GestureData::None),
        Ok(None)
    );
    assert_eq!(
        PanUpdate::from_long_press(GestureState::Cancelled, GestureData::None),
        Ok(Some(PanUpdate::Cancelled))
    );
}

// ---- вид по дуге ----

fn px_scale() -> PxPerPt {
    PxPerPt::new(SCALE).unwrap()
}

/// Узел-хост: прямоугольник, центр которого на (540, 1500) px; центр диска на 100 pt ниже центра узла.
fn host(opacity: f32) -> NodePlacementPx {
    NodePlacementPx {
        size: PxSize::new(1080.0, 600.0),
        transform: Affine2::translate(540.0, 1500.0),
        opacity,
    }
}

const DISC_OFFSET_PT: (f64, f64) = (0.0, 100.0);

fn disc_center_px() -> PxPoint {
    PxPoint::new(540.0, 1500.0 + 100.0 * SCALE)
}

/// Угол риски от маркера вверх (градусы, по часовой): от центра диска к центру фигуры.
fn angle_of(center: PxPoint) -> f64 {
    let disc = disc_center_px();
    f64::from(center.x - disc.x)
        .atan2(-f64::from(center.y - disc.y))
        .to_degrees()
}

fn radius_of(center: PxPoint) -> f64 {
    let disc = disc_center_px();
    f64::from(center.x - disc.x).hypot(f64::from(center.y - disc.y)) / f64::from(SCALE)
}

fn draw(
    scale: &SnapScale<Logarithmic>,
    tokens: &UiKitTokens,
    labels: &[ArcLabel],
    opacity: f32,
) -> ui_kit::view::ArcScaleDrawing {
    arc_scale_items(
        &host(opacity),
        DISC_OFFSET_PT,
        scale,
        ms(0),
        px_scale(),
        &tokens.arc_scale,
        &tokens.colors,
        labels,
        |value| (value - value.round()).abs() < 1e-6 && value >= 3.0,
    )
    .unwrap()
}

/// Иконка-маркер: последний элемент переднего слоя (должна быть иконкой, не фигурой).
fn marker_icon(drawing: &ui_kit::view::ArcScaleDrawing) -> IconItem {
    match drawing.foreground.last() {
        Some(DrawItem::Icon(icon)) => *icon,
        other => panic!("маркер должен быть иконкой: {other:?}"),
    }
}

fn shape_center(item: &DrawItem) -> (PxPoint, ShapeGeometry, DisplayColor, f32) {
    let DrawItem::Shape(shape) = item else {
        panic!("ожидалась фигура: {item:?}")
    };
    (
        shape.transform.apply_point(PxPoint::ORIGIN),
        shape.geometry,
        shape.fill,
        shape.opacity.get(),
    )
}

#[test]
fn the_disc_is_a_dark_circle_of_the_measured_radius_under_the_ticks() {
    let tokens = UiKitTokens::default();
    let scale = dial_scale(1.0, &tokens);
    let drawing = draw(&scale, &tokens, &[], 1.0);
    assert_eq!(drawing.backing.len(), 1);
    let (center, geometry, fill, opacity) = shape_center(&drawing.backing[0]);
    let ShapeGeometry::Circle { diameter } = geometry else {
        panic!("{geometry:?}")
    };
    assert!(
        (f64::from(diameter) / f64::from(SCALE) - 486.0).abs() < 0.01,
        "радиус 243 pt"
    );
    assert_eq!(fill, DisplayColor::BLACK);
    assert!((opacity - 0.45).abs() < 1e-6);
    assert!((center.x - 540.0).abs() < 0.01 && (center.y - disc_center_px().y).abs() < 0.01);
}

#[test]
fn ticks_sit_on_the_ring_at_14_2_degrees_per_octave_around_the_fixed_marker() {
    let tokens = UiKitTokens::default();
    let scale = dial_scale(1.0, &tokens);
    let drawing = draw(&scale, &tokens, &[], 1.0);
    let marker = marker_icon(&drawing);
    // Маркер — жёлтый треугольник #FFCE00 наверху (угол 0), 3,3 × 8,7 pt, на радиусе середины кольца.
    assert_eq!(marker.icon, IconId::ZoomMarkerTriangle);
    assert_eq!(marker.color, tokens.colors.zoom_selected);
    let marker_center = marker.transform.apply_point(PxPoint::ORIGIN);
    assert!(angle_of(marker_center).abs() < 1e-3);
    assert!((radius_of(marker_center) - 230.5).abs() < 0.01);
    assert!((f64::from(marker.size_px.width) / f64::from(SCALE) - 3.3).abs() < 0.01);
    assert!((f64::from(marker.size_px.height) / f64::from(SCALE) - 8.7).abs() < 0.01);

    // Остальные фигуры — риски на радиусе середины кольца (222,5…238,5).
    let ticks: Vec<_> = drawing.foreground[..drawing.foreground.len() - 1]
        .iter()
        .map(shape_center)
        .collect();
    assert!(!ticks.is_empty());
    for (center, ..) in &ticks {
        assert!((radius_of(*center) - 230.5).abs() < 0.01, "{}", radius_of(*center));
    }
    // Риска 2,0 — на октаву правее: 14,2° по часовой. Риска 1,0 — под маркером.
    let angles: Vec<f64> = ticks.iter().map(|(center, ..)| angle_of(*center)).collect();
    assert!(angles.iter().any(|angle| (angle - 14.2).abs() < 0.01), "{angles:?}");
    assert!(angles.iter().any(|angle| angle.abs() < 0.01));
    // Риска 1,5 — на log2(1,5) × 14,2°.
    let expected = 1.5_f64.log2() * 14.2;
    assert!(angles.iter().any(|angle| (angle - expected).abs() < 0.01), "{angles:?}");
}

#[test]
fn bright_ticks_use_full_white_and_the_rest_the_dim_alpha() {
    let tokens = UiKitTokens::default();
    let scale = dial_scale(3.0, &tokens);
    let drawing = draw(&scale, &tokens, &[], 1.0);
    let ticks: Vec<_> = drawing.foreground[..drawing.foreground.len() - 1]
        .iter()
        .map(shape_center)
        .collect();
    let at = |target: f64| {
        ticks
            .iter()
            .find(|(center, ..)| (angle_of(*center) - target).abs() < 0.01)
            .unwrap_or_else(|| panic!("нет риски под углом {target}: ждали одну из {}", ticks.len()))
    };
    // Текущее значение 3,0 — целое от порога: яркая (белая). 3,5 — тусклая (α 0,2).
    let current = at(0.0);
    assert!((current.2.a - 1.0).abs() < 1e-6);
    let half_step = at((3.5_f64 / 3.0).log2() * 14.2);
    assert!((half_step.2.a - 0.2).abs() < 1e-6, "{:?}", half_step.2);
    // Упор «2» — крупная риска (опорная точка): всегда яркая.
    let anchor = at((2.0_f64 / 3.0).log2() * 14.2);
    assert!((anchor.2.a - 1.0).abs() < 1e-6);
}

#[test]
fn ticks_fade_from_43_degrees_and_vanish_at_56() {
    let tokens = UiKitTokens::default();
    let scale = dial_scale(1.0, &tokens);
    let drawing = draw(&scale, &tokens, &[], 1.0);
    let ticks: Vec<_> = drawing.foreground[..drawing.foreground.len() - 1]
        .iter()
        .map(shape_center)
        .collect();
    assert!(
        ticks.iter().all(|(center, ..)| angle_of(*center).abs() < 56.0),
        "дальше 56° рисок нет"
    );
    // До 43° прозрачность полная, между 43° и 56° убывает.
    for (center, _, _, opacity) in &ticks {
        let angle = angle_of(*center).abs();
        if angle <= 43.0 {
            assert!((opacity - 1.0).abs() < 1e-6, "{angle}° → {opacity}");
        } else {
            assert!(*opacity < 1.0 && *opacity > 0.0, "{angle}° → {opacity}");
        }
    }
    // Риска 9,0 стоит на 45,0° (две градуса внутрь зоны затухания): smoothstep(2/13) → множитель ≈ 0,936.
    let nine = ticks
        .iter()
        .find(|(center, ..)| (angle_of(*center) - 9.0_f64.log2() * 14.2).abs() < 0.01)
        .expect("риска 9,0 нарисована");
    let progress = 2.0 / 13.0_f64;
    let expected = 1.0 - progress * progress * (3.0 - 2.0 * progress);
    assert!(
        (f64::from(nine.3) - expected).abs() < 1e-3,
        "{} против {expected}",
        nine.3
    );
}

#[test]
fn labels_stand_on_their_radius_rotated_to_the_tick_angle() {
    let tokens = UiKitTokens::default();
    let scale = dial_scale(1.0, &tokens);
    let painter = LabelPainter::new(&tokens.typography, px_scale());
    let run = painter.layout("2", TextRoleKind::ZoomLabel).unwrap();
    let labels = [ArcLabel {
        offset_points: scale.model().position_of(2.0).unwrap() * scale.points_per_unit(),
        run,
        color: DisplayColor::WHITE,
        radius_pt: 206.8,
    }];
    let drawing = draw(&scale, &tokens, &labels, 1.0);
    let texts: Vec<_> = drawing
        .foreground
        .iter()
        .filter_map(|item| match item {
            DrawItem::Text(text) => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(texts.len(), 1);
    // Начало координат содержимого подписи — центр хоста: его образ лежит на радиусе 206,8 под углом 14,2°.
    let origin = texts[0].transform.apply_point(PxPoint::new(
        texts[0].run.advance_px * 0.5,
        -texts[0].run.cap_height_px * 0.5,
    ));
    assert!((radius_of(origin) - 206.8).abs() < 0.05, "{}", radius_of(origin));
    assert!((angle_of(origin) - 14.2).abs() < 0.01, "{}", angle_of(origin));
}

#[test]
fn node_opacity_scales_every_drawn_item_and_hidden_ticks_leave_only_disc_and_marker() {
    let tokens = UiKitTokens::default();
    let scale = dial_scale(1.0, &tokens);
    let drawing = draw(&scale, &tokens, &[], 0.5);
    let (_, _, _, disc_opacity) = shape_center(&drawing.backing[0]);
    assert!((disc_opacity - 0.45 * 0.5).abs() < 1e-6);
    assert!((marker_icon(&drawing).opacity.get() - 0.5).abs() < 1e-6);

    let spec = ScaleSpec {
        ticks: TickVisibility::Hidden,
        density: ScaleDensity::Dial,
        snap: SnapPolicy::ContinuousWithMagnets,
        direction: GrowthDirection::FingerLeft,
        drag_origin: DragOrigin::FromRecognition,
    };
    let hidden = SnapScale::new(dial_model(), 1.0, spec, &env(&tokens, MotionPreference::Full), ms(0)).unwrap();
    let drawing = draw(&hidden, &tokens, &[], 1.0);
    assert_eq!(drawing.foreground.len(), 1, "только маркер");
}

#[test]
fn the_ring_turns_when_the_value_changes() {
    let tokens = UiKitTokens::default();
    let at_one = draw(&dial_scale(1.0, &tokens), &tokens, &[], 1.0);
    let at_two = draw(&dial_scale(2.0, &tokens), &tokens, &[], 1.0);
    let angle_of_value = |drawing: &ui_kit::view::ArcScaleDrawing, value: f64, current: f64| {
        let target = (value / current).log2() * 14.2;
        drawing.foreground[..drawing.foreground.len() - 1]
            .iter()
            .map(shape_center)
            .any(|(center, ..)| (angle_of(center) - target).abs() < 0.01)
    };
    assert!(angle_of_value(&at_one, 1.5, 1.0));
    assert!(
        angle_of_value(&at_two, 1.5, 2.0),
        "то же значение сместилось вместе с поворотом кольца"
    );
}

// ---- токены ----

#[test]
fn frost_preset_blurs_without_lensing_tinting_or_shadow() {
    let tokens = UiKitTokens::default();
    tokens.validate().unwrap();
    let style = tokens.glass.style_for(GlassRole::Frost);
    let resolved = style.resolve(PxSize::new(1080.0, 1500.0), SCALE);
    assert!(resolved.blur_radius_px > 0.0);
    assert_eq!(resolved.refraction_strength_px, 0.0);
    assert_eq!(resolved.dispersion, 0.0);
    assert_eq!(resolved.specular.alpha, 0.0);
    assert_eq!(resolved.dark_edge.strength, 0.0);
    assert_eq!(resolved.dimming.max_alpha, 0.0);
    assert_eq!(resolved.tint_level, 0.0);
    assert_eq!(resolved.shadow.alpha, 0.0);
    assert_eq!(resolved.saturation, 1.0);
}

#[test]
fn arc_scale_and_throttle_tokens_reject_nonsense() {
    let tokens = UiKitTokens {
        arc_scale: ArcScaleTokens {
            fade_start_degrees: 60.0,
            ..ArcScaleTokens::default()
        },
        ..UiKitTokens::default()
    };
    assert!(tokens.validate().is_err(), "затухание должно расти с углом");
    let mut tokens = UiKitTokens::default();
    tokens.arc_scale.tick_outer_radius_pt = 300.0;
    assert!(tokens.validate().is_err(), "кольцо должно лежать внутри диска");
    let mut tokens = UiKitTokens::default();
    tokens.haptic_throttle.min_interval_ms = 0.0;
    assert!(tokens.validate().is_err());
    let mut tokens = UiKitTokens::default();
    tokens.density.dial_points_per_unit = f64::NAN;
    assert!(tokens.validate().is_err());
}
