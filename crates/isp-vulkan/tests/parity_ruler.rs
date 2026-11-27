//! Линейка паритета из библиотеки (`isp_vulkan::saturation::parity`) на настоящем GPU ПК (RADV): она ИЗМЕРЯЕТ (числа не
//! нулевые и осмысленные), её вердикт держит допуск, и она отказывает на кадрах, которые не может измерить честно.
//!
//! Остальные тесты паритета (`viewfinder_*parity*.rs`) гоняют ту же линейку через тонкую обвязку `viewfinder_support`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod viewfinder_support;

use gpu_vulkan::storage_image::StorageImageFormat;
use isp_model::image::DisplayGamut;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_vulkan::saturation::parity::{ErrorAllowance, ParityError, ParityRuler, SyntheticRawFrame};
use look_support::all_bands;
use viewfinder_support::{ParamsSpec, assert_no_validation_errors, gpu_context_or_skip, hue_sweep};

fn agx_spec() -> ParamsSpec {
    ParamsSpec {
        gamut: DisplayGamut::DisplayP3,
        tone_base: ToneBase::Agx(AgxSettings::default()),
        ..ParamsSpec::default()
    }
}

#[test]
fn ruler_measures_a_real_difference_between_gpu_and_reference_and_stays_inside_the_allowance() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = agx_spec();
    let params = spec.build_with_look(all_bands(1.0, 1.0, -1.0));
    let raw = hue_sweep(&spec);
    let mut ruler = ParityRuler::new(&context).unwrap();
    let measure = ruler.measure(&raw, &params, spec.display()).unwrap();

    // Измерение происходит: GPU не бит в бит с эталоном (округление хранения), но в пределах допуска.
    assert!(measure.half.max_abs > 0.0, "{measure:?}");
    assert!(measure.ten_bit.max_abs > 0.0, "{measure:?}");
    assert_eq!(measure.violations(ErrorAllowance::Standard), Vec::<String>::new());
    assert!(measure.worst_math_excess() <= ErrorAllowance::Standard.math_excess());
    assert_eq!(
        measure.ten_bit_format,
        StorageImageFormat::A2B10G10R10Unorm,
        "RADV пишет rgb10a2"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn ruler_is_reusable_across_frames_and_params_with_the_same_result_as_a_fresh_ruler() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = agx_spec();
    let raw = hue_sweep(&spec);
    let neutral = spec.build();
    let looked = spec.build_with_look(all_bands(0.7, -0.6, 0.5));
    let mut reused = ParityRuler::new(&context).unwrap();
    // Порядок «нейтраль, вид, нейтраль»: состояние ISP от предыдущего кадра не утекает в следующий.
    let first = reused.measure(&raw, &neutral, spec.display()).unwrap();
    let with_look = reused.measure(&raw, &looked, spec.display()).unwrap();
    let again = reused.measure(&raw, &neutral, spec.display()).unwrap();
    assert_eq!(first, again, "повтор нейтрали на той же линейке даёт те же числа");
    let fresh = ParityRuler::new(&context)
        .unwrap()
        .measure(&raw, &looked, spec.display())
        .unwrap();
    assert_eq!(with_look, fresh, "переиспользованная линейка = свежая");
    assert_no_validation_errors(&context);
}

#[test]
fn ruler_refuses_frames_it_cannot_measure_honestly() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let mut ruler = ParityRuler::new(&context).unwrap();

    // Отсчёт не помещается в RAW10: линейка не обрезает молча.
    let too_bright = SyntheticRawFrame::from_fn(64, 48, spec.pattern, |_, _, _| 2000);
    let error = ruler.measure(&too_bright, &params, spec.display()).unwrap_err();
    assert!(matches!(error, ParityError::UnsupportedFrame { .. }), "{error}");

    // Ширина не кратна 4: RAW10 так не упаковать.
    let ragged = SyntheticRawFrame::from_fn(62, 48, spec.pattern, |_, _, _| 100);
    let error = ruler.measure(&ragged, &params, spec.display()).unwrap_err();
    assert!(matches!(error, ParityError::UnsupportedFrame { .. }), "{error}");

    // Больше предела линейки (суммы AWB рассчитаны на 256x192).
    let huge = SyntheticRawFrame::from_fn(512, 384, spec.pattern, |_, _, _| 100);
    let error = ruler.measure(&huge, &params, spec.display()).unwrap_err();
    assert!(matches!(error, ParityError::UnsupportedFrame { .. }), "{error}");

    // После отказов линейка исправна: нормальный кадр измеряется.
    let raw = hue_sweep(&spec);
    let measure = ruler.measure(&raw, &params, spec.display()).unwrap();
    assert!(measure.worst_math_excess() <= ErrorAllowance::Standard.math_excess());
    assert_no_validation_errors(&context);
}
