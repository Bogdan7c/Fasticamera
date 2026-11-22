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
use camera_contract::flash::{FlashDirective, FlashFramePlan, FlashShare, FlashShotId, PreflashMetering};
use continuity_rig::{MAIN, Rig, SIZE_A, StillSetup, dark, gray, passport, test_continuity};
use render_loop::backend::DisplayBackend;
use render_loop::config::{RevealExposureCheck, StillDevelopConfig};
use render_loop::still::{StillBegin, StillCommand, StillCommandOutcome, StillEventSink};
use still_contract::develop::DevelopEvent;
use still_contract::frame_choice::FrameChoice;
use still_contract::plan::{FrameSelection, StillPlan};
use still_rig::{JPEG, Sink, plan, press_during};

const SHOT: FlashShotId = FlashShotId(5);

/// Предлагает обычный кадр видоискателя и возвращает его метку сенсора.
fn frame(rig: &mut Rig, epoch: raw_frame_stream::StreamEpoch) -> SensorTimestamp {
    let sensor_timestamp = SensorTimestamp::from_nanos(rig.next_sensor_ts);
    rig.offer(epoch, dark());
    rig.tick();
    rig.backend.advance_stills();
    sensor_timestamp
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
    let still = rig.backend.still_diagnostics().expect("диагностика снимков");
    assert_eq!(still.flash.frames_found, 1);
    assert_eq!(still.flash.lit_check_passed, 1, "{:?}", still.flash);
    assert_eq!(
        sink.finals(),
        0,
        "итог придёт после цели кодера; до этого ровно один Secured"
    );
}
