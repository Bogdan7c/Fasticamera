//! Сценарий `viewfinder-latency` через настоящий маршрутизатор на подставном рендере: порядок
//! профилей ABBA, возврат настроек, отчёт окон и поломки замера. Помощники — родительского модуля.

use raw_frame_stream::ViewfinderPacing;
use render_loop::config::{AdpfMode, DepthReductionMode, LateLatchMode, PresentTimelinePolicy};

use super::*;
use crate::viewfinder_control::PresentTargetSetting;

/// Пара «линия кадра, ADPF» из журнала регулировок: по две регулировки на окно, затем две на возврат.
fn present_and_adpf(harness: &Harness) -> (Vec<PresentTargetSetting>, Vec<AdpfMode>) {
    let recorded = harness.control.recorded();
    let present = recorded
        .iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::PresentTarget(setting) => Some(*setting),
            _ => None,
        })
        .collect();
    let adpf = recorded
        .iter()
        .filter_map(|adjustment| match adjustment {
            RenderAdjustment::Adpf(mode) => Some(*mode),
            _ => None,
        })
        .collect();
    (present, adpf)
}

#[test]
fn latency_scenario_alternates_profiles_abba_and_restores_the_configured_settings() {
    let (harness, result) = run_viewfinder(
        short_config(|_, _| {}),
        "viewfinder-latency",
        MirrorFaults::default(),
        None,
    );

    assert_eq!(result["status"], "passed", "{result}");
    let report = &result["report"];
    assert_eq!(report["windows"], 6);
    assert_eq!(report["profiles"][0]["name"], "baseline");
    assert_eq!(report["profiles"][1]["name"], "tuned");

    let baseline = PresentTargetSetting {
        policy: PresentTimelinePolicy::Preferred,
        late_latch: LateLatchMode::Disabled,
        depth_reduction: DepthReductionMode::Disabled,
    };
    let tuned = PresentTargetSetting {
        policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1_500 },
        late_latch: LateLatchMode::BeforeCameraPoll,
        depth_reduction: DepthReductionMode::SkipOneVsync,
    };
    let (present, adpf) = present_and_adpf(&harness);
    // ABBA по шести окнам, затем возврат к настройкам из конфига рендера: продуктовые значения по
    // умолчанию (NearestFeasible, поздний захват, сход на ближнюю линию; ADPF выключен).
    assert_eq!(present, [baseline, tuned, tuned, baseline, baseline, tuned, tuned]);
    use AdpfMode::{Disabled as Off, Enabled as On};
    assert_eq!(adpf, [Off, On, On, Off, Off, On, Off]);
    // Порядок внутри окна: сначала линия кадра, потом ADPF; возврат — в самом конце журнала.
    let recorded = harness.control.recorded();
    assert!(
        matches!(recorded[0], RenderAdjustment::PresentTarget(_)),
        "{recorded:?}"
    );
    assert!(matches!(recorded[1], RenderAdjustment::Adpf(Off)), "{recorded:?}");
    assert!(
        matches!(recorded[recorded.len() - 1], RenderAdjustment::Adpf(Off)),
        "{recorded:?}"
    );

    // Профили реально отличались в статусе рендера: отчёты ADPF копились только в окнах tuned.
    let comparison = &report["comparison"];
    assert_eq!(comparison["baseline"]["windows"], 3);
    assert_eq!(comparison["tuned"]["windows"], 3);
    assert_eq!(comparison["baseline"]["adpf_reports_sent"], 0);
    assert!(
        comparison["tuned"]["adpf_reports_sent"].as_u64().unwrap_or(0) > 0,
        "{comparison}"
    );
    assert!(
        comparison["baseline"]["processed_fps"].as_f64().unwrap_or(0.0) > 30.0,
        "{comparison}"
    );
    assert_eq!(comparison["tuned"]["frame_gpu_p50_ms"], 9.0);
    // Одинаковая задержка у обоих профилей — честное «разницы нет», а не провал.
    assert_eq!(comparison["verdict"]["verdict"], "no_clear_difference");
    assert_eq!(stages_named(&harness, "window").len(), 6);
    assert_eq!(stages_named(&harness, "window")[1]["profile"], "tuned");
    // Подача кадров в потоке RAW переключалась вместе с профилем (ABBA) и вернулась к штатной.
    let pacing_per_window: Vec<_> = stages_named(&harness, "window")
        .iter()
        .map(|stage| stage["viewfinder_pacing"].as_str().unwrap_or("?").to_owned())
        .collect();
    assert_eq!(
        pacing_per_window,
        [
            "latest_wins",
            "jitter_buffer",
            "jitter_buffer",
            "latest_wins",
            "latest_wins",
            "jitter_buffer"
        ]
    );
    assert_eq!(
        harness.rig().services.raw_stream().viewfinder_pacing(),
        ViewfinderPacing::JitterBuffer,
        "конец сценария вернул штатную подачу"
    );
    // Сводки профилей несут показатели подачи: подставной поток выдаёт каждый опубликованный кадр.
    assert_eq!(comparison["baseline"]["shown_once_ratio"], 1.0, "{comparison}");
    assert_eq!(comparison["tuned"]["shown_once_ratio"], 1.0, "{comparison}");
    assert_fits_logcat(&harness, &result);
}

#[test]
fn latency_scenario_fails_when_render_ignores_the_adpf_command() {
    let faults = MirrorFaults {
        ignores_adpf_command: true,
        ..MirrorFaults::default()
    };
    let (_harness, result) = run_viewfinder(short_config(|_, _| {}), "viewfinder-latency", faults, None);

    assert_eq!(result["status"], "failed", "{result}");
    let error = result["error"].as_str().unwrap_or_default();
    assert!(
        error.contains("(tuned)") && error.contains("не с настройками профиля"),
        "{error}"
    );
}

#[test]
fn latency_scenario_uses_the_configured_profiles() {
    let config = short_config(|viewfinder, _| {
        viewfinder.latency.profiles[0].name = "a".to_owned();
        viewfinder.latency.profiles[1].name = "b".to_owned();
        // Проверяемый профиль без ADPF: меняется только линия кадра.
        viewfinder.latency.profiles[1].adpf = AdpfMode::Disabled;
    });
    let (harness, result) = run_viewfinder(config, "viewfinder-latency", MirrorFaults::default(), None);

    assert_eq!(result["status"], "passed", "{result}");
    assert_eq!(result["report"]["profiles"][1]["name"], "b");
    let (_, adpf) = present_and_adpf(&harness);
    assert!(adpf.iter().all(|mode| *mode == AdpfMode::Disabled), "{adpf:?}");
    assert_eq!(result["report"]["comparison"]["tuned"]["adpf_reports_sent"], 0);
}

#[test]
fn latency_scenario_with_invalid_profiles_fails_at_start_without_touching_render() {
    let config = short_config(|viewfinder, _| {
        viewfinder.latency.profiles.truncate(1);
    });
    let (harness, result) = run_viewfinder(config, "viewfinder-latency", MirrorFaults::default(), None);

    assert_eq!(result["status"], "failed", "{result}");
    assert!(
        result["error"]
            .as_str()
            .unwrap_or_default()
            .contains("нужно ровно 2 профиля"),
        "{result}"
    );
    assert!(harness.control.recorded().is_empty());
}
