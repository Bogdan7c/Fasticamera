//! Снимок со вспышкой целиком на настоящем движке: предвспышка, замеры, пауза, разовый запрос,
//! результат; сроки; выходы при потере потока, смене объектива и уходе в фон.

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::auto_exposure::MeasurementQuality;
use camera_contract::flash::{
    FlashControl, FlashModeEcho, FlashResult, FlashShotId, FlashState, FlashStillFailure, PreflashLight,
    PreflashMetering, PreflashSamples,
};
use camera_contract::ids::LensRole;

use super::flash_support::*;
use super::three_a_support::*;
use super::*;
use crate::backend::{BackendFailure, RequestFailure};
use crate::flash_status::{FlashPhase, FlashStillReport};
use crate::request_controls::ExposurePlan;

const FIRED_SINGLE: FlashResult = FlashResult {
    mode: Some(FlashModeEcho::Single),
    state: Some(FlashState::Fired),
};

fn failed(reports: &[FlashStillReport]) -> Vec<(FlashShotId, FlashStillFailure)> {
    reports
        .iter()
        .filter_map(|report| match report {
            FlashStillReport::Failed { shot, failure } => Some((*shot, *failure)),
            FlashStillReport::Fired { .. } => None,
        })
        .collect()
}

#[test]
fn flash_still_runs_preflash_gap_single_and_reports_the_planned_frame() {
    let mut flash = FlashRig::measured(8.0);
    let before_product = flash.repeating_product();
    assert!(
        (before_product - 8.0).abs() < 0.1,
        "AE стоит на цели 8, а не {before_product}"
    );
    flash.rig.fresh_events();

    // 1. Принято: в повторяющемся запросе фонарик на предвспышечной экспозиции.
    assert_eq!(flash.capture(SHOT), CommandOutcome::Accepted);
    let controls = flash.repeating_controls();
    assert_eq!(controls.flash, FlashControl::Torch);
    assert!(
        (flash.repeating_product() - 8.0).abs() < 0.1,
        "потолок 2^3,7 ≈ 13 не режет цель 8"
    );
    assert_eq!(
        flash.directive().flash.preflash,
        PreflashMetering::Active { shot: SHOT }
    );
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 0
        }
    );
    flash.assert_led_invariant("после принятия");

    // 2. Замеры: фон 8 (свет 1/8), с фонариком 4 (свет 1/4) → вклад фонарика 1/8, вспышки 1,87/8.
    flash.publish_good_samples(8.0, 4.0);
    let expected_product = 1.0 / (1.0 / 8.0 + 1.87 / 8.0);
    let expected_share = (1.87 / 8.0) / (1.0 / 8.0 + 1.87 / 8.0);
    assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT });
    flash.assert_led_invariant("в паузе");
    assert_eq!(
        flash.repeating_controls().flash,
        FlashControl::Off,
        "пауза идёт БЕЗ фонарика"
    );
    assert!(
        (flash.repeating_product() - 8.0).abs() < 0.1,
        "повторяющийся запрос вернулся к плану AE"
    );
    let planned = flash.directive().flash.planned.expect("план опубликован");
    assert_eq!(planned.shot, SHOT);
    assert_eq!(
        planned.ambient_frame,
        frame_time(flash.pre_request_frame()),
        "план помнит кадр, по которому посчитан свет сцены"
    );
    assert!((planned.mid_gray_product.seconds_iso() - expected_product).abs() < 1e-9);
    assert!((planned.flash_share.value() - expected_share).abs() < 1e-9);
    assert_eq!(
        flash.submitted_flash_stills(),
        0,
        "до конца паузы разовый запрос не уходит"
    );

    // 3. Пауза 100 мс: на 99-й мс ещё нет, на 100-й уходит.
    flash.rig.advance(ms(99));
    assert_eq!(flash.submitted_flash_stills(), 0);
    flash.rig.advance(ms(1));
    assert_eq!(flash.submitted_flash_stills(), 1);
    let (shot, single) = flash.rig.fake.last_submitted_flash_still().expect("запрос принят");
    assert_eq!(shot, SHOT);
    assert_eq!(single.flash, FlashControl::Single);
    let ExposurePlan::Sensor { exposure_time, iso } = single.exposure else {
        panic!("разовый запрос должен нести явную экспозицию");
    };
    let single_product = ExposureProduct::from_sensor(exposure_time, iso).seconds_iso();
    assert!(
        (single_product / expected_product - 1.0).abs() < 0.01,
        "экспозиция запроса {single_product} ≠ плану {expected_product}"
    );
    assert_eq!(flash.phase(), FlashPhase::Fired { shot: SHOT });
    flash.assert_led_invariant("после отправки");

    // 4. Результат кадра со вспышкой → ровно один итог Fired с планом и экспозицией запроса.
    flash.rig.advance(ms(120));
    flash.rig.fake.complete_flash_still(SHOT, FIRED_SINGLE);
    flash.rig.pump();
    let events = flash.rig.fresh_events();
    let reports = flash_reports(&events);
    let [
        FlashStillReport::Fired {
            shot,
            frame,
            plan,
            exposure_time: reported_time,
            iso: reported_iso,
            elapsed,
        },
    ] = reports.as_slice()
    else {
        panic!("ожидался один итог Fired, пришло {reports:?}");
    };
    assert_eq!(*shot, SHOT);
    assert_eq!(*plan, planned);
    assert_eq!((*reported_time, *reported_iso), (exposure_time, iso));
    assert_eq!(frame.flash, FIRED_SINGLE);
    assert_eq!(*elapsed, ms(220), "от принятия: пауза 100 + 120 до результата");
    assert_eq!(flash.observer.reports(), reports, "наблюдатель получил тот же итог");
    assert_eq!(flash.phase(), FlashPhase::Idle);
    flash.assert_led_invariant("после итога");
    let counters = flash.rig.engine.status().three_a.flash.counters;
    assert_eq!((counters.requested, counters.fired, counters.failed), (1, 1, 0));
    assert_eq!(counters.results_not_fired, 0);
    assert_eq!(flash.rig.engine.next_deadline(), None, "сроки сняты");
}

#[test]
fn result_without_fired_state_is_still_reported_but_counted() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.publish_good_samples(8.0, 4.0);
    flash.rig.advance(ms(100));
    flash.rig.fresh_events();
    flash.rig.fake.complete_flash_still(
        SHOT,
        FlashResult {
            mode: Some(FlashModeEcho::Single),
            state: Some(FlashState::Ready),
        },
    );
    flash.rig.pump();
    let reports = flash_reports(&flash.rig.fresh_events());
    assert!(
        matches!(reports.as_slice(), [FlashStillReport::Fired { .. }]),
        "{reports:?}"
    );
    assert_eq!(flash.rig.engine.status().three_a.flash.counters.results_not_fired, 1);
}

#[test]
fn clipped_torch_sample_lowers_the_preflash_exposure_once_then_plans_anyway() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    let first_product = flash.repeating_product();
    let ambient = sample(
        PreflashLight::Ambient,
        flash.pre_request_frame(),
        first_product,
        8.0,
        MeasurementQuality::Measured,
    );

    // Пересвеченный замер: экспозиция падает на шаг конфига (3 EV), фонарик остаётся.
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(sample(
            PreflashLight::Torch,
            28,
            first_product,
            1.0,
            MeasurementQuality::ClippedHighlights,
        )),
    });
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 1
        }
    );
    let lowered = flash.repeating_product();
    assert!(
        (first_product / lowered - 8.0).abs() < 0.3,
        "{first_product} → {lowered}: ждали /8"
    );
    assert_eq!(flash.repeating_controls().flash, FlashControl::Torch);
    assert_eq!(flash.rig.engine.status().three_a.flash.counters.preflash_remeasures, 1);

    // Тот же старый замер повторно (новое поколение ящика, та же метка кадра) не считается новым.
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(sample(
            PreflashLight::Torch,
            28,
            first_product,
            1.0,
            MeasurementQuality::ClippedHighlights,
        )),
    });
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 1
        }
    );

    // Пересвечен и после перемера: лимит перемеров исчерпан — считаем по тому, что есть.
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(sample(
            PreflashLight::Torch,
            40,
            lowered,
            2.0,
            MeasurementQuality::ClippedHighlights,
        )),
    });
    assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT });
    flash.assert_led_invariant("после перемера");
}

#[test]
fn foreign_stale_and_mismatched_torch_samples_are_ignored() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    let requested = flash.repeating_product();
    let ambient = sample(
        PreflashLight::Ambient,
        flash.pre_request_frame(),
        requested,
        8.0,
        MeasurementQuality::Measured,
    );
    let torch = sample(PreflashLight::Torch, 28, requested, 4.0, MeasurementQuality::Measured);

    // Чужая камера.
    let mut foreign = torch;
    foreign.measured_on = foreign_tag();
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(foreign),
    });
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 0
        },
        "чужая камера"
    );

    // Экспозиция кадра не та, что в запросе (кадр прошлой экспозиции).
    let mut wrong_product = torch;
    wrong_product.frame_product = product(requested * 2.0);
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(wrong_product),
    });
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 0
        },
        "чужая экспозиция"
    );

    // Замер с фонариком старше начала снимка (кадр до пола метки).
    let mut stale = torch;
    stale.frame = frame_time(0);
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(stale),
    });
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 0
        },
        "замер прошлого снимка"
    );

    // Те же годные замеры — принимаются.
    flash.publish_samples(PreflashSamples {
        ambient: Some(ambient),
        torch: Some(torch),
    });
    assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT });
}

#[test]
fn ambient_sample_older_than_the_limit_waits_for_a_fresh_one() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    let requested = flash.repeating_product();
    // Замер без фонарика (снят до запроса) на 1,3 с старше кадра с фонариком: другая сцена, считать нельзя.
    let old_ambient = sample(
        PreflashLight::Ambient,
        -50,
        requested,
        8.0,
        MeasurementQuality::Measured,
    );
    let torch = sample(PreflashLight::Torch, 28, requested, 4.0, MeasurementQuality::Measured);
    flash.publish_samples(PreflashSamples {
        ambient: Some(old_ambient),
        torch: Some(torch),
    });
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 0
        }
    );
    // Запоздавший замер кадра, снятого тоже ДО запроса, но свежее прежнего (запаздывание замера),
    // заменяет старый — и тот же замер с фонариком теперь годится.
    let fresh_ambient = sample(
        PreflashLight::Ambient,
        flash.pre_request_frame(),
        requested,
        8.0,
        MeasurementQuality::Measured,
    );
    flash.publish_samples(PreflashSamples {
        ambient: Some(fresh_ambient),
        torch: Some(torch),
    });
    assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT });
}

#[test]
fn preflash_timeout_fails_with_torch_off_and_one_report() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.rig.fresh_events();
    flash.rig.advance(ms(499));
    assert_eq!(
        flash.phase(),
        FlashPhase::Preflash {
            shot: SHOT,
            remeasures: 0
        }
    );
    flash.rig.advance(ms(1));
    let reports = flash_reports(&flash.rig.fresh_events());
    assert_eq!(failed(&reports), vec![(SHOT, FlashStillFailure::PreflashNotMeasured)]);
    assert_eq!(flash.phase(), FlashPhase::Idle);
    assert_eq!(flash.repeating_controls().flash, FlashControl::Off);
    flash.assert_led_invariant("после таймаута предвспышки");
    assert_eq!(flash.observer.reports(), reports);
}

#[test]
fn missing_result_times_out_and_a_late_result_is_ignored() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.publish_good_samples(8.0, 4.0);
    flash.rig.advance(ms(100));
    flash.rig.fresh_events();
    flash.rig.advance(ms(399));
    assert_eq!(flash.phase(), FlashPhase::Fired { shot: SHOT });
    flash.rig.advance(ms(1));
    let reports = flash_reports(&flash.rig.fresh_events());
    assert_eq!(failed(&reports), vec![(SHOT, FlashStillFailure::ResultTimedOut)]);
    // Поздний результат второго итога не создаёт.
    flash.rig.fake.complete_flash_still(SHOT, FIRED_SINGLE);
    flash.rig.pump();
    assert!(flash_reports(&flash.rig.fresh_events()).is_empty());
    assert_eq!(flash.observer.reports().len(), 1);
}

#[test]
fn rejected_single_request_fails_but_keeps_the_stream() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.publish_good_samples(8.0, 4.0);
    flash
        .rig
        .fake
        .push_flash_still_result(Err(RequestFailure::Backend(BackendFailure {
            operation: "submit_flash_still",
            code: 1,
            detail: "тест".to_owned(),
        })));
    flash.rig.fresh_events();
    flash.rig.advance(ms(100));
    let reports = flash_reports(&flash.rig.fresh_events());
    assert_eq!(failed(&reports), vec![(SHOT, FlashStillFailure::SubmitFailed)]);
    assert_eq!(
        flash.rig.state_name(),
        "streaming",
        "отказ запроса вспышки поток не ломает"
    );
    assert_eq!(flash.phase(), FlashPhase::Idle);
    flash.assert_led_invariant("после отказа отправки");
}

#[test]
fn lost_device_at_submit_reports_submit_failed_then_recovers_the_session() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.publish_good_samples(8.0, 4.0);
    flash
        .rig
        .fake
        .push_flash_still_result(Err(RequestFailure::DeviceDisconnected));
    flash.rig.fresh_events();
    flash.rig.advance(ms(100));
    let events = flash.rig.fresh_events();
    assert_eq!(
        failed(&flash_reports(&events)),
        vec![(SHOT, FlashStillFailure::SubmitFailed)]
    );
    assert_ne!(
        flash.rig.state_name(),
        "streaming",
        "потеря устройства уводит в восстановление"
    );
    assert_eq!(flash.phase(), FlashPhase::Idle);
}

#[test]
fn losing_the_session_in_preflash_or_gap_reports_session_lost_and_clears_the_directive() {
    for in_gap in [false, true] {
        let mut flash = FlashRig::measured(8.0);
        flash.capture(SHOT);
        if in_gap {
            flash.publish_good_samples(8.0, 4.0);
            assert_eq!(flash.phase(), FlashPhase::Gap { shot: SHOT });
        }
        flash.rig.fresh_events();
        flash.rig.fake.disconnect_current_device();
        flash.rig.pump();
        let reports = flash_reports(&flash.rig.fresh_events());
        assert_eq!(
            failed(&reports),
            vec![(SHOT, FlashStillFailure::SessionLost)],
            "в паузе: {in_gap}"
        );
        assert_eq!(flash.phase(), FlashPhase::Idle);
        assert_eq!(flash.directive().flash.preflash, PreflashMetering::Inactive);
        assert_eq!(
            flash.rig.engine.next_deadline(),
            None,
            "сроки сняты вместе с последовательностью"
        );
        assert_eq!(flash.observer.reports().len(), 1);
    }
}

#[test]
fn going_to_background_mid_preflash_turns_the_led_off_for_the_next_request() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.rig.fresh_events();
    assert_eq!(flash.rig.send(CameraCommand::EnterBackground), CommandOutcome::Accepted);
    let reports = flash_reports(&flash.rig.fresh_events());
    assert_eq!(failed(&reports), vec![(SHOT, FlashStillFailure::SessionLost)]);
    assert_eq!(flash.directive().flash.preflash, PreflashMetering::Inactive);
    // Вернулись: первый же запрос новой сессии без фонарика.
    assert_eq!(flash.rig.send(CameraCommand::EnterForeground), CommandOutcome::Accepted);
    assert_eq!(flash.repeating_controls().flash, FlashControl::Off);
    flash.assert_led_invariant("после возврата из фона");
}

#[test]
fn switching_lens_mid_preflash_reports_lens_changed_and_the_new_camera_has_no_torch() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.rig.fresh_events();
    assert_eq!(
        flash.rig.send(CameraCommand::SwitchTo {
            role: LensRole::UltraWide
        }),
        CommandOutcome::Accepted
    );
    let reports = flash_reports(&flash.rig.fresh_events());
    assert_eq!(failed(&reports), vec![(SHOT, FlashStillFailure::LensChanged)]);
    assert_eq!(flash.directive().flash.preflash, PreflashMetering::Inactive);
    assert_eq!(flash.phase(), FlashPhase::Idle);
    assert_eq!(flash.repeating_controls().flash, FlashControl::Off);
}

#[test]
fn close_mid_gap_reports_session_lost() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.publish_good_samples(8.0, 4.0);
    flash.rig.fresh_events();
    assert_eq!(flash.rig.send(CameraCommand::Close), CommandOutcome::Accepted);
    let reports = flash_reports(&flash.rig.fresh_events());
    assert_eq!(failed(&reports), vec![(SHOT, FlashStillFailure::SessionLost)]);
    assert_eq!(flash.rig.engine.next_deadline(), None, "срок паузы снят");
}

#[test]
fn own_ae_does_not_replan_during_the_sequence_and_catches_up_after() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    let preflash_product = flash.repeating_product();
    // Сцена резко потемнела (цель 16), пока идёт предвспышка: запрос не трогаем.
    let frame = flash.feed_repeating();
    flash.channel.publish(camera_contract::auto_exposure::ExposureProposal {
        measured_frame: frame_time(frame),
        measured_on: camera_contract::auto_exposure::CameraTag::of(&camera_contract::fixtures::camera_id("2")),
        target: product(16.0),
        metered_area: camera_contract::auto_exposure::MeteredArea::WholeFrame,
        convergence: camera_contract::auto_exposure::ProposalConvergence::Settled,
        quality: MeasurementQuality::Measured,
    });
    flash.deliver_pushes();
    assert_eq!(
        flash.repeating_product(),
        preflash_product,
        "AE молчит во время предвспышки"
    );
    flash.publish_good_samples(8.0, 4.0);
    assert!(
        (flash.repeating_product() - 8.0).abs() < 0.1,
        "в паузе AE всё ещё на старой цели"
    );
    // Разовый запрос и результат: после итога AE догоняет решение.
    flash.rig.advance(ms(100));
    flash.rig.fake.complete_flash_still(SHOT, FIRED_SINGLE);
    flash.rig.pump();
    assert!(
        (flash.repeating_product() - 16.0).abs() < 0.2,
        "после итога AE принял цель 16, а не {}",
        flash.repeating_product()
    );
}

#[test]
fn focus_trigger_during_preflash_carries_the_torch() {
    let mut flash = FlashRig::measured(8.0);
    flash.capture(SHOT);
    flash.rig.fake.take_calls();
    assert_eq!(
        flash.rig.send(CameraCommand::FocusAndExposeAt { point: center() }),
        CommandOutcome::Accepted
    );
    let calls = flash.rig.fake.take_calls();
    let trigger_flash: Vec<FlashControl> = calls
        .iter()
        .filter_map(|call| match call {
            BackendCall::SubmitFocusTrigger { controls, .. } => Some(controls.flash),
            _ => None,
        })
        .collect();
    assert_eq!(trigger_flash, vec![FlashControl::Torch]);
    flash.assert_led_invariant("после тапа в предвспышке");
}
