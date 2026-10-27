use std::sync::{Arc, Mutex};

use isp_vulkan::viewfinder::{IspStage, IspStageSet};
use render_loop::config::{AdpfMode, DepthReductionMode, LateLatchMode, PresentTimelinePolicy, RenderLoopConfig};
use render_loop::viewfinder::{HeadroomBench, ViewfinderCommand, ViewfinderPainter};
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};

use super::*;
use crate::gpu_services::UnavailableDiagnostics;
use crate::gpu_test_support::{FakeRenderStatus, RecordingRenderControl, presenting_status};

/// Оболочка, которая только запоминает просьбы о частоте экрана.
#[derive(Default)]
struct RateRecorder {
    rates: Mutex<Vec<f32>>,
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
