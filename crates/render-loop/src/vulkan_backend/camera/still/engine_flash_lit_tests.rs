//! Проверка «кадр со вспышкой освещён» под коррекцией виньетирования (дефект 14f): кадр мерится в шкале статистики
//! видоискателя, то есть после коррекции, а не по сырым отсчётам.

use std::sync::Arc;

use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::StreamPassport;
use camera_contract::lens_shading::{LensShadingCalibration, LensShadingCalibrationState, LensShadingGainGrid};
use isp_model::lens_shading::LensShadingControl;
use still_contract::develop::DevelopEvent;
use still_contract::frame_choice::NoFrameReason;

use super::engine_flash_tests::{D65, DIM_RAW, FIRED, Facts, SHOT, flash_plan, flash_rig_on_stream, only_not_secured};
use super::test_support::{FRAME_SIZE, MAIN, StreamRig, stream_passport};

/// Паспорт потока с калибровкой виньетирования: усиление ×2 во всех каналах и по всему кадру.
fn vignetting_passport() -> StreamPassport {
    let mut passport = stream_passport(MAIN);
    let grid = LensShadingGainGrid::new(2, 2, vec![[2.0; 4]; 4]).unwrap();
    passport.lens_shading = LensShadingCalibrationState::Calibrated(Arc::new(LensShadingCalibration {
        camera: camera_id(MAIN),
        active_array: FRAME_SIZE,
        grid,
    }));
    passport
}

/// Прогон снимка со вспышкой на потоке с коррекцией виньетирования. `raw_fill` — сырой отсчёт кадра (нормированный
/// зелёный ≈ 0,1, после усиления ×2 ≈ 0,2); `ambient` — уровень сцены, который статистика назвала для кадра плана.
fn run_flash_under_vignetting(ambient: f64, control: LensShadingControl) -> Vec<DevelopEvent> {
    let stream = StreamRig::with_ring_and_passport(vignetting_passport());
    let mut rig = flash_rig_on_stream(
        Some(Facts {
            ambient_green_per_product: Some(ambient),
            ..Facts::good()
        }),
        D65,
        stream,
    );
    rig.engine = rig.engine.with_lens_shading(control);
    // Параметры ISP кадра берутся из его потока с текущей коррекцией: подтолкнём их тем же путём, что видоискатель.
    rig.gpu.lock().unwrap().raw_fill = Some(DIM_RAW);
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    sink.take()
}

#[test]
fn a_lit_frame_is_accepted_under_vignetting_because_it_is_measured_in_the_statistics_scale() {
    // Сцена 0,1 на единицу произведения (кадр 1 с·ISO); кадр после коррекции ≈ 0,2: вспышка дала ≈ 50 % света при
    // предсказанных 60 % (порог 0,5 × 0,6). До починки кадр читался до коррекции (≈ 0,1) и бракуется как `NotLit`.
    let events = run_flash_under_vignetting(0.1, LensShadingControl::default());
    assert!(
        matches!(events.as_slice(), [DevelopEvent::Secured { .. }]),
        "{events:?}"
    );
}

#[test]
fn a_frame_without_the_flash_is_still_rejected_under_vignetting() {
    // Вспышка не горела: кадр после коррекции ровно так же ярок, как сцена (0,2) — доля 0.
    let events = run_flash_under_vignetting(0.2002, LensShadingControl::default());
    assert_eq!(only_not_secured(&events), NoFrameReason::FlashFrameNotLit);
}

#[test]
fn with_the_correction_disabled_the_frame_is_measured_in_the_old_scale() {
    // Коррекция выключена управлением: шкала прежняя (≈ 0,1), и тот же уровень сцены 0,1 даёт долю 0 — `NotLit`.
    let events = run_flash_under_vignetting(0.1, LensShadingControl::Disabled);
    assert_eq!(only_not_secured(&events), NoFrameReason::FlashFrameNotLit);
}
