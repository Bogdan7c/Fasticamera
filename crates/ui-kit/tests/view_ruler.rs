#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Линейка нижней карточки (`view::ruler_items`): шаг 20 pt на единицу, равные риски 2 × 13 pt,
//! отметка 2 × 26 pt жёлтая, крупные риски ярче, затухание у краёв по расстоянию в шагах.

use std::num::NonZeroU32;

use ui_draw::color::DisplayColor;
use ui_draw::draw_list::DrawItem;
use ui_draw::geometry::{PxPoint, PxSize};
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::drive::{DragOrigin, GrowthDirection, ScrollTransition};
use ui_kit::tokens::{ScaleDensity, UiKitTokens};
use ui_kit::view::{MajorTickPeriod, PxPerPt, ruler_items};

mod common;
use common::*;

const SCALE: f32 = 3.0;
const CENTER: (f32, f32) = (660.0, 900.0);

#[derive(Debug, Clone, Copy)]
struct Bar {
    center: PxPoint,
    size: PxSize,
    fill: DisplayColor,
}

fn bars(items: &[DrawItem]) -> Vec<Bar> {
    items
        .iter()
        .map(|item| {
            let DrawItem::Shape(shape) = item else {
                panic!("{item:?}")
            };
            let ShapeGeometry::RoundedRect { size, .. } = shape.geometry else {
                panic!("{:?}", shape.geometry)
            };
            Bar {
                center: shape.transform.apply_point(PxPoint::ORIGIN),
                size,
                fill: shape.fill,
            }
        })
        .collect()
}

/// Линейка с плотностью Ruler; значение 0 — в центре.
fn ruler_bars(value: f64) -> (Vec<Bar>, UiKitTokens) {
    let tokens = UiKitTokens::default();
    let mut spec = exposure_spec(GrowthDirection::FingerLeft, DragOrigin::FromRecognition);
    spec.density = ScaleDensity::Ruler;
    let mut scale = exposure_scale(0.0, spec, &tokens);
    scale.set_value(value, ScrollTransition::Immediate, ms(0)).unwrap();
    let placement = NodePlacementPx {
        size: PxSize::new(900.0, 120.0),
        transform: Affine2::translate(CENTER.0, CENTER.1),
        opacity: 1.0,
    };
    let period = MajorTickPeriod::every_steps(NonZeroU32::new(3).unwrap());
    let items = ruler_items(
        &placement,
        &scale,
        ms(0),
        PxPerPt::new(SCALE).unwrap(),
        &tokens.ruler,
        period,
        &tokens.colors,
    )
    .unwrap();
    (bars(&items), tokens)
}

fn close(actual: f32, expected: f32, what: &str) {
    assert!((actual - expected).abs() < 0.01, "{what}: {actual} != {expected}");
}

#[test]
fn ticks_are_20_pt_apart_equal_in_size_and_the_mark_is_yellow_and_longer() {
    let (drawn, tokens) = ruler_bars(0.0);
    let (mark, ticks) = drawn.split_last().unwrap();
    // Отметка: жёлтая, 2 × 26 pt, в центре узла.
    assert_eq!(mark.fill, tokens.colors.focus_yellow);
    assert_eq!(mark.size, PxSize::new(2.0 * SCALE, 26.0 * SCALE));
    close(mark.center.x, CENTER.0, "отметка в центре");
    // Риски до ±6 шагов (13 штук), каждая 2 × 13 pt, ровно через 20 pt (60 px) друг от друга.
    assert_eq!(ticks.len(), 13);
    for tick in ticks {
        assert_eq!(tick.size, PxSize::new(2.0 * SCALE, 13.0 * SCALE));
        close(tick.center.y, CENTER.1, "y риски");
    }
    for pair in ticks.windows(2) {
        close((pair[1].center.x - pair[0].center.x).abs(), 20.0 * SCALE, "шаг линейки");
    }
}

#[test]
fn every_third_tick_is_brighter_and_the_period_comes_from_the_caller() {
    let (drawn, tokens) = ruler_bars(0.0);
    let ticks = &drawn[..drawn.len() - 1];
    // Номера шагов -6..=6; в полную силу виден только диапазон до 4 шагов.
    for (index, tick) in (-6_i32..=6).zip(ticks) {
        let fade = tokens.ruler.fade_factor(f64::from(index));
        let base = if index.rem_euclid(3) == 0 {
            tokens.ruler.major_tick_alpha
        } else {
            tokens.ruler.minor_tick_alpha
        };
        close(tick.fill.a, base * fade, &format!("альфа шага {index}"));
    }
    // Явные значения из замера для центральных рисок.
    close(ticks[6].fill.a, 0.8, "ноль — крупная");
    close(ticks[7].fill.a, 0.5, "+1 — мелкая");
    close(ticks[9].fill.a, 0.8, "+3 — целый EV");
    close(ticks[10].fill.a, 0.5 * 1.0, "+4 — ещё в полную силу");
}

#[test]
fn ticks_fade_towards_the_edges_and_vanish_beyond_six_steps() {
    let (drawn, tokens) = ruler_bars(0.0);
    let ticks = &drawn[..drawn.len() - 1];
    // Полная до 4 шагов, 60 % на 5, 10 % на 6 (замер R00 t=159.0).
    close(tokens.ruler.fade_factor(4.0), 1.0, "4 шага");
    close(tokens.ruler.fade_factor(5.0), 0.6, "5 шагов");
    close(tokens.ruler.fade_factor(6.0), 0.1, "6 шагов");
    close(tokens.ruler.fade_factor(6.01), 0.0, "дальше 6");
    close(tokens.ruler.fade_factor(-5.0), 0.6, "слева так же");
    // Во вложенной шкале риски на ±5 и ±6 шагов приглушены, на ±6 они последние.
    close(ticks[0].fill.a, 0.8 * 0.1, "шаг -6 (крупная, 10 %)");
    close(ticks[1].fill.a, 0.5 * 0.6, "шаг -5 (мелкая, 60 %)");
    // Смещаем значение: риски дальше 6 шагов от отметки вообще не создаются.
    let (shifted, _) = ruler_bars(1.0);
    let steps_visible = shifted.len() - 1;
    assert!(steps_visible < 13, "риски дальше 6 шагов не рисуются: {steps_visible}");
    for tick in &shifted[..steps_visible] {
        assert!(tick.fill.a > 0.0);
        assert!((tick.center.x - CENTER.0).abs() <= 6.0 * 20.0 * SCALE + 0.01);
    }
}

#[test]
fn major_period_is_a_caller_parameter() {
    let tokens = UiKitTokens::default();
    let mut spec = exposure_spec(GrowthDirection::FingerLeft, DragOrigin::FromRecognition);
    spec.density = ScaleDensity::Ruler;
    let scale = exposure_scale(0.0, spec, &tokens);
    let placement = NodePlacementPx {
        size: PxSize::new(900.0, 120.0),
        transform: Affine2::translate(CENTER.0, CENTER.1),
        opacity: 1.0,
    };
    let alphas = |period: u32| {
        let items = ruler_items(
            &placement,
            &scale,
            ms(0),
            PxPerPt::new(SCALE).unwrap(),
            &tokens.ruler,
            MajorTickPeriod::every_steps(NonZeroU32::new(period).unwrap()),
            &tokens.colors,
        )
        .unwrap();
        bars(&items).iter().map(|bar| bar.fill.a).collect::<Vec<_>>()
    };
    // Период 2: шаг +2 (индекс 8) крупный, при периоде 3 — мелкий.
    assert!((alphas(2)[8] - 0.8).abs() < 1e-4);
    assert!((alphas(3)[8] - 0.5).abs() < 1e-4);
}

#[test]
fn ruler_density_is_20_pt_per_unit_and_leaves_the_dense_one_alone() {
    let tokens = UiKitTokens::default();
    assert_eq!(tokens.density.points_per_unit(ScaleDensity::Ruler), 20.0);
    assert_eq!(tokens.density.points_per_unit(ScaleDensity::Dense), 12.0);
}
