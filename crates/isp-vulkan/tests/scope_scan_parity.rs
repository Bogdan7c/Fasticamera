//! Положение телефона на GPU: чтение картинки глазом (`ScopeScan`) даёт те же счётчики, что CPU-эталон, БИТ В
//! БИТ, на waveform и parade во всех четырёх положениях, при зеркале и без, и не пересоздаёт ресурсы.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod scope_support;

use gpu_vulkan::live_objects::LiveObjectKind;
use gpu_vulkan::source_window::HorizontalMirror;
use gpu_vulkan::storage_image::StorageFormatPolicy;
use isp_vulkan::scopes::{ColorScopes, ColorScopesConfig, ScopeFrame};
use scope_contract::ScopeScan;
use scope_model::{SampleStride, ScopeModelSettings, ScopeRule};
use scope_support::{
    all_scenes, assert_no_validation_errors, cpu_counters, gpu_context_or_skip, mirror_name, picture_size,
    rule_cases_for_scan, run_scope_frame, small_settings, upload_scene,
};

/// Waveform и parade (первые два случая `rule_cases`) — приборы, которым важно положение.
const TRACE_RULES: std::ops::Range<usize> = 0..2;

#[test]
fn every_scan_matches_cpu_bit_exact_and_creates_no_gpu_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let base_settings = small_settings();
    let mut scopes = ColorScopes::new(
        &context,
        &ColorScopesConfig {
            frame_slots: 2,
            counter_sizing: base_settings.clone(),
        },
    )
    .unwrap();
    let pipelines_at_creation = scopes.accumulator_pipeline_count();
    let live_pipelines = context.live_objects().count(LiveObjectKind::Pipeline);
    let mut compared = 0_usize;
    let mut differs_from_upright = 0_usize;
    for scene in all_scenes() {
        let input = upload_scene(&context, &scene, StorageFormatPolicy::PreferTenBitUnorm, (7, 5));
        for mirror in [HorizontalMirror::Off, HorizontalMirror::On] {
            let shown = input.shown_codes(mirror);
            for stride in [1_u32, 2] {
                let settings = ScopeModelSettings {
                    sample_stride: SampleStride::try_new(stride).unwrap(),
                    ..base_settings.clone()
                };
                let mut upright_total = Vec::new();
                for scan in ScopeScan::ALL {
                    let rules = rule_cases_for_scan(&settings, scan);
                    for rule_index in TRACE_RULES {
                        let (rule_name, rule) = &rules[rule_index];
                        let frame = ScopeFrame {
                            input: input.input(mirror),
                            rule,
                            settings: &settings,
                            picture_size: picture_size(160, 96),
                            frame_slot: compared % 2,
                        };
                        run_scope_frame(&context, &mut scopes, &frame).unwrap();
                        let gpu = scopes.read_back_counters_blocking().unwrap();
                        let cpu = cpu_counters(&shown, &settings, rule as &dyn ScopeRule);
                        assert_eq!(
                            gpu,
                            cpu,
                            "{} / {rule_name} / {scan:?} / {} / шаг {stride}: GPU и CPU разошлись",
                            scene.name,
                            mirror_name(mirror)
                        );
                        if scan == ScopeScan::Upright {
                            upright_total.push(cpu);
                        } else {
                            differs_from_upright += usize::from(cpu != upright_total[rule_index]);
                        }
                        compared += 1;
                    }
                }
            }
        }
    }
    assert_eq!(compared, 6 * 2 * 2 * 4 * 2, "размер матрицы");
    // Чтение действительно меняет картину (иначе «равны» могло бы значить «положение не учтено»): из 144
    // повёрнутых сравнений подавляющее большинство отличается от портретного (одинаковые — симметричные сцены).
    let rotated_comparisons = compared / 4 * 3;
    assert!(
        differs_from_upright * 2 >= rotated_comparisons,
        "повёрнутые чтения отличаются от портретного лишь в {differs_from_upright} случаях из {rotated_comparisons}"
    );
    // Смена положения — только параметры: ни одного нового пайплайна, ни по владельцу, ни по устройству.
    assert_eq!(scopes.accumulator_pipeline_count(), pipelines_at_creation);
    assert_eq!(context.live_objects().count(LiveObjectKind::Pipeline), live_pipelines);
    assert_eq!(
        scopes.pass_counters().picture_recreations,
        0,
        "картинка не пересоздавалась"
    );
    assert_no_validation_errors(&context);
}
