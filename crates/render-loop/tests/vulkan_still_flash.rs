//! Снимок со вспышкой на настоящем бэкенде (RADV, headless, полная validation), сессия 14d: кадр разового запроса
//! со вспышкой доходит до `Secured` через кольцо снимков и настоящий GPU, а видоискатель его не показывает.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod still_rig;
mod support;
mod vulkan_support;

use std::sync::Arc;

use camera_contract::auto_exposure::{ExposureDirective, ExposureProduct};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::fixtures::camera_id;
use camera_contract::flash::{FlashDirective, FlashFramePlan, FlashShare, FlashShotId, PreflashMetering};
use camera_contract::frame_passport::StreamPassport;
use camera_contract::lens_shading::{LensShadingCalibration, LensShadingCalibrationState, LensShadingGainGrid};
use continuity_rig::{CellValues, MAIN, Rig, SIZE_A, StillSetup, dark, gray, passport, test_continuity};
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use isp_model::sensor::SensorDescription;
use render_loop::backend::DisplayBackend;
use render_loop::config::{RevealExposureCheck, StillDevelopConfig};
use render_loop::still::{StillBegin, StillCommand, StillCommandOutcome, StillEventSink};
use still_contract::develop::DevelopEvent;
use still_contract::frame_choice::FrameChoice;
use still_contract::plan::{FrameSelection, StillPlan};
use still_rig::{JPEG, Sink, plan, press_during};

const SHOT: FlashShotId = FlashShotId(5);
/// Постоянная белая светодиода плана: ≈ 2856 К (видоискатель в этом стенде без указаний держит D65).
const LED_KELVIN: f64 = 2_856.0;

fn led_setting(kelvin: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::NEUTRAL,
    }
}

/// Предлагает обычный кадр видоискателя и возвращает его метку сенсора.
fn frame(rig: &mut Rig, epoch: raw_frame_stream::StreamEpoch) -> SensorTimestamp {
    let sensor_timestamp = SensorTimestamp::from_nanos(rig.next_sensor_ts);
    rig.offer(epoch, dark());
    rig.tick();
    rig.backend.advance_stills();
    sensor_timestamp
}

/// Нейтраль DNG (нормированная по наибольшему каналу), которую профиль камеры потока даёт для белой `kelvin`, Duv 0.
fn neutral_of(stream: &StreamPassport, kelvin: f64) -> [f64; 3] {
    let white = chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(kelvin),
        duv: Duv(0.0),
    })
    .expect("белая в таблице");
    let sensor = SensorDescription::from_stream_passport(stream, &stream.color_calibration).expect("описание сенсора");
    let values = sensor
        .color_profile()
        .neutral_for_white(white)
        .expect("нейтраль")
        .values();
    let maximum = values.iter().copied().fold(f64::MIN_POSITIVE, f64::max);
    values.map(|channel| channel / maximum)
}

fn neutral_distance(actual: [f64; 3], expected: [f64; 3]) -> f64 {
    actual
        .iter()
        .zip(expected)
        .map(|(a, e)| (a - e).abs())
        .fold(0.0, f64::max)
}

fn assert_neutral_close(actual: [f64; 3], expected: [f64; 3]) {
    assert!(
        neutral_distance(actual, expected) < 1e-9,
        "нейтраль {actual:?} против {expected:?}"
    );
}

#[test]
fn a_flash_frame_reaches_secured_through_the_ring_and_the_real_gpu_and_the_viewfinder_never_shows_it() {
    let Some(mut rig) = Rig::with_stills(
        test_continuity(RevealExposureCheck::Ignore),
        true,
        Some(StillSetup {
            config: StillDevelopConfig::default(),
            ring_frames: 12,
        }),
    ) else {
        return;
    };
    let stream = Arc::new(passport(MAIN, SIZE_A));
    let epoch = rig.begin(&stream);
    // Первый кадр потока видоискатель измеряет всегда (холодный старт замера): его уровень света сцены
    // и называет план как кадр, по которому посчитан свет без вспышки.
    let ambient_frame = frame(&mut rig, epoch);
    for _ in 1..10 {
        frame(&mut rig, epoch);
    }
    // Движок камеры выбрал план кадра со вспышкой: кадр 1 с·ISO, план — тот же уровень.
    rig.exposure_directives.publish(ExposureDirective {
        flash: FlashDirective {
            preflash: PreflashMetering::Inactive,
            planned: Some(FlashFramePlan {
                shot: SHOT,
                // Кадр без светодиода, свет которого движок камеры взял за свет сцены: видоискатель его измерил.
                ambient_frame,
                mid_gray_product: ExposureProduct::from_seconds_iso(1.0).expect("произведение"),
                flash_share: FlashShare::new(0.6),
                led_white: led_setting(LED_KELVIN),
            }),
        },
        ..ExposureDirective::DEFAULT
    });
    for _ in 0..6 {
        frame(&mut rig, epoch);
    }

    let sink = Arc::new(Sink::default());
    let events: Arc<dyn StillEventSink> = Arc::clone(&sink) as Arc<dyn StillEventSink>;
    let flash_plan = StillPlan {
        selection: FrameSelection::FlashFrame { shot: SHOT },
        ..plan(1, press_during(3), JPEG)
    };
    let outcome = rig.backend.apply_still_command(StillCommand::Begin(StillBegin {
        plan: flash_plan,
        events,
    }));
    assert!(matches!(outcome, StillCommandOutcome::Accepted), "{outcome:?}");

    let before = rig.backend.diagnostics().viewfinder.expect("диагностика видоискателя");
    rig.offer_flash_still(epoch, gray(), SHOT.0);
    for _ in 0..30 {
        if sink.has_secured() {
            break;
        }
        rig.tick();
        rig.backend.advance_stills();
    }
    assert!(sink.has_secured(), "кадр вспышки дошёл до Secured");
    let facts = match sink.events().first() {
        Some(DevelopEvent::Secured { facts, .. }) => facts.clone(),
        other => panic!("первым событием должен быть Secured: {other:?}"),
    };
    assert!(
        matches!(facts.choice, FrameChoice::FlashFrame { shot, .. } if shot == SHOT),
        "{:?}",
        facts.choice
    );
    let after = rig.backend.diagnostics().viewfinder.expect("диагностика видоискателя");
    assert_eq!(
        after.counters.flash_frames_skipped,
        before.counters.flash_frames_skipped + 1
    );
    assert_eq!(
        after.counters.frames_processed, before.counters.frames_processed,
        "видоискатель кадр вспышки не обрабатывал и не показывал"
    );
    // Белая кадра — постоянный цвет светодиода из плана, а не белая видоискателя (в этом стенде — временная D65):
    // нейтраль DNG равна той, что даёт профиль камеры для белой плана, и отличается от нейтрали D65.
    assert_neutral_close(facts.neutral.0, neutral_of(&stream, LED_KELVIN));
    assert!(
        neutral_distance(facts.neutral.0, neutral_of(&stream, 6_504.0)) > 0.05,
        "белая кадра со вспышкой не D65 видоискателя: {:?}",
        facts.neutral
    );
    let still = rig.backend.still_diagnostics().expect("диагностика снимков");
    assert_eq!((still.flash.white_from_plan, still.flash.white_unconvertible), (1, 0));
    assert_eq!(still.flash.frames_found, 1);
    assert_eq!(still.flash.lit_check_passed, 1, "{:?}", still.flash);
    assert_eq!(
        sink.finals(),
        0,
        "итог придёт после цели кодера; до этого ровно один Secured"
    );
}

/// Паспорт потока с калибровкой виньетирования: усиление ×2 во всех каналах по всему кадру (равномерное, чтобы веса
/// замера AE не влияли на сравнение шкал).
fn vignetting_passport() -> StreamPassport {
    let mut stream = passport(MAIN, SIZE_A);
    let grid = LensShadingGainGrid::new(2, 2, vec![[2.0; 4]; 4]).expect("сетка");
    stream.lens_shading = LensShadingCalibrationState::Calibrated(Arc::new(LensShadingCalibration {
        camera: camera_id(MAIN),
        active_array: SIZE_A,
        grid,
    }));
    stream
}

#[test]
fn a_really_lit_frame_passes_the_check_on_the_real_gpu_when_the_vignetting_correction_is_on() {
    // Дефект 14f на настоящей статистике GPU: уровень сцены статистика считает ПОСЛЕ коррекции (×2), а RAW кадра
    // со вспышкой проверка читала ДО неё. Кадр вдвое ярче сцены по сырым отсчётам (доля вспышки 0,5 при предсказанных
    // 0,6) по старой шкале давал долю 0 и браковался.
    let Some(mut rig) = Rig::with_stills(
        test_continuity(RevealExposureCheck::Ignore),
        true,
        Some(StillSetup {
            config: StillDevelopConfig::default(),
            ring_frames: 12,
        }),
    ) else {
        return;
    };
    let stream = Arc::new(vignetting_passport());
    let epoch = rig.begin(&stream);
    let ambient_frame = frame(&mut rig, epoch);
    for _ in 1..10 {
        frame(&mut rig, epoch);
    }
    rig.exposure_directives.publish(ExposureDirective {
        flash: FlashDirective {
            preflash: PreflashMetering::Inactive,
            planned: Some(FlashFramePlan {
                shot: SHOT,
                ambient_frame,
                mid_gray_product: ExposureProduct::from_seconds_iso(1.0).expect("произведение"),
                flash_share: FlashShare::new(0.6),
                led_white: led_setting(LED_KELVIN),
            }),
        },
        ..ExposureDirective::DEFAULT
    });
    for _ in 0..6 {
        frame(&mut rig, epoch);
    }
    let sink = Arc::new(Sink::default());
    let events: Arc<dyn StillEventSink> = Arc::clone(&sink) as Arc<dyn StillEventSink>;
    let flash_plan = StillPlan {
        selection: FrameSelection::FlashFrame { shot: SHOT },
        ..plan(1, press_during(3), JPEG)
    };
    let outcome = rig.backend.apply_still_command(StillCommand::Begin(StillBegin {
        plan: flash_plan,
        events,
    }));
    assert!(matches!(outcome, StillCommandOutcome::Accepted), "{outcome:?}");
    // Сырой зелёный сцены `dark()` = 70 (над чёрным 64 — 6); кадр вспышки — 76 (над чёрным 12): вдвое ярче.
    rig.offer_flash_still(
        epoch,
        CellValues {
            red: 76,
            green: 76,
            blue: 76,
        },
        SHOT.0,
    );
    for _ in 0..30 {
        if sink.has_secured() {
            break;
        }
        rig.tick();
        rig.backend.advance_stills();
    }
    let still = rig.backend.still_diagnostics().expect("диагностика снимков");
    assert!(
        sink.has_secured(),
        "кадр принят, а не забракован как не освещённый: {:?}",
        still.flash
    );
    assert_eq!(
        (still.flash.lit_check_passed, still.flash.not_lit),
        (1, 0),
        "{:?}",
        still.flash
    );
}
