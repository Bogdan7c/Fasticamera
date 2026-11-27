use std::sync::{Arc, Mutex};

use isp_vulkan::viewfinder::{IspStage, IspStageSet};
use render_loop::config::{AdpfMode, DepthReductionMode, LateLatchMode, PresentTimelinePolicy, RenderLoopConfig};
use render_loop::viewfinder::{
    GpuWorkSuppression, HeadroomBench, SuppressibleGpuWork, ViewfinderCommand, ViewfinderPainter,
};
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};

use super::*;
use crate::gpu_services::UnavailableDiagnostics;
use crate::gpu_test_support::{FakeRenderStatus, RecordedUiOverlay, RecordingRenderControl, presenting_status};

/// Оболочка, которая только запоминает просьбы о частоте экрана.
#[derive(Default)]
pub(super) struct RateRecorder {
    pub(super) rates: Mutex<Vec<f32>>,
}

impl ShellCommands for RateRecorder {
    fn request_camera_permission(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, rate: FrameRate) -> FrameRateOutcome {
        self.rates.lock().expect("замок").push(rate.hertz());
        FrameRateOutcome::Applied
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn keep_screen_on(&self, _wake: ScreenWake) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

fn gpu_with(control: Arc<dyn RenderControl>, render_config: RenderLoopConfig) -> GpuServices {
    GpuServices::new(
        FakeRenderStatus::live(presenting_status(), crate::gpu_services::ValidationState::Enabled),
        Arc::new(UnavailableDiagnostics::new("не нужны")),
        render_config,
        control,
    )
}

#[test]
fn untouched_render_is_not_restored_and_no_command_is_sent() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    let shell = RateRecorder::default();

    let effects = gpu.render_scenario_ended(&shell);

    assert!(effects.is_empty());
    assert!(
        control.recorded().is_empty(),
        "сценарий рендер не трогал — возвращать нечего"
    );
    assert!(
        shell.rates.lock().expect("замок").is_empty(),
        "частоту экрана не трогали"
    );
}

#[test]
fn scenario_adjustments_reach_the_control_as_intent_commands() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    let without_tone = IspStageSet::ALL.without(IspStage::Tone);
    let nearest = PresentTargetSetting {
        policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1500 },
        late_latch: LateLatchMode::BeforeCameraPoll,
        depth_reduction: DepthReductionMode::SkipOneVsync,
    };

    assert!(gpu.set_isp_stages_for_scenario(without_tone).is_delivered());
    assert!(
        gpu.set_viewfinder_painter_for_scenario(ViewfinderPainter::DebugMosaic)
            .is_delivered()
    );
    assert!(gpu.set_present_target_for_scenario(nearest).is_delivered());
    assert!(
        gpu.set_headroom_bench_for_scenario(HeadroomBench::Enabled)
            .is_delivered()
    );

    assert_eq!(
        control.recorded(),
        vec![
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(without_tone)),
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(ViewfinderPainter::DebugMosaic)),
            RenderAdjustment::PresentTarget(nearest),
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Enabled)),
        ]
    );
}

#[test]
fn end_restores_only_touched_settings_to_configured_values_once() {
    let control = RecordingRenderControl::new();
    let mut render_config = RenderLoopConfig::default();
    render_config.present_target.policy = PresentTimelinePolicy::NearestFeasible { margin_us: 900 };
    render_config.present_target.depth_reduction = DepthReductionMode::Disabled;
    render_config.preferred_frame_rate_hz = 60.0;
    let gpu = gpu_with(control.clone(), render_config);
    let shell = RateRecorder::default();
    gpu.set_isp_stages_for_scenario(IspStageSet::ALL.without(IspStage::Demosaic));
    gpu.set_present_target_for_scenario(PresentTargetSetting {
        policy: PresentTimelinePolicy::Preferred,
        late_latch: LateLatchMode::Disabled,
        depth_reduction: DepthReductionMode::SkipOneVsync,
    });
    gpu.request_display_refresh_for_scenario(&shell, 120.0)
        .expect("частота верна");
    let before_end = control.recorded().len();

    let effects = gpu.render_scenario_ended(&shell);

    // Рисовальщик и стенд не трогали — их не возвращаем; стадии и линия кадра — к конфигу.
    assert_eq!(
        control.recorded()[before_end..],
        [
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetIspStages(IspStageSet::ALL)),
            RenderAdjustment::PresentTarget(PresentTargetSetting {
                policy: PresentTimelinePolicy::NearestFeasible { margin_us: 900 },
                // Поздний захват и сход на ближнюю линию вернулись к конфигу рендера, а не к тому,
                // что ставил сценарий (там было Disabled и SkipOneVsync).
                late_latch: LateLatchMode::BeforeCameraPoll,
                depth_reduction: DepthReductionMode::Disabled,
            }),
        ]
    );
    assert_eq!(
        effects.adjustments.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["isp_stages", "present_target"]
    );
    assert_eq!(effects.display_refresh, Some(FrameRateOutcome::Applied));
    assert_eq!(*shell.rates.lock().expect("замок"), [120.0, 60.0]);

    // Учёт забран: второй конец ничего не шлёт.
    let again = gpu.render_scenario_ended(&shell);
    assert!(again.is_empty());
    assert_eq!(control.recorded().len(), before_end + 2);
}

#[test]
fn bench_is_switched_off_first_and_painter_returns_to_the_configured_one() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    gpu.set_viewfinder_painter_for_scenario(ViewfinderPainter::DebugMosaic);
    gpu.set_headroom_bench_for_scenario(HeadroomBench::Enabled);
    let before_end = control.recorded().len();

    gpu.render_scenario_ended(&RateRecorder::default());

    assert_eq!(
        control.recorded()[before_end..],
        [
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetHeadroomBench(HeadroomBench::Disabled)),
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(
                RenderLoopConfig::default().viewfinder.painter
            )),
        ]
    );
}

#[test]
fn invalid_refresh_is_an_error_and_is_not_counted_as_touched() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control, RenderLoopConfig::default());
    let shell = RateRecorder::default();

    assert!(gpu.request_display_refresh_for_scenario(&shell, f32::NAN).is_err());
    let effects = gpu.render_scenario_ended(&shell);

    assert!(
        effects.is_empty(),
        "неверная частота не дошла до платформы — возвращать нечего"
    );
    assert!(shell.rates.lock().expect("замок").is_empty());
}

#[test]
fn unavailable_render_answers_every_adjustment_with_its_reason() {
    let gpu = gpu_with(
        Arc::new(UnavailableRenderControl::new("GPU не поднялся")),
        RenderLoopConfig::default(),
    );
    let expected = ControlDelivery::RenderUnavailable {
        reason: "GPU не поднялся".to_owned(),
    };

    assert_eq!(gpu.set_isp_stages_for_scenario(IspStageSet::ALL), expected);
    assert_eq!(gpu.set_headroom_bench_for_scenario(HeadroomBench::Enabled), expected);
    let effects = gpu.render_scenario_ended(&RateRecorder::default());
    assert!(
        effects.adjustments.iter().all(|(_, delivery)| *delivery == expected),
        "{effects:?}"
    );
}

#[test]
fn adpf_adjustment_is_delivered_and_restored_to_the_configured_mode() {
    let control = RecordingRenderControl::new();
    let mut render_config = RenderLoopConfig::default();
    render_config.adpf.mode = AdpfMode::Enabled;
    let gpu = gpu_with(control.clone(), render_config);

    assert!(gpu.set_adpf_for_scenario(AdpfMode::Disabled).is_delivered());
    let effects = gpu.render_scenario_ended(&RateRecorder::default());

    assert_eq!(
        control.recorded(),
        [
            RenderAdjustment::Adpf(AdpfMode::Disabled),
            RenderAdjustment::Adpf(AdpfMode::Enabled),
        ],
        "сценарий выключил ADPF, конец вернул режим из конфига"
    );
    assert_eq!(
        effects.adjustments.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["adpf"]
    );
}

#[test]
fn adpf_is_not_restored_when_the_scenario_did_not_touch_it() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    gpu.set_present_target_for_scenario(PresentTargetSetting {
        policy: PresentTimelinePolicy::Preferred,
        late_latch: LateLatchMode::Disabled,
        depth_reduction: DepthReductionMode::Disabled,
    });

    let effects = gpu.render_scenario_ended(&RateRecorder::default());

    assert!(
        control
            .recorded()
            .iter()
            .all(|adjustment| !matches!(adjustment, RenderAdjustment::Adpf(_))),
        "ADPF не трогали — его не возвращаем: {:?}",
        control.recorded()
    );
    assert_eq!(
        effects.adjustments.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["present_target"]
    );
}

#[test]
fn unavailable_render_reports_reason_for_adpf_too() {
    let gpu = gpu_with(
        Arc::new(UnavailableRenderControl::new("GPU не поднялся")),
        RenderLoopConfig::default(),
    );

    assert_eq!(
        gpu.set_adpf_for_scenario(AdpfMode::Enabled),
        ControlDelivery::RenderUnavailable {
            reason: "GPU не поднялся".to_owned()
        }
    );
}

// ---- UI и стекло поверх видоискателя (сессия 11) ----

/// Источник кадра UI, который ничего не рисует: тестам нужен только факт установки.
struct FakeUiSource;

impl ui_draw::frame_source::UiFrameSource for FakeUiSource {
    fn name(&self) -> &'static str {
        "fake-ui"
    }

    fn produce_frame(
        &mut self,
        _request: &ui_draw::frame_source::UiFrameRequest,
    ) -> Result<ui_draw::draw_list::DrawList, ui_draw::frame_source::UiFrameError> {
        Ok(ui_draw::draw_list::DrawList::new(
            ui_draw::light::LightDirection::from_angle_degrees(0.0),
        ))
    }
}

#[test]
fn installed_ui_source_reaches_the_control_and_is_removed_first_at_the_end_of_the_scenario() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    gpu.set_viewfinder_painter_for_scenario(ViewfinderPainter::DebugMosaic);
    assert!(
        gpu.install_ui_overlay_for_scenario(Box::new(FakeUiSource))
            .is_delivered()
    );
    assert_eq!(control.recorded_ui_overlay(), [RecordedUiOverlay::Install("fake-ui")]);
    let before_end = control.recorded().len();

    let effects = gpu.render_scenario_ended(&RateRecorder::default());

    assert_eq!(
        control.recorded_ui_overlay(),
        [RecordedUiOverlay::Install("fake-ui"), RecordedUiOverlay::Remove],
        "конец сценария снял источник"
    );
    assert_eq!(
        effects.adjustments.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["ui_overlay", "painter"],
        "UI снимается раньше остальных возвратов"
    );
    assert_eq!(
        control.recorded().len(),
        before_end + 1,
        "возврат рисовальщика — отдельная регулировка"
    );

    // Учёт забран: второй конец ничего не шлёт.
    assert!(gpu.render_scenario_ended(&RateRecorder::default()).is_empty());
    assert_eq!(control.recorded_ui_overlay().len(), 2);
}

#[test]
fn ui_source_never_installed_means_the_end_sends_no_ui_command() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    gpu.set_isp_stages_for_scenario(IspStageSet::ALL.without(IspStage::Tone));

    let effects = gpu.render_scenario_ended(&RateRecorder::default());

    assert!(
        control.recorded_ui_overlay().is_empty(),
        "источник не ставили — снимать нечего"
    );
    assert_eq!(
        effects.adjustments.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["isp_stages"]
    );
}

#[test]
fn removing_the_source_inside_the_scenario_leaves_nothing_for_the_end_to_remove() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    gpu.install_ui_overlay_for_scenario(Box::new(FakeUiSource));
    assert!(gpu.remove_ui_overlay_for_scenario().is_delivered());

    let effects = gpu.render_scenario_ended(&RateRecorder::default());

    assert!(effects.is_empty(), "{effects:?}");
    assert_eq!(
        control.recorded_ui_overlay(),
        [RecordedUiOverlay::Install("fake-ui"), RecordedUiOverlay::Remove],
        "снятие — одно, сценарием"
    );
}

#[test]
fn a_new_install_after_removal_is_claimed_again() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    gpu.install_ui_overlay_for_scenario(Box::new(FakeUiSource));
    gpu.remove_ui_overlay_for_scenario();
    gpu.install_ui_overlay_for_scenario(Box::new(FakeUiSource));

    gpu.render_scenario_ended(&RateRecorder::default());

    assert_eq!(
        control.recorded_ui_overlay().last(),
        Some(&RecordedUiOverlay::Remove),
        "второй источник тоже снят концом сценария"
    );
    assert_eq!(control.recorded_ui_overlay().len(), 4);
}

#[test]
fn unavailable_render_answers_the_ui_commands_with_its_reason_and_the_end_reports_it() {
    let gpu = gpu_with(
        Arc::new(UnavailableRenderControl::new("GPU не поднялся")),
        RenderLoopConfig::default(),
    );
    let expected = ControlDelivery::RenderUnavailable {
        reason: "GPU не поднялся".to_owned(),
    };

    assert_eq!(gpu.install_ui_overlay_for_scenario(Box::new(FakeUiSource)), expected);
    let effects = gpu.render_scenario_ended(&RateRecorder::default());
    assert_eq!(effects.adjustments, [("ui_overlay", expected.clone())]);
    // Снятие недоставлено — учёт не сбрасывается, сценарий и дальше «владеет» источником.
    assert_eq!(gpu.remove_ui_overlay_for_scenario(), expected);
}

#[test]
fn inbox_control_delivers_ui_commands_in_order_and_reports_a_stopped_render_thread() {
    use render_loop::mailbox::{InboxEvent, render_mailbox};
    use render_loop::ui_overlay::UiOverlayCommand;

    let (inbox, receiver) = render_mailbox();
    let control = InboxRenderControl::new(inbox);
    assert_eq!(
        control.ui_overlay(UiOverlayCommand::Install(Box::new(FakeUiSource))),
        ControlDelivery::Delivered
    );
    assert_eq!(control.ui_overlay(UiOverlayCommand::Remove), ControlDelivery::Delivered);
    let trace: Vec<String> = receiver
        .take_pending()
        .events
        .into_iter()
        .map(|stamped| match stamped.event {
            InboxEvent::UiOverlay(UiOverlayCommand::Install(source)) => format!("install:{}", source.name()),
            InboxEvent::UiOverlay(UiOverlayCommand::Remove) => "remove".to_owned(),
            other => format!("лишнее: {other:?}"),
        })
        .collect();
    assert_eq!(trace, ["install:fake-ui", "remove"]);

    drop(receiver);
    assert_eq!(
        control.ui_overlay(UiOverlayCommand::Remove),
        ControlDelivery::RenderGone,
        "поток рендера завершён: команда потеряна и это видно"
    );
}

#[test]
fn gpu_work_suppression_reaches_the_control_and_the_end_restores_none_once() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    let scopes_and_ui = GpuWorkSuppression::NONE
        .suppressing(SuppressibleGpuWork::ColorScopes)
        .suppressing(SuppressibleGpuWork::UiOverlayDrawing);

    assert!(gpu.suppress_gpu_work_for_scenario(scopes_and_ui).is_delivered());
    assert_eq!(
        control.recorded(),
        [RenderAdjustment::Viewfinder(ViewfinderCommand::SuppressGpuWork(
            scopes_and_ui
        ))],
        "сценарий послал именно этот набор"
    );
    let before_end = control.recorded().len();

    let effects = gpu.render_scenario_ended(&RateRecorder::default());

    assert_eq!(
        control.recorded()[before_end..],
        [RenderAdjustment::Viewfinder(ViewfinderCommand::SuppressGpuWork(
            GpuWorkSuppression::NONE
        ))],
        "конец сценария возвращает продукт и больше ничего"
    );
    assert_eq!(
        effects.adjustments.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["gpu_work_suppression"]
    );
    // Учёт забран: второй конец ничего не шлёт.
    assert!(gpu.render_scenario_ended(&RateRecorder::default()).is_empty());
    assert_eq!(control.recorded().len(), before_end + 1);
}

#[test]
fn a_scenario_that_never_suppressed_gpu_work_sends_no_restore() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    gpu.set_isp_stages_for_scenario(IspStageSet::ALL.without(IspStage::Tone));
    let before_end = control.recorded().len();

    let effects = gpu.render_scenario_ended(&RateRecorder::default());

    assert_eq!(
        effects.adjustments.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        ["isp_stages"]
    );
    assert!(
        control.recorded()[before_end..].iter().all(|adjustment| !matches!(
            adjustment,
            RenderAdjustment::Viewfinder(ViewfinderCommand::SuppressGpuWork(_))
        )),
        "подавление не трогали — NONE не посылается"
    );
}

#[test]
fn gpu_work_suppression_is_claimed_even_when_not_delivered_and_the_unavailable_reason_is_reported() {
    let gpu = gpu_with(
        Arc::new(UnavailableRenderControl::new("GPU не поднялся")),
        RenderLoopConfig::default(),
    );
    let expected = ControlDelivery::RenderUnavailable {
        reason: "GPU не поднялся".to_owned(),
    };
    let suppression = GpuWorkSuppression::NONE.suppressing(SuppressibleGpuWork::UiOverlayDrawing);

    assert_eq!(gpu.suppress_gpu_work_for_scenario(suppression), expected);
    let effects = gpu.render_scenario_ended(&RateRecorder::default());

    assert_eq!(
        effects.adjustments,
        [("gpu_work_suppression", expected)],
        "недоставленная команда всё равно получила возврат"
    );
}

#[test]
fn suppression_restore_goes_before_the_painter_restore() {
    let control = RecordingRenderControl::new();
    let gpu = gpu_with(control.clone(), RenderLoopConfig::default());
    gpu.set_viewfinder_painter_for_scenario(ViewfinderPainter::DebugMosaic);
    gpu.suppress_gpu_work_for_scenario(GpuWorkSuppression::NONE.suppressing(SuppressibleGpuWork::ColorScopes));
    let before_end = control.recorded().len();

    gpu.render_scenario_ended(&RateRecorder::default());

    assert_eq!(
        control.recorded()[before_end..],
        [
            RenderAdjustment::Viewfinder(ViewfinderCommand::SuppressGpuWork(GpuWorkSuppression::NONE)),
            RenderAdjustment::Viewfinder(ViewfinderCommand::SetPainter(
                RenderLoopConfig::default().viewfinder.painter
            )),
        ]
    );
}
