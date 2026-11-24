//! Тесты решения «рисовать ли тик» (без GPU): каждое условие, запрещающее пропуск, и каждый вход, меняющий пиксели.

use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, DrawList, ShapeItem};
use ui_draw::frame_source::BackdropContinuity;
use ui_draw::geometry::PxSize;
use ui_draw::light::LightDirection;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;

use super::*;
use crate::config::{UnchangedFrameConfig, UnchangedFramePolicy};
use crate::viewfinder::HoldEnd;
use crate::vulkan_backend::camera::continuity::FramePlan;
use crate::vulkan_backend::camera::pipelines::ReadyUi;
use crate::vulkan_backend::camera::session::transition::TransitionFrame;

// ---- построители входов ----

fn circle(diameter: f32) -> DrawItem {
    DrawItem::Shape(ShapeItem {
        geometry: ShapeGeometry::Circle { diameter },
        transform: Affine2::IDENTITY,
        fill: DisplayColor {
            r: 1.0,
            g: 1.0,
            b: 1.0,
            a: 1.0,
        },
        stroke: None,
        shadow: None,
        opacity: Opacity::OPAQUE,
    })
}

fn list(light_degrees: f32, diameters: &[f32]) -> DrawList {
    let mut list = DrawList::new(LightDirection::from_angle_degrees(light_degrees));
    list.items = diameters.iter().copied().map(circle).collect();
    list
}

fn ui_from(draw_list: DrawList) -> UiTick {
    UiTick::Ready(ReadyUi::new(
        draw_list,
        PxSize::new(400.0, 800.0),
        2.0,
        BackdropContinuity::Live,
    ))
}

fn ui() -> UiTick {
    ui_from(list(45.0, &[10.0, 20.0]))
}

fn gate(policy: UnchangedFramePolicy, max_consecutive_skips: u32) -> UnchangedFrameGate {
    UnchangedFrameGate::new(UnchangedFrameConfig {
        policy,
        max_consecutive_skips,
    })
}

/// Шлюз, у которого эталон — нарисованный чистый живой кадр с UI `ui()`.
fn gate_with_baseline() -> UnchangedFrameGate {
    let mut gate = gate(UnchangedFramePolicy::SkipWhenUnchanged, 8);
    gate.note_drawn(ui(), DrawnFrame::CleanLive);
    gate
}

// ---- решение ----

#[test]
fn quiet_recompose_with_the_same_ui_as_the_shown_frame_is_skipped() {
    let mut gate = gate_with_baseline();
    assert_eq!(gate.judge(TickKind::QuietLiveRecompose, &ui()), FrameVerdict::Skip);
}

#[test]
fn every_input_that_changes_the_ui_pixels_forces_a_draw() {
    let changed_inputs = [
        ("добавлен элемент", ui_from(list(45.0, &[10.0, 20.0, 30.0]))),
        ("изменён элемент", ui_from(list(45.0, &[10.0, 21.0]))),
        ("повёрнут свет", ui_from(list(46.0, &[10.0, 20.0]))),
        (
            "другой размер цели",
            UiTick::Ready(ReadyUi::new(
                list(45.0, &[10.0, 20.0]),
                PxSize::new(400.0, 801.0),
                2.0,
                BackdropContinuity::Live,
            )),
        ),
        (
            "другой масштаб pt",
            UiTick::Ready(ReadyUi::new(
                list(45.0, &[10.0, 20.0]),
                PxSize::new(400.0, 800.0),
                2.5,
                BackdropContinuity::Live,
            )),
        ),
        (
            "другой фон под UI",
            UiTick::Ready(ReadyUi::new(
                list(45.0, &[10.0, 20.0]),
                PxSize::new(400.0, 800.0),
                2.0,
                BackdropContinuity::NoCameraFrame,
            )),
        ),
        ("UI исчез", UiTick::Inactive),
    ];
    for (what, changed) in changed_inputs {
        let mut gate = gate_with_baseline();
        assert_eq!(
            gate.judge(TickKind::QuietLiveRecompose, &changed),
            FrameVerdict::Draw(DrawReason::UiChanged),
            "{what}"
        );
    }
}

#[test]
fn ui_appearing_over_a_frame_without_ui_forces_a_draw() {
    let mut gate = gate(UnchangedFramePolicy::SkipWhenUnchanged, 8);
    gate.note_drawn(UiTick::Inactive, DrawnFrame::CleanLive);
    assert_eq!(
        gate.judge(TickKind::QuietLiveRecompose, &UiTick::Inactive),
        FrameVerdict::Skip,
        "без UI на обоих кадрах — пропуск"
    );
    assert_eq!(
        gate.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Draw(DrawReason::UiChanged)
    );
}

#[test]
fn tick_kinds_that_change_the_picture_are_never_skipped() {
    for (kind, reason) in [
        (TickKind::NewCameraFrame, DrawReason::NewCameraFrame),
        (TickKind::GpuWorkSuppressed, DrawReason::GpuWorkSuppressed),
        (TickKind::Other, DrawReason::NotQuietLiveRecompose),
    ] {
        let mut gate = gate_with_baseline();
        assert_eq!(gate.judge(kind, &ui()), FrameVerdict::Draw(reason), "{kind:?}");
    }
}

#[test]
fn always_redraw_policy_draws_even_a_perfectly_unchanged_tick() {
    let mut gate = gate(UnchangedFramePolicy::AlwaysRedraw, 8);
    gate.note_drawn(ui(), DrawnFrame::CleanLive);
    assert_eq!(
        gate.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Draw(DrawReason::PolicyAlwaysRedraw)
    );
}

#[test]
fn ui_that_could_not_be_produced_is_never_compared() {
    for unavailable in [UiTick::Unavailable, UiTick::Suppressed] {
        let mut gate = gate_with_baseline();
        assert_eq!(
            gate.judge(TickKind::QuietLiveRecompose, &unavailable),
            FrameVerdict::Draw(DrawReason::UiUnavailable)
        );
    }
}

#[test]
fn without_a_trustworthy_shown_frame_the_first_tick_is_drawn() {
    let mut fresh = gate(UnchangedFramePolicy::SkipWhenUnchanged, 8);
    assert_eq!(
        fresh.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Draw(DrawReason::NoBaseline),
        "первого кадра ещё не было"
    );

    let mut invalidated = gate_with_baseline();
    invalidated.invalidate();
    assert_eq!(
        invalidated.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Draw(DrawReason::NoBaseline),
        "поверхность пересоздана или показ не удался"
    );

    let mut unreliable = gate(UnchangedFramePolicy::SkipWhenUnchanged, 8);
    unreliable.note_drawn(ui(), DrawnFrame::Unreliable);
    assert_eq!(
        unreliable.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Draw(DrawReason::NoBaseline),
        "кадр с ошибкой записи, чёрный или с оборванным UI эталоном не служит"
    );

    let mut after_unavailable = gate(UnchangedFramePolicy::SkipWhenUnchanged, 8);
    after_unavailable.note_drawn(UiTick::Unavailable, DrawnFrame::CleanLive);
    assert_eq!(
        after_unavailable.judge(TickKind::QuietLiveRecompose, &UiTick::Unavailable),
        FrameVerdict::Draw(DrawReason::UiUnavailable)
    );
    assert_eq!(
        after_unavailable.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Draw(DrawReason::NoBaseline),
        "кадр без UI из-за отказа на экране выглядит неизвестно как: эталоном не стал"
    );
}

#[test]
fn a_viewfinder_command_makes_the_shown_frame_stale() {
    let mut gate = gate_with_baseline();
    gate.note_scene_changed();
    assert_eq!(
        gate.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Draw(DrawReason::SceneChanged)
    );
    // Нарисовали заново при новых настройках: тот же UI снова можно пропускать.
    gate.note_drawn(ui(), DrawnFrame::CleanLive);
    assert_eq!(gate.judge(TickKind::QuietLiveRecompose, &ui()), FrameVerdict::Skip);
}

#[test]
fn a_run_of_skips_is_cut_by_the_limit_and_restarts_after_a_draw() {
    let mut gate = gate(UnchangedFramePolicy::SkipWhenUnchanged, 3);
    gate.note_drawn(ui(), DrawnFrame::CleanLive);
    for _ in 0..3 {
        assert_eq!(gate.judge(TickKind::QuietLiveRecompose, &ui()), FrameVerdict::Skip);
    }
    assert_eq!(
        gate.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Draw(DrawReason::SkipRunLimit),
        "четвёртый тик подряд рисуется: нужен слот кольца для итогов кадров"
    );
    gate.note_drawn(ui(), DrawnFrame::CleanLive);
    assert_eq!(
        gate.judge(TickKind::QuietLiveRecompose, &ui()),
        FrameVerdict::Skip,
        "после отрисовки серия началась заново"
    );
}

// ---- классификация тика по решению prepare ----

fn plan(screen: Screen) -> FramePlan {
    FramePlan {
        screen,
        begins_hold: None,
        ends_hold: None,
        held_since: None,
        magnification: 1.0,
    }
}

fn transition(event: StreamEvent, plan: FramePlan) -> TransitionFrame {
    TransitionFrame {
        event,
        plan,
        picture: None,
        crop: None,
    }
}

fn recompose(change: FrameChange, transition: Option<TransitionFrame>) -> Prepared {
    Prepared {
        content: Content::Recompose,
        change,
        transition,
        new_lease: None,
        camera_wait: None,
        stream_inactive: false,
    }
}

fn quiet_live() -> Prepared {
    recompose(
        FrameChange::Keep,
        Some(transition(StreamEvent::Quiet, plan(Screen::Live))),
    )
}

fn classify(prepared: &Prepared) -> TickKind {
    classify_tick(prepared, ViewfinderPainter::Isp, GpuWorkSuppression::NONE)
}

#[test]
fn only_a_quiet_live_recompose_without_hold_changes_is_a_skip_candidate() {
    assert_eq!(classify(&quiet_live()), TickKind::QuietLiveRecompose);

    let mut inactive = quiet_live();
    inactive.stream_inactive = true;
    assert_eq!(classify(&inactive), TickKind::Other, "поток закрыт");

    assert_eq!(
        classify(&recompose(
            FrameChange::Clear,
            Some(transition(StreamEvent::Quiet, plan(Screen::Live)))
        )),
        TickKind::Other,
        "с экрана уходит кадр мозаики: сабмит обязан его вернуть"
    );
    assert_eq!(
        classify(&recompose(FrameChange::Keep, None)),
        TickKind::Other,
        "без решения непрерывности"
    );
    for (what, event) in [
        ("кадр пришёл, но показать нельзя", StreamEvent::Unpresentable),
        ("поток закрылся", StreamEvent::Inactive),
    ] {
        assert_eq!(
            classify(&recompose(
                FrameChange::Keep,
                Some(transition(event, plan(Screen::Live)))
            )),
            TickKind::Other,
            "{what}"
        );
    }
    for screen in [Screen::Held, Screen::Black] {
        assert_eq!(
            classify(&recompose(
                FrameChange::Keep,
                Some(transition(StreamEvent::Quiet, plan(screen)))
            )),
            TickKind::Other,
            "{screen:?}: удержанный и чёрный кадры живут по времени"
        );
    }
    let ending_hold = FramePlan {
        ends_hold: Some(HoldEnd::Condition),
        ..plan(Screen::Live)
    };
    assert_eq!(
        classify(&recompose(
            FrameChange::Keep,
            Some(transition(StreamEvent::Quiet, ending_hold))
        )),
        TickKind::Other,
        "тик, которым кончается удержание, меняет машину непрерывности"
    );
}

#[test]
fn painter_other_than_isp_and_gpu_suppression_are_never_skip_candidates() {
    assert_eq!(
        classify_tick(&quiet_live(), ViewfinderPainter::DebugMosaic, GpuWorkSuppression::NONE),
        TickKind::Other
    );
    for work in [SuppressibleGpuWork::ColorScopes, SuppressibleGpuWork::UiOverlayDrawing] {
        assert_eq!(
            classify_tick(
                &quiet_live(),
                ViewfinderPainter::Isp,
                GpuWorkSuppression::NONE.suppressing(work)
            ),
            TickKind::GpuWorkSuppressed,
            "{work:?}"
        );
    }
}

#[test]
fn black_frames_are_not_skip_candidates() {
    let black = Prepared {
        content: Content::Black,
        change: FrameChange::Keep,
        transition: Some(transition(StreamEvent::Quiet, plan(Screen::Black))),
        new_lease: None,
        camera_wait: None,
        stream_inactive: false,
    };
    assert_eq!(classify(&black), TickKind::Other);
}
