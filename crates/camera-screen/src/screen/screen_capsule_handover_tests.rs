//! Капсула режимов при перетаскивании между PHOTO и разделом калибровки: подписи не «прыгают дважды».
//!
//! Капсула живёт в двух домах с отдельным состоянием прокрутки (см. `capsule_handover`). Палец тащит только видимый
//! дом, режим меняется при отпускании, и показывается второй дом. Проверяется то, что видит владелец: положение
//! подписи нужного режима вдоль оси капсулы кадр за кадром. Через смену режима оно непрерывно (шаг в смену не
//! больше соседних), не возвращается назад и приходит ровно в покой нужного режима.

use look_contract::LookFacts;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_core::time::MotionPreference;
use ui_draw::draw_list::DrawList;
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{calibrating, surface_vector};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::look_test_support::look_facts;
use crate::modes::CaptureMode;

const FRAME: i64 = FRAME_MS;
/// Допуск на покой подписи, px: хвост пружины заканчивается на доли пикселя.
const REST_TOLERANCE_PX: f32 = 0.6;
/// Допуск на перелёт покоя пружиной с унаследованной скоростью броска, px.
const OVERSHOOT_TOLERANCE_PX: f32 = 8.0;

/// Как отпускаем палец.
#[derive(Clone, Copy, Debug)]
enum Release {
    /// Палец останавливается и держится, потом отпускается (скорость отпускания нулевая).
    AfterHold,
    /// Быстрый бросок: отрыв на ходу.
    Fling,
}

/// След подписи по кадрам: положение вдоль оси капсулы (px) и кадр, в который сменился режим.
struct LabelTrace {
    positions: Vec<Option<f32>>,
    flip_frame: usize,
}

/// Положение подписи `label` вдоль её оси относительно центра капсулы режимов, px; `None` — подписи нет в кадре.
fn label_position(list: &DrawList, label: &str) -> Option<f32> {
    let capsule = panel_members(list).into_iter().find(|(shape, ..)| {
        matches!(shape, ShapeGeometry::Capsule { size } if size.height > px(40.0) && size.height < px(60.0))
    })?;
    let text = texts_of(list, label, TextRoleKind::ModeLabel).into_iter().next()?;
    // Ось строки: образ локальной оси x (столбец `a`, `c` преобразования) (в повёрнутом разделе это глазная горизонталь).
    let axis_length = text.transform.a.hypot(text.transform.c);
    let (axis_x, axis_y) = (text.transform.a / axis_length, text.transform.c / axis_length);
    let half_width = text.run.advance_px / 2.0;
    let centre_x = text.transform.tx + axis_x * half_width;
    let centre_y = text.transform.ty + axis_y * half_width;
    Some((centre_x - capsule.1) * axis_x + (centre_y - capsule.2) * axis_y)
}

/// Ведёт палец от `from` на `delta` (pt поверхности), записывая подпись `label` после каждого кадра; затем ждёт
/// покоя. Реальные `Began` / `Moved` / `Ended` через экран.
fn drag_and_trace(rig: &mut Rig, from: Point, delta: Vector, release: Release, label: &str) -> LabelTrace {
    let mode_before = rig.screen.state().mode();
    let mut positions = Vec::new();
    let mut flip_frame = None;
    let mut step_frame = |rig: &mut Rig, positions: &mut Vec<Option<f32>>| {
        rig.now_ms += FRAME;
        let list = rig.frame().clone();
        if flip_frame.is_none() && rig.screen.state().mode() != mode_before {
            flip_frame = Some(positions.len());
        }
        positions.push(label_position(&list, label));
    };
    let (steps, hold_frames) = match release {
        Release::AfterHold => (24, 12),
        Release::Fling => (8, 0),
    };
    rig.touch(TouchPhase::Began, from, rig.now_ms);
    step_frame(rig, &mut positions);
    let mut at = Point::new(from.x, from.y);
    for step in 1..=steps {
        let t = f64::from(steps).recip() * f64::from(step);
        at = Point::new(from.x + delta.dx * t, from.y + delta.dy * t);
        rig.touch(TouchPhase::Moved, at, rig.now_ms + FRAME);
        step_frame(rig, &mut positions);
    }
    for _ in 0..hold_frames {
        rig.touch(TouchPhase::Moved, at, rig.now_ms + FRAME);
        step_frame(rig, &mut positions);
    }
    rig.touch(TouchPhase::Ended, at, rig.now_ms + FRAME);
    for _ in 0..90 {
        step_frame(rig, &mut positions);
    }
    LabelTrace {
        positions,
        flip_frame: flip_frame.expect("режим сменился при отпускании"),
    }
}

/// Шаги между соседними кадрами, где подпись есть в обоих.
fn steps_of(trace: &LabelTrace) -> Vec<(usize, f32)> {
    trace
        .positions
        .windows(2)
        .enumerate()
        .filter_map(|(index, pair)| Some((index + 1, pair[1]? - pair[0]?)))
        .collect()
}

/// Подпись непрерывна через смену режима, не идёт назад и приходит ровно в `rest_px`.
fn assert_smooth_handover(trace: &LabelTrace, rest_px: f32, release: Release, context: &str) {
    let steps = steps_of(trace);
    let flip = trace.flip_frame;
    // Подпись видна вокруг смены режима: иначе проверять нечего.
    for frame in flip.saturating_sub(2)..=flip + 2 {
        assert!(
            trace.positions[frame].is_some(),
            "{context}: подписи нет в кадре {frame}"
        );
    }
    let step_at = |frame: usize| {
        steps
            .iter()
            .find(|(index, _)| *index == frame)
            .map(|(_, step)| step.abs())
    };
    let around = [flip.saturating_sub(1), flip + 1, flip + 2]
        .into_iter()
        .filter_map(step_at)
        .fold(0.0_f32, f32::max);
    for frame in [flip, flip + 1] {
        let step = step_at(frame).expect("шаг в кадре смены");
        assert!(
            step <= 1.5 * around + 1.0,
            "{context}: шаг {step} px в кадре {frame} (смена режима в {flip}) больше соседних {around} px"
        );
    }
    // Нет отката: после смены режима подпись не уходит от покоя дальше, чем была в момент смены (пружина с унаследованной
    // скоростью броска может чуть перелететь покой и вернуться: перелёт ограничен). Откат на ширину прежнего хода
    // (двойной прыжок) на порядок больше допуска.
    // Бросок вправе улететь за покой далеко (скорость пальца большая), поэтому откат проверяется у остановленного пальца,
    // а у броска непрерывность шага и приход в покой.
    if matches!(release, Release::Fling) {
        let end = trace.positions.last().copied().flatten().expect("подпись в конце");
        assert!(
            (end - rest_px).abs() < REST_TOLERANCE_PX,
            "{context}: бросок остановился в {end} px, покой {rest_px} px"
        );
        return;
    }
    let at_flip = (trace.positions[flip].expect("подпись в смене") - rest_px).abs();
    let farthest = trace.positions[flip..]
        .iter()
        .flatten()
        .map(|position| (position - rest_px).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        farthest <= at_flip + OVERSHOOT_TOLERANCE_PX,
        "{context}: после смены режима подпись откатилась: в смене {at_flip} px от покоя, дальше всего {farthest} px"
    );
    let end = trace.positions.last().copied().flatten().expect("подпись в конце");
    assert!(
        (end - rest_px).abs() < REST_TOLERANCE_PX,
        "{context}: подпись остановилась в {end} px, покой {rest_px} px"
    );
}

const ORIENTATIONS: [DeviceOrientation; 3] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
];

/// Экран в PHOTO в ориентации `orientation`, всё осело.
fn photo_rig(orientation: DeviceOrientation) -> Rig {
    let mut rig = Rig::with(ScreenConfig::default(), MotionPreference::Full);
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(look_facts() as LookFacts)));
    rig.screen
        .handle_action(ScreenAction::DeviceOrientationChanged(orientation));
    rig.run(800);
    rig.effects();
    rig
}

/// Покой подписи `label` в доме с выбранным `mode`: берётся из спокойного кадра.
fn rest_of(rig: &mut Rig, label: &str) -> f32 {
    rig.run(1200);
    let list = rig.frame().clone();
    label_position(&list, label).expect("подпись в покое")
}

fn section_to_photo(orientation: DeviceOrientation, release: Release) {
    let mut rig = calibrating(look_facts(), orientation);
    let capsule = *rig
        .screen
        .node_centers("mode_capsule")
        .first()
        .expect("капсула раздела");
    // Глазом вправо: из CALIBRATE (последний) в PHOTO.
    let delta = surface_vector(&rig, Vector::new(90.0, 0.0));
    let trace = drag_and_trace(&mut rig, capsule, delta, release, "PHOTO");
    assert_eq!(rig.screen.state().mode(), CaptureMode::Photo);
    let rest = rest_of(&mut rig, "PHOTO");
    assert_smooth_handover(
        &trace,
        rest,
        release,
        &format!("раздел → PHOTO, {orientation:?}, {release:?}"),
    );
}

fn photo_to_section(orientation: DeviceOrientation, release: Release) {
    let mut rig = photo_rig(orientation);
    let capsule = *rig.screen.node_centers("mode_capsule").first().expect("капсула PHOTO");
    // Палец влево: из PHOTO в CALIBRATE.
    let trace = drag_and_trace(&mut rig, capsule, Vector::new(-90.0, 0.0), release, "CALIBRATE");
    assert_eq!(rig.screen.state().mode(), CaptureMode::Calibrate);
    let rest = rest_of(&mut rig, "CALIBRATE");
    assert_smooth_handover(
        &trace,
        rest,
        release,
        &format!("PHOTO → раздел, {orientation:?}, {release:?}"),
    );
}

#[test]
fn dragging_the_section_capsule_to_photo_keeps_the_label_continuous_in_every_orientation() {
    for orientation in ORIENTATIONS {
        for release in [Release::AfterHold, Release::Fling] {
            section_to_photo(orientation, release);
        }
    }
}

#[test]
fn dragging_the_photo_capsule_to_calibrate_keeps_the_label_continuous_in_every_orientation() {
    for orientation in ORIENTATIONS {
        for release in [Release::AfterHold, Release::Fling] {
            photo_to_section(orientation, release);
        }
    }
}

/// Тап и выбор режима действием (не жестом по капсуле) остаются плавными: подпись идёт к покою без разворота.
#[test]
fn tap_and_mode_selected_still_glide_without_a_reversal() {
    for orientation in ORIENTATIONS {
        for via_tap in [true, false] {
            let mut rig = photo_rig(orientation);
            let capsule = *rig.screen.node_centers("mode_capsule").first().expect("капсула");
            let rest_photo = rest_of(&mut rig, "PHOTO");
            if via_tap {
                rig.tap(Point::new(capsule.x + 90.0, capsule.y));
            } else {
                rig.screen
                    .handle_action(ScreenAction::ModeSelected(CaptureMode::Calibrate));
            }
            let mut track = Vec::new();
            for _ in 0..90 {
                rig.now_ms += FRAME;
                let list = rig.frame().clone();
                track.push(label_position(&list, "CALIBRATE"));
            }
            assert_eq!(
                rig.screen.state().mode(),
                CaptureMode::Calibrate,
                "{orientation:?} tap={via_tap}"
            );
            let positions: Vec<f32> = track.into_iter().flatten().collect();
            let steps: Vec<f32> = positions.windows(2).map(|pair| pair[1] - pair[0]).collect();
            let direction = steps
                .iter()
                .copied()
                .max_by(|a, b| a.abs().total_cmp(&b.abs()))
                .expect("шаги");
            assert!(
                steps.iter().all(|step| step * direction.signum() >= -0.3),
                "{orientation:?} tap={via_tap}: разворот подписи"
            );
            let _ = rest_photo;
        }
    }
}
