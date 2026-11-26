//! Пересборка раздела калибровки посреди жеста (поворот телефона): уходящий раздел сам заканчивает свой жест чистым
//! `Cancelled`, новый раздел чужих событий не получает, а следующее касание ведёт линейку как обычно.

use look_contract::{GlobalKnob, LookIntent};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{center, drag_eye, portrait};
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::look_test_support::{look_facts, look_intents};

const FRAME: i64 = 16;

/// Сколько намерений `SetKnob` по ручке CONTRAST ушло в этих эффектах.
fn contrast_sets(effects: &[crate::effect::Effect]) -> usize {
    look_intents(effects)
        .into_iter()
        .filter(|intent| {
            matches!(
                intent,
                LookIntent::SetKnob {
                    knob: GlobalKnob::Contrast,
                    ..
                }
            )
        })
        .count()
}

/// Палец ведёт линейку CONTRAST влево и держит там, не отпуская.
fn hold_ruler_mid_drag(rig: &mut Rig) -> Point {
    let ruler = center(rig, "calib_ruler", 0);
    let delta = super::screen_calibration_tests::surface_vector(rig, Vector::new(-30.0, 0.0));
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, ruler, start);
    for step in 1..=12_i64 {
        let t = step as f64 / 12.0;
        let point = Point::new(ruler.x + delta.dx * t, ruler.y + delta.dy * t);
        rig.touch(TouchPhase::Moved, point, start + step * FRAME);
    }
    rig.now_ms = start + 12 * FRAME;
    rig.frame();
    Point::new(ruler.x + delta.dx, ruler.y + delta.dy)
}

#[test]
fn turning_the_phone_mid_drag_cancels_the_drag_cleanly_and_the_new_section_gets_nothing_foreign() {
    let mut rig = portrait(look_facts());
    let held_at = hold_ruler_mid_drag(&mut rig);
    let before_turn = rig.effects();
    assert!(contrast_sets(&before_turn) > 0, "палец довёл линейку до значения");
    let before = rig.screen.diagnostics();
    assert_eq!(before.gestures.cancelled, 0);

    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    // Палец ещё на экране: продолжает двигаться и потом отпускает, пока раздел уже пересобран.
    rig.touch(TouchPhase::Moved, held_at, rig.now_ms + FRAME);
    rig.now_ms += 2 * FRAME;
    rig.frame();
    rig.touch(TouchPhase::Ended, held_at, rig.now_ms + FRAME);
    rig.now_ms += 2 * FRAME;
    rig.frame();
    rig.run(600);

    let after = rig.screen.diagnostics();
    assert_eq!(
        after.gestures.cancelled, 1,
        "старый жест закончился ровно одним Cancelled"
    );
    assert_eq!(
        after.orphan_pan_continuations, 0,
        "новая строка чужих событий не получала"
    );
    assert_eq!(after.unhandled_gestures, 0);
    assert_eq!(after.frame_failures, 0);
    assert_eq!(
        contrast_sets(&rig.effects()),
        0,
        "отмена без действия: значение осталось там, куда дошло, и дальше не менялось"
    );
    // Палец в старом разделе закончил жест: в новом ничего не растворено.
    let drawn = rig.frame().clone();
    assert_eq!(
        texts_of(&drawn, "MIDTONES", TextRoleKind::CardTitle).len(),
        1,
        "другие строки на месте"
    );
    assert_eq!(
        texts_of(&drawn, "SCALES", TextRoleKind::SegmentLabel).len(),
        1,
        "переключатель на месте"
    );
}

#[test]
fn a_new_touch_after_the_turn_drives_the_rebuilt_ruler_normally() {
    let mut rig = portrait(look_facts());
    let held_at = hold_ruler_mid_drag(&mut rig);
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.touch(TouchPhase::Ended, held_at, rig.now_ms + FRAME);
    rig.now_ms += 2 * FRAME;
    rig.run(600);
    rig.effects();

    let ruler = center(&rig, "calib_ruler", 0);
    drag_eye(&mut rig, ruler, Vector::new(-30.0, 0.0));
    assert!(
        contrast_sets(&rig.effects()) > 0,
        "новое касание ведёт линейку новой раскладки: намерение ручки ушло"
    );
    assert_eq!(rig.screen.diagnostics().orphan_pan_continuations, 0);
}

/// Строки и переключатель видны целиком (ничего не растворено), и вторая строка ведётся пальцем.
fn assert_column_fully_alive(rig: &mut Rig) {
    let drawn = rig.frame().clone();
    for title in ["CONTRAST", "MIDTONES"] {
        assert_eq!(
            texts_of(&drawn, title, TextRoleKind::CardTitle).len(),
            1,
            "строка {title} на месте"
        );
    }
    assert_eq!(texts_of(&drawn, "SCALES", TextRoleKind::SegmentLabel).len(), 1);
    assert_eq!(texts_of(&drawn, "WHEELS", TextRoleKind::SegmentLabel).len(), 1);
    rig.effects();
    let midtones = center(rig, "calib_ruler", 1);
    drag_eye(rig, midtones, Vector::new(-30.0, 0.0));
    let moved_midtones = look_intents(&rig.effects()).into_iter().any(|intent| {
        matches!(
            intent,
            LookIntent::SetKnob {
                knob: GlobalKnob::Midtones,
                ..
            }
        )
    });
    assert!(
        moved_midtones,
        "строка MIDTONES принимает касания: ничего не осталось выключенным"
    );
}

#[test]
fn opening_a_bottom_card_mid_drag_leaves_no_faded_or_dead_controls_after_the_release() {
    let mut rig = portrait(look_facts());
    let held_at = hold_ruler_mid_drag(&mut rig);
    // Вторым пальцем карточку не открыть (капсула белого под рукой ведущего пальца) — открываем действием.
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.touch(TouchPhase::Ended, held_at, rig.now_ms + FRAME);
    rig.now_ms += 2 * FRAME;
    rig.run(400);
    rig.act(ScreenAction::ControlsMenuCloseRequested);
    rig.run(600);
    assert_eq!(rig.screen.diagnostics().frame_failures, 0);
    assert_column_fully_alive(&mut rig);
    assert_eq!(rig.screen.diagnostics().finger_focus_refusals, 0);
}

#[test]
fn switching_scales_and_wheels_mid_drag_leaves_no_faded_or_dead_controls_after_the_release() {
    use crate::calibration::ControlViewKind;
    let mut rig = portrait(look_facts());
    let held_at = hold_ruler_mid_drag(&mut rig);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    // Палец ещё не отпущен, но взятая им линейка ушла вместе с видом: переключатель вида не растворён.
    rig.run(400);
    let switched = rig.frame().clone();
    assert_eq!(
        texts_of(&switched, "WHEELS", TextRoleKind::SegmentLabel).len(),
        1,
        "под отменённым жестом переключатель виден"
    );
    rig.touch(TouchPhase::Ended, held_at, rig.now_ms + FRAME);
    rig.now_ms += 2 * FRAME;
    rig.run(600);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Scales));
    rig.run(600);
    assert_eq!(rig.screen.diagnostics().frame_failures, 0);
    assert_column_fully_alive(&mut rig);
    assert_eq!(rig.screen.diagnostics().orphan_pan_continuations, 0);
}
