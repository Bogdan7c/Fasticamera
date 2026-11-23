//! Снимок со вспышкой в движке снимков (14d) на настоящем кольце и поддельном GPU: кадр ищется по метке запроса,
//! параметры берутся из плана и белой светодиода, свет проверяется по пикселям, итог РОВНО один.

use std::sync::Arc;

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::flash::{
    FlashFramePlan, FlashModeEcho, FlashResult, FlashShare, FlashShotId, FlashState, FlashStillFailure,
};
use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::{PixelRect, PixelSize};
use isp_model::color::chromaticity::Chromaticity;
use isp_model::sensor::SensorDescription;
use raw_frame_stream::RawFrameStreamConfig;
use still_contract::develop::{DevelopEvent, PrimaryTarget, PrimaryTargetOffer};
use still_contract::frame_choice::{FrameChoice, NoFrameReason};
use still_contract::plan::{FrameSelection, StillId, StillPlan};
use still_contract::shot::StillFrameFacts;

use super::feed::StillFeed;
use super::test_support::{EngineRig, MAIN, StreamRig, engine_rig, plan, stamp, stream_passport};
use crate::config::StillDevelopConfig;
use crate::still::{StillCommand, StillCommandOutcome, StillWorkload};
use crate::vulkan_backend::camera::flash_facts::{AmbientLevel, TorchWhite};

const SHOT: FlashShotId = FlashShotId(7);
/// Белая лампы накаливания ≈ 2856 К (как светодиод): заметно отличается от D65 видоискателя.
const LED_WHITE: Chromaticity = Chromaticity { x: 0.4476, y: 0.4074 };
/// Сырой отсчёт, дающий нормированный зелёный ≈ 0,5 (чёрный 64, белый 1023).
const LIT_RAW: u16 = 543;
/// Сырой отсчёт ≈ 0,1.
const DIM_RAW: u16 = 160;
const FIRED: FlashResult = FlashResult {
    mode: Some(FlashModeEcho::Single),
    state: Some(FlashState::Fired),
};

fn flash_plan(id: u64) -> StillPlan {
    StillPlan {
        selection: FrameSelection::FlashFrame { shot: SHOT },
        ..plan(id, 0)
    }
}

/// Что видоискатель сообщил движку о свете (план, белая светодиода, свет сцены).
struct Facts {
    mid_gray_ev_over_frame: f32,
    flash_share: f64,
    torch_white: Option<Chromaticity>,
    /// Средний зелёный сцены на единицу произведения; `None` — уровня сцены нет.
    ambient_green_per_product: Option<f64>,
}

impl Facts {
    fn good() -> Self {
        Self {
            mid_gray_ev_over_frame: 0.25,
            flash_share: 0.6,
            torch_white: Some(LED_WHITE),
            ambient_green_per_product: Some(0.1),
        }
    }
}

/// Движок с кольцом, видоискателем, уже показавшим 3 кадра (есть поток и параметры), и фактами о свете.
fn flash_rig(facts: Option<Facts>) -> EngineRig {
    let mut stream = StreamRig::with_ring(MAIN);
    let frames = stream.offer_many(3);
    let mut rig = engine_rig(StillDevelopConfig::default(), stream);
    rig.engine.ingest_feed(rig.stream.feed_for(&frames, 0));
    if let Some(facts) = facts {
        tell(&mut rig, facts);
    }
    rig.gpu.lock().unwrap().raw_fill = Some(LIT_RAW);
    rig
}

fn tell(rig: &mut EngineRig, facts: Facts) {
    let passport = Arc::clone(&rig.stream.passport);
    let mut feed = StillFeed::default();
    feed.flash_plans.push(FlashFramePlan {
        shot: SHOT,
        ambient_frame: stamp(1),
        // Кадр со вспышкой имеет 1 с·ISO: план выше него на `mid_gray_ev_over_frame` EV.
        mid_gray_product: ExposureProduct::from_seconds_iso(f64::from(facts.mid_gray_ev_over_frame).exp2()).unwrap(),
        flash_share: FlashShare::new(facts.flash_share),
    });
    if let Some(white) = facts.torch_white {
        feed.torch_whites.push(TorchWhite {
            stream: Arc::clone(&passport),
            frame: stamp(1),
            white,
        });
    }
    feed.ambient_levels
        .extend(facts.ambient_green_per_product.map(|green_per_product| AmbientLevel {
            stream: passport,
            frame: stamp(1),
            green_per_product,
        }));
    rig.engine.ingest_feed(feed);
}

fn secured(events: &[DevelopEvent]) -> &StillFrameFacts {
    match events {
        [DevelopEvent::Secured { facts, .. }] => facts,
        other => panic!("ожидался один Secured, пришло {other:?}"),
    }
}

fn only_not_secured(events: &[DevelopEvent]) -> NoFrameReason {
    match events {
        [DevelopEvent::NotSecured { reason, .. }] => *reason,
        other => panic!("ожидался один NotSecured, пришло {other:?}"),
    }
}

/// Нейтраль DNG, которую обязана дать белая `white` (та же цепочка, что у `build_facts`).
fn neutral_of(rig: &EngineRig, white: Chromaticity) -> [f64; 3] {
    let passport = &rig.stream.passport;
    let sensor = SensorDescription::from_stream_passport(passport, &passport.color_calibration).unwrap();
    let values = sensor.color_profile().neutral_for_white(white).unwrap().values();
    let maximum = values.iter().copied().fold(f64::MIN_POSITIVE, f64::max);
    values.map(|channel| channel / maximum)
}

fn assert_neutral(actual: [f64; 3], expected: [f64; 3]) {
    for (a, e) in actual.iter().zip(expected) {
        assert!((a - e).abs() < 1e-9, "нейтраль {actual:?} против {expected:?}");
    }
}

#[test]
fn the_flash_frame_arriving_later_is_secured_with_its_shot_plan_gain_and_the_led_white() {
    let mut rig = flash_rig(Some(Facts::good()));
    let sink = rig.begin(flash_plan(1));
    // Кадра запроса ещё нет; обычные кадры и кадр ЧУЖОГО снимка подставлять нельзя.
    rig.stream.offer_flash_still(99, FIRED);
    rig.stream.offer();
    rig.run(5);
    assert_eq!(sink.len(), 0, "ждём свой кадр");
    assert!(rig.gpu.lock().unwrap().captured.is_empty(), "ничего не копировалось");
    assert_eq!(rig.engine.workload(), StillWorkload::Unfinished);

    let flash_frame = rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    let events = sink.take();
    let facts = secured(&events);
    assert_eq!(facts.sensor_timestamp, flash_frame);
    assert_eq!(
        facts.choice,
        FrameChoice::FlashFrame {
            sensor_timestamp: flash_frame,
            shot: SHOT
        }
    );
    assert_eq!(
        rig.gpu.lock().unwrap().captured,
        vec![flash_frame],
        "скопирован именно кадр запроса"
    );
    // 0,25 EV — ровно 8 шагов по 1/32 EV, в пределе 0,5 EV.
    assert!((facts.digital_gain.0 - 0.25).abs() < 1e-5, "{}", facts.digital_gain.0);
    assert_neutral(facts.neutral.0, neutral_of(&rig, LED_WHITE));
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(
        (
            diagnostics.flash.begins,
            diagnostics.flash.frames_found,
            diagnostics.flash.white_from_torch_frame,
            diagnostics.flash.lit_check_passed
        ),
        (1, 1, 1, 1)
    );
    assert_eq!(diagnostics.picks.exact, 0, "это не выбор «что было на экране»");

    // Дальше снимок идёт тем же путём, что обычный, и кончается одним итогом.
    rig.engine
        .apply_command(StillCommand::Target(PrimaryTargetOffer::Ready {
            id: StillId(1),
            target: PrimaryTarget::Readback,
        }));
    rig.run(10);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::PrimaryReadback { .. }]));
}

#[test]
fn the_flash_frame_is_developed_with_the_look_from_its_plan() {
    // Вид кадра со вспышкой — значения плана на момент нажатия, как у обычного снимка (инвариант 3).
    let mut rig = flash_rig(Some(Facts::good()));
    let look = look_contract::LookSettings {
        warmth: look_contract::LookKnob::new(0.75).unwrap(),
        ..look_contract::LookSettings::NEUTRAL
    };
    let sink = rig.begin(StillPlan {
        look: look.clone(),
        ..flash_plan(1)
    });
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert_eq!(sink.len(), 1, "кадр вспышки захвачен");

    match &rig.engine.queue[0].stage {
        super::entry::Stage::AwaitingTarget { inputs, .. } => assert_eq!(inputs.params.look(), &look),
        _ => panic!("снимок должен ждать цель кодера"),
    }
}

#[test]
fn the_residual_gain_is_rounded_and_limited_like_the_viewfinder_gain() {
    // План на 1 EV выше кадра: предел подтяжки видоискателя 0,5 EV.
    let mut rig = flash_rig(Some(Facts {
        mid_gray_ev_over_frame: 1.0,
        ..Facts::good()
    }));
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert!((secured(&sink.take()).digital_gain.0 - 0.5).abs() < 1e-5);

    // План ниже кадра на 0,3 EV: 0,3 / (1/32) = 9,6 → 10 шагов = 0,3125.
    let mut rig = flash_rig(Some(Facts {
        mid_gray_ev_over_frame: -0.3,
        ..Facts::good()
    }));
    let sink = rig.begin(flash_plan(2));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert!((secured(&sink.take()).digital_gain.0 + 0.3125).abs() < 1e-5);
}

#[test]
fn without_a_torch_estimate_the_viewfinder_white_is_used_and_counted_not_silent() {
    let mut rig = flash_rig(Some(Facts {
        torch_white: None,
        ..Facts::good()
    }));
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    let events = sink.take();
    let viewfinder_white = rig.stream.params_for().white;
    assert_neutral(secured(&events).neutral.0, neutral_of(&rig, viewfinder_white));
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(diagnostics.flash.white_fallback_no_torch_estimate, 1);
    assert_eq!(diagnostics.flash.white_from_torch_frame, 0);
}

#[test]
fn a_flash_frame_that_never_arrives_ends_with_never_arrived_at_the_deadline_and_nothing_else_is_taken() {
    let mut rig = flash_rig(Some(Facts::good()));
    let sink = rig.begin(flash_plan(1));
    // Обычный кадр и кадр другого снимка в кольце есть, своего — нет.
    rig.stream.offer();
    rig.stream.offer_flash_still(SHOT.0 + 1, FIRED);
    let wait_ms = u64::from(StillDevelopConfig::default().flash_frame_wait_ms);
    rig.run(usize::try_from(wait_ms / 16).unwrap() - 3);
    assert_eq!(sink.len(), 0, "срок ещё не вышел");
    rig.run(8);
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::FlashFrameNeverArrived);
    assert_eq!(rig.engine.queue_len(), 0);
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(diagnostics.flash.never_arrived, 1);
    assert_eq!(diagnostics.captures_started, 0, "чужой кадр подставлен не был");
    assert_eq!(diagnostics.outcomes.total(), 1);
}

#[test]
fn the_camera_reporting_a_failed_flash_ends_the_waiting_still_at_once_with_the_reason() {
    let mut rig = flash_rig(Some(Facts::good()));
    let sink = rig.begin(flash_plan(1));
    rig.run(2);
    let outcome = rig.engine.apply_command(StillCommand::FlashFailed {
        id: StillId(1),
        failure: FlashStillFailure::PreflashNotMeasured,
    });
    assert!(matches!(outcome, StillCommandOutcome::Accepted));
    assert_eq!(
        only_not_secured(&sink.take()),
        NoFrameReason::FlashNotCaptured(FlashStillFailure::PreflashNotMeasured)
    );
    assert_eq!(rig.engine.queue_len(), 0);
    // Повтор того же отказа опоздал: снимка уже нет, второго итога не будет.
    rig.engine.apply_command(StillCommand::FlashFailed {
        id: StillId(1),
        failure: FlashStillFailure::ResultTimedOut,
    });
    assert_eq!(sink.len(), 0);
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(
        (
            diagnostics.flash.not_captured_by_camera,
            diagnostics.flash.late_failure_reports
        ),
        (1, 1)
    );
    assert_eq!(diagnostics.outcomes.total(), 1);
}

#[test]
fn a_failure_report_does_not_touch_a_regular_still_or_an_already_captured_flash_still() {
    let mut rig = flash_rig(Some(Facts::good()));
    let regular = rig.begin(plan(1, 25));
    let flash = rig.begin(flash_plan(2));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    // Обычному снимку вспышечный отказ не адресован; захваченный кадр со вспышкой его тоже игнорирует.
    for id in [1, 2] {
        rig.engine.apply_command(StillCommand::FlashFailed {
            id: StillId(id),
            failure: FlashStillFailure::SessionLost,
        });
    }
    assert!(rig.engine.diagnostics().flash.late_failure_reports >= 2);
    assert!(matches!(flash.take().as_slice(), [DevelopEvent::Secured { .. }]));
    assert!(
        regular.take().iter().all(|event| !event.is_final()),
        "обычный снимок не получил итога от чужого отказа"
    );
    assert_eq!(rig.engine.queue_len(), 2);
}

#[test]
fn a_frame_the_hal_called_fired_but_the_pixels_show_dark_is_not_secured() {
    let mut rig = flash_rig(Some(Facts::good()));
    rig.gpu.lock().unwrap().raw_fill = Some(DIM_RAW);
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::FlashFrameNotLit);
    assert_eq!(
        sink.finals_for(StillId(1)),
        0,
        "события уже забраны; итог ровно один — по счётчикам"
    );
    let diagnostics = rig.engine.diagnostics();
    assert_eq!((diagnostics.flash.not_lit, diagnostics.flash.lit_check_passed), (1, 0));
    assert_eq!(diagnostics.outcomes.total(), 1);
    assert_eq!(rig.engine.queue_len(), 0);
}

#[test]
fn when_the_check_cannot_decide_the_frame_is_taken_and_the_skip_is_counted_separately() {
    // Вспышка ожидается слабой: тёмный кадр от освещённого не отличить.
    let mut rig = flash_rig(Some(Facts {
        flash_share: 0.1,
        ..Facts::good()
    }));
    rig.gpu.lock().unwrap().raw_fill = Some(DIM_RAW);
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::Secured { .. }]));
    assert_eq!(rig.engine.diagnostics().flash.lit_check_skipped_low_predicted_share, 1);

    // Уровня света сцены нет вовсе.
    let mut rig = flash_rig(Some(Facts {
        ambient_green_per_product: None,
        ..Facts::good()
    }));
    rig.gpu.lock().unwrap().raw_fill = Some(DIM_RAW);
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::Secured { .. }]));
    assert_eq!(rig.engine.diagnostics().flash.lit_check_skipped_no_ambient_level, 1);

    // Отсчёты RAW не читаются.
    let mut rig = flash_rig(Some(Facts::good()));
    rig.gpu.lock().unwrap().fail_raw_samples = true;
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::Secured { .. }]));
    assert_eq!(rig.engine.diagnostics().flash.lit_check_skipped_samples_unavailable, 1);
}

#[test]
fn a_flash_frame_without_a_plan_or_without_exposure_is_unusable_and_counted() {
    // Плана для снимка нет.
    let mut rig = flash_rig(None);
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::FrameUnusable);
    assert_eq!(rig.engine.diagnostics().flash.no_plan, 1);

    // План есть, у кадра нет выдержки: единичную подтяжку подставлять нельзя.
    let mut rig = flash_rig(Some(Facts::good()));
    let sink = rig.begin(flash_plan(2));
    rig.stream.offer_detailed(
        camera_contract::three_a::RequestOrigin::FlashStill { shot: SHOT },
        true,
        None,
        FIRED,
    );
    rig.run(3);
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::FrameUnusable);
    assert_eq!(rig.engine.diagnostics().flash.no_exposure_product, 1);
    assert_eq!(rig.engine.diagnostics().captures_started, 0);
}

#[test]
fn losing_the_surface_while_waiting_ends_the_still_as_renderer_stopped_and_forgets_the_facts() {
    let mut rig = flash_rig(Some(Facts::good()));
    let sink = rig.begin(flash_plan(1));
    rig.run(2);
    assert_eq!(rig.engine.workload(), StillWorkload::Unfinished);
    rig.engine.on_detach();
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::RendererStopped);
    assert_eq!(rig.engine.workload(), StillWorkload::Idle);
    // Факты о свете забыты вместе с потоком: у следующего снимка плана нет, пока видоискатель не пришлёт новый.
    let sink = rig.begin(flash_plan(2));
    let shown_again = rig.stream.offer();
    rig.engine.ingest_feed(rig.stream.feed_for(&[shown_again], 10));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::FrameUnusable);
    assert_eq!(rig.engine.diagnostics().flash.no_plan, 1);
}

#[test]
fn a_disabled_ring_refuses_the_flash_still_at_begin_like_any_other_still() {
    let stream = StreamRig::new(
        RawFrameStreamConfig {
            snapshot_ring_budget_bytes: 0,
            viewfinder_pacing: raw_frame_stream::ViewfinderPacing::LatestWins,
            ..RawFrameStreamConfig::default()
        },
        MAIN,
    );
    let mut rig = engine_rig(StillDevelopConfig::default(), stream);
    let sink = rig.begin(flash_plan(1));
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::RingDisabled);
    assert_eq!(rig.engine.queue_len(), 0);
}

/// Прогон с занятым GPU до конца срока ожидания кадра со вспышкой (с запасом).
fn run_past_the_flash_wait(rig: &mut EngineRig) {
    let wait_ms = u64::from(StillDevelopConfig::default().flash_frame_wait_ms);
    rig.run(usize::try_from(wait_ms / 16).unwrap() + 4);
}

#[test]
fn a_busy_capture_slot_does_not_keep_the_waiting_still_past_its_deadline() {
    let mut rig = flash_rig(Some(Facts::good()));
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    // Слот копии занят (чужая копия не заканчивается): кадр не изымается, но срок идёт.
    rig.gpu.lock().unwrap().flight.capture = true;
    rig.run(10);
    assert_eq!(sink.len(), 0, "срок ещё не вышел");
    run_past_the_flash_wait(&mut rig);
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::FlashFrameNeverArrived);
    assert_eq!(rig.engine.queue_len(), 0);
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(diagnostics.flash.never_arrived, 1);
    assert_eq!(diagnostics.flash.frames_found, 0, "кадр при занятом слоте не изымался");
}

#[test]
fn a_flash_frame_met_again_while_the_gpu_is_busy_is_counted_once_and_the_deadline_still_ends_the_wait() {
    let mut rig = flash_rig(Some(Facts::good()));
    let sink = rig.begin(flash_plan(1));
    // Объектив сменился: кадр запроса пришёл из потока крупнее, чем тот, под который готов GPU, а GPU занят
    // проявлением другого снимка: кадр возвращается в кольцо и находится заново на каждом шаге.
    let larger = Arc::new(StreamPassport {
        buffer_size: PixelSize::new(128, 96),
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(PixelSize::new(128, 96)),
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: PixelSize::new(128, 96),
        },
        ..stream_passport(MAIN)
    });
    rig.stream.epoch = rig.stream.stream.producer().begin_stream(larger).epoch;
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.gpu.lock().unwrap().flight.develop = true;
    rig.run(10);
    let found_while_waiting = rig.engine.diagnostics().flash.frames_found;
    assert_eq!(
        found_while_waiting, 1,
        "кадр найден один раз, сколько бы ни было повторов"
    );
    run_past_the_flash_wait(&mut rig);
    assert_eq!(only_not_secured(&sink.take()), NoFrameReason::FlashFrameNeverArrived);
    assert_eq!(rig.engine.queue_len(), 0);
    let diagnostics = rig.engine.diagnostics();
    assert_eq!(
        (diagnostics.flash.frames_found, diagnostics.flash.never_arrived),
        (1, 1)
    );
    assert_eq!(diagnostics.captures_started, 0);
}

#[test]
fn the_lit_check_uses_the_ambient_level_of_the_planned_frame_not_the_latest_one() {
    let mut rig = flash_rig(Some(Facts::good()));
    // Позже кадра плана (stamp(1), зелёный 0,1) измерен ещё один «обычный» кадр — частично освещённый, очень
    // светлый. По нему кадр со вспышкой (зелёный ≈ 0,5) выглядел бы неосвещённым.
    let mut later = StillFeed::default();
    later.ambient_levels.push(AmbientLevel {
        stream: Arc::clone(&rig.stream.passport),
        frame: stamp(5),
        green_per_product: 0.9,
    });
    rig.engine.ingest_feed(later);
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::Secured { .. }]));
    let diagnostics = rig.engine.diagnostics();
    assert_eq!((diagnostics.flash.lit_check_passed, diagnostics.flash.not_lit), (1, 0));
}

#[test]
fn without_the_ambient_level_of_the_planned_frame_the_check_is_undecidable_and_counted() {
    // Уровень есть только у другого кадра (не у кадра плана): подставлять его нельзя.
    let mut rig = flash_rig(Some(Facts {
        ambient_green_per_product: None,
        ..Facts::good()
    }));
    let mut other_frame = StillFeed::default();
    other_frame.ambient_levels.push(AmbientLevel {
        stream: Arc::clone(&rig.stream.passport),
        frame: stamp(5),
        green_per_product: 0.1,
    });
    rig.engine.ingest_feed(other_frame);
    rig.gpu.lock().unwrap().raw_fill = Some(DIM_RAW);
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::Secured { .. }]));
    assert_eq!(rig.engine.diagnostics().flash.lit_check_skipped_no_ambient_level, 1);
}

#[test]
fn ambient_levels_are_remembered_boundedly_and_the_oldest_planned_frame_is_forgotten_first() {
    let mut rig = flash_rig(Some(Facts::good()));
    // 32 более поздних кадров вытесняют кадр плана (stamp(1)) из ограниченной памяти.
    let mut later = StillFeed::default();
    for index in 0..32 {
        later.ambient_levels.push(AmbientLevel {
            stream: Arc::clone(&rig.stream.passport),
            frame: stamp(10 + index),
            green_per_product: 0.1,
        });
    }
    rig.engine.ingest_feed(later);
    rig.gpu.lock().unwrap().raw_fill = Some(DIM_RAW);
    let sink = rig.begin(flash_plan(1));
    rig.stream.offer_flash_still(SHOT.0, FIRED);
    rig.run(3);
    assert!(matches!(sink.take().as_slice(), [DevelopEvent::Secured { .. }]));
    assert_eq!(rig.engine.diagnostics().flash.lit_check_skipped_no_ambient_level, 1);
}
