//! Переключатели страниц гистограммы и CIE сквозь все слои (решения 331, 332, 335): ПАЛЕЦ по полоске прибора →
//! экран → запрос рендеру → НАСТОЯЩИЙ бэкенд (RADV, headless) держит этот запрос и кладёт подписи охвата на доску →
//! экран рисует «P3» внутри картинки. Доказательство — запрос в диагностике рендера и подпись в рисунке экрана, а не
//! вызовы. Стенд — `calibration_rig` (общий с `calibration_end_to_end.rs`), портрет.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/viewfinder_rig/mod.rs"]
mod viewfinder_rig;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;

#[allow(
    dead_code,
    reason = "стенд общий: каждый тестовый файл берёт своё подмножество шагов"
)]
mod calibration_rig;

use calibration_rig::{AcceptingPersistence, CalibrationRig, library_over, live_scope_request};
use camera_screen::ScreenAction;
use camera_screen::calibration::{CalibrationConfig, ScopeStripFrames};
use camera_screen::modes::CaptureMode;
use fasticamera::camera_screen_input::InputDelivery;
use scope_contract::{CieDiagram, HistogramMode, ScopeIntent, ScopeKind, ScopeLabelsState};
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Rect};
use viewfinder_rig::reddish;

/// Пикселей поверхности стенда на точку экрана (1080 px на 440 pt).
const PX_PER_PT: f64 = 1080.0 / 440.0;

fn calibrating_rig() -> Option<CalibrationRig> {
    let mut rig = CalibrationRig::start(library_over(Box::new(AcceptingPersistence)), reddish())?;
    rig.send(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    rig.frames(4);
    Some(rig)
}

/// Рамки частей полоски прибора на экране (портрет: глаз и поверхность совпадают).
fn strip_frames(rig: &CalibrationRig) -> ScopeStripFrames {
    let layout = rig.screen().calibration().layout().expect("раскладка раздела");
    ScopeStripFrames::of(layout.scope_strip(), &CalibrationConfig::default().widgets)
}

/// Центр сегмента `index` из `count` в ряду `row` (как раскладывает ряд сегментов виджета: равные доли и зазор).
fn segment_centre(row: Rect, count: usize, index: usize) -> Point {
    let gap = CalibrationConfig::default().widgets.segment_gap_pt;
    let width = (row.size.width - gap * (count as f64 - 1.0)) / count as f64;
    Point::new(row.min_x() + index as f64 * (width + gap) + width / 2.0, row.center().y)
}

/// Касание пальцем: нажатие, отпускание, несколько кадров на реакцию.
fn tap(rig: &mut CalibrationRig, point: Point) {
    for phase in [TouchPhase::Began, TouchPhase::Ended] {
        let delivery = rig.touch(phase, (point.x * PX_PER_PT) as f32, (point.y * PX_PER_PT) as f32);
        assert_eq!(delivery, InputDelivery::Queued, "касание {phase:?} не принято каналом");
        rig.frame();
    }
    rig.frames(4);
}

#[test]
fn a_finger_on_the_histogram_segments_changes_the_mode_the_render_holds() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    let opening = live_scope_request(&mut rig);
    assert_eq!(opening.kind, ScopeKind::Histogram, "раздел открылся на гистограмме");
    assert_eq!(opening.histogram_mode, HistogramMode::Rgb);

    let frames = strip_frames(&rig);
    for (index, expected) in [
        (1, HistogramMode::Parade),
        (2, HistogramMode::Luma),
        (0, HistogramMode::Rgb),
    ] {
        tap(&mut rig, segment_centre(frames.histogram_toggle, 3, index));
        let ScopeIntent::Show(sent) = *rig.scope_intents.last().expect("запрос прибора") else {
            panic!("ожидалось «показать»: {:?}", rig.scope_intents);
        };
        assert_eq!(
            sent.histogram_mode, expected,
            "сегмент {index}: экран просит этот режим"
        );
        let held = live_scope_request(&mut rig);
        assert_eq!(held, sent, "сегмент {index}: рендер держит ровно то, что просил экран");
        assert_eq!(held.kind, ScopeKind::Histogram);
    }
    rig.finish();
}

#[test]
fn the_cie_page_draws_p3_inside_the_picture_and_the_diagram_toggle_republishes_the_labels() {
    let Some(mut rig) = calibrating_rig() else {
        return;
    };
    // Палец по точке последней страницы (CIE).
    let frames = strip_frames(&rig);
    let cie_dot = frames.page_dots[ScopeKind::ALL.len() - 1];
    tap(&mut rig, Point::new(cie_dot.center().x, cie_dot.center().y));
    rig.frames(4);
    let xy = live_scope_request(&mut rig);
    assert_eq!(xy.kind, ScopeKind::Cie);
    assert_eq!(xy.cie_diagram, CieDiagram::Xy1931);
    match rig.bench.scope_labels.latest() {
        ScopeLabelsState::Published(labels) => assert_eq!(labels.request, xy, "подписи посчитаны для запроса CIE"),
        other => panic!("рендер не опубликовал подписи охвата: {other:?}"),
    }
    let layout = *rig.screen().calibration().layout().expect("раскладка");
    let picture = layout.scope_image_rect_for(ScopeKind::Cie);
    let origins = rig.drawn_caption_origins("P3");
    assert_eq!(origins.len(), 1, "подпись «P3» нарисована один раз: {origins:?}");
    let (x_px, y_px) = origins[0];
    let (x_pt, y_pt) = (f64::from(x_px) / PX_PER_PT, f64::from(y_px) / PX_PER_PT);
    assert!(
        x_pt >= picture.min_x() && x_pt <= picture.max_x() && y_pt >= picture.min_y() && y_pt <= picture.max_y(),
        "«P3» внутри картинки CIE: ({x_pt}, {y_pt}) против {picture:?}"
    );

    // u'v': запрос другой; на первом же кадре после касания чужих подписей xy нет, потом рендер их публикует заново.
    let uv_spot = segment_centre(frames.cie_toggle, 2, 1);
    for phase in [TouchPhase::Began, TouchPhase::Ended] {
        let delivery = rig.touch(phase, (uv_spot.x * PX_PER_PT) as f32, (uv_spot.y * PX_PER_PT) as f32);
        assert_eq!(delivery, InputDelivery::Queued);
    }
    rig.frame();
    let ScopeIntent::Show(sent) = *rig.scope_intents.last().expect("запрос") else {
        panic!("ожидалось «показать»");
    };
    assert_eq!(
        sent.cie_diagram,
        CieDiagram::UvPrime1976,
        "палец по u'v' просит эту шкалу"
    );
    rig.frames(5);
    let uv = live_scope_request(&mut rig);
    assert_eq!(uv, sent);
    match rig.bench.scope_labels.latest() {
        ScopeLabelsState::Published(labels) => assert_eq!(labels.request, uv, "подписи пересчитаны под u'v'"),
        other => panic!("после смены шкалы подписи не опубликованы: {other:?}"),
    }
    assert_eq!(
        rig.drawn_caption_origins("P3").len(),
        1,
        "«P3» нарисована для новой шкалы"
    );
    rig.finish();
}
