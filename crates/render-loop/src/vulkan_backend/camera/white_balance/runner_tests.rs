//! Белая потока рендера без GPU: указания владельца, холодный старт, переходы, источники, смена потока,
//! расписание статистики. Роль «владельца белого» (движка) играет тест: публикует указания и читает ответы.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::frame_passport::{CaptureColorHints, FrameCaptureMeta, MissingCaptureMeta};
use camera_contract::white::{
    WhileAwaitingWhite, WhiteRequestId, WhiteSourceId, WhiteSourceOutcome, WhiteTransition, WhiteUnavailableReason,
};
use camera_contract::white_reading::WhiteReading;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use scene_estimation::white_source::WhiteInputKind;

use super::test_support::{
    FAKE_ID, PROVISIONAL, chromaticity_of, directive_of, fake_registry, frame, frame_at, kelvin_of, new_stream, over,
    phone_hints, ready_outcome, request_of, runner, runner_with, runner_without_sources,
};
use super::*;
use crate::viewfinder::{WhiteLinks, WhiteSourceRegistry};

const HIDE: WhileAwaitingWhite = WhileAwaitingWhite::HideFrames;

/// Показывается ли кадр.
fn shown(plan: &FramePlan) -> bool {
    plan.presentation == IspPresentation::Show
}

#[test]
fn without_directives_the_white_is_provisional_and_nothing_is_hidden() {
    let (mut runner, _) = runner_without_sources();
    let stream = new_stream();
    let first = runner.plan_frame(&frame(&stream, 0));
    assert_eq!(first.white, PROVISIONAL, "стенд без движка: временная D65");
    assert!(shown(&first), "кадры без движка не прячутся");
    let diagnostics = runner.diagnostics();
    assert_eq!(diagnostics.frames_with_provisional_white, 1);
    assert_eq!(diagnostics.applied_white, None);
}

#[test]
fn the_directive_white_goes_to_the_isp_bit_exactly_whatever_the_scene_and_the_hints_say() {
    // Подсказки телефона уходят от указания всё дальше: картинка = указание.
    let (mut runner, links) = runner();
    links
        .directives
        .publish(directive_of(6500.0, WhiteTransition::Immediate, None));
    let stream = new_stream();
    let expected = chromaticity_of(6500.0);
    for index in 0..40 {
        let plan = runner.plan_frame(&frame_at(&stream, index, phone_hints(2800.0 + 100.0 * index as f64)));
        assert_eq!(plan.white, expected, "кадр {index}");
        assert!(shown(&plan));
    }
    let diagnostics = runner.diagnostics();
    assert!(diagnostics.hal_white.is_some(), "белая HAL — диагностика");
    assert!(diagnostics.hal_delta.is_some());
}

#[test]
fn the_phone_source_answers_the_hide_request_the_owner_closes_it_and_the_picture_appears_with_that_white() {
    let (mut runner, links) = runner();
    let fallback = directive_of(
        5600.0,
        WhiteTransition::Immediate,
        Some(request_of(1, WhiteSourceId::PHONE, HIDE)),
    );
    links.directives.publish(fallback);
    let stream = new_stream();
    // Первые 5 кадров — белая «прошлого сеанса» (неподвижная), затем автоматика мерит 2800 К.
    let mut plans = Vec::new();
    let mut answer = None;
    for index in 0..60 {
        let kelvin = if index < 5 {
            6500.0
        } else {
            2800.0 + f64::from((index % 2) as u8)
        };
        plans.push(runner.plan_frame(&frame_at(&stream, index, phone_hints(kelvin))));
        if let Some((_, published)) = links.answers.newer_than(None) {
            answer = Some(published);
            break;
        }
    }
    let answer = answer.expect("телефон ответил");
    assert_eq!(answer.request, WhiteRequestId(1));
    let WhiteSourceOutcome::Ready { illuminant, .. } = answer.outcome else {
        panic!("ожидался готовый ответ: {answer:?}");
    };
    let camera_contract::white::SceneIlluminant::Uniform(white) = illuminant else {
        panic!("один свет");
    };
    assert!((white.temperature.kelvin() - 2800.0).abs() < 150.0, "{white:?}");
    assert!(plans.iter().all(|plan| !shown(plan)), "пока заявка жива, кадры скрыты");
    assert!(plans.iter().all(|plan| plan.white == chromaticity_of(5600.0)));
    // Владелец принял ответ: публикует указание с белой телефона и закрывает заявку.
    links.directives.publish(WhiteDirective {
        white,
        transition: WhiteTransition::Over(std::time::Duration::from_millis(300)),
        request: None,
    });
    let next = runner.plan_frame(&frame_at(&stream, 100, phone_hints(2800.0)));
    assert!(shown(&next), "заявка закрылась — показ");
    assert_eq!(
        next.white,
        chromaticity_from_cct_duv(CctDuv {
            temperature: Kelvin(white.temperature.kelvin()),
            duv: Duv(white.tint.duv())
        })
        .unwrap(),
        "кадры были скрыты, поэтому переход мгновенный: плавность для глаз, а глаз ничего не видел"
    );
    let diagnostics = runner.diagnostics();
    assert_eq!(diagnostics.hidden_frames_cold_start, plans.len() as u64);
    assert_eq!(diagnostics.answers_published, 1);
    assert_eq!(diagnostics.cold_start, ColdStartState::Warm);
}

#[test]
fn hiding_stops_at_the_limit_and_the_late_answer_is_applied_smoothly() {
    let (registry, board) = fake_registry(WhiteInputKind::ColorHints);
    let config = WhiteBalanceConfig {
        max_hidden_frames: 5,
        ..WhiteBalanceConfig::default()
    };
    let (mut runner, links) = runner_with(config, registry);
    links.directives.publish(directive_of(
        5600.0,
        WhiteTransition::Immediate,
        Some(request_of(1, FAKE_ID, HIDE)),
    ));
    let stream = new_stream();
    let mut verdicts = Vec::new();
    for index in 0..8 {
        verdicts.push(shown(&runner.plan_frame(&frame(&stream, index))));
    }
    assert_eq!(verdicts, [false, false, false, false, false, true, true, true]);
    let diagnostics = runner.diagnostics();
    assert_eq!(
        (diagnostics.hidden_frames_cold_start, diagnostics.cold_start_forced),
        (5, 1)
    );
    // Поздний ответ: источник сказал «лампа», владелец принял и переезжает плавно (картинка уже на экране).
    board.reply(1, ready_outcome(2800.0));
    let late = directive_of(2800.0, over(300), None);
    links.directives.publish(late);
    let mut whites = Vec::new();
    for index in 8..40 {
        whites.push(runner.plan_frame(&frame(&stream, index)).white);
    }
    let target = chromaticity_of(2800.0);
    assert_ne!(whites[0], target, "картинка видна — переход плавный");
    assert!(
        whites
            .iter()
            .any(|white| *white != target && *white != chromaticity_of(5600.0)),
        "есть промежуточные кадры"
    );
    assert_eq!(*whites.last().unwrap(), target);
}

#[test]
fn the_opening_count_survives_a_stream_change_and_a_reissued_request_does_not_hide_a_shown_picture() {
    let (registry, _board) = fake_registry(WhiteInputKind::ColorHints);
    let config = WhiteBalanceConfig {
        max_hidden_frames: 4,
        ..WhiteBalanceConfig::default()
    };
    let (mut runner, links) = runner_with(config, registry);
    links.directives.publish(directive_of(
        5600.0,
        WhiteTransition::Immediate,
        Some(request_of(1, FAKE_ID, HIDE)),
    ));
    let first = new_stream();
    assert!(!shown(&runner.plan_frame(&frame(&first, 0))));
    assert!(!shown(&runner.plan_frame(&frame(&first, 1))));
    // Линза сменилась посреди ожидания: счёт открытия не обнуляется.
    runner.forget_stream();
    let second = new_stream();
    assert!(!shown(&runner.plan_frame(&frame(&second, 2))));
    assert!(!shown(&runner.plan_frame(&frame(&second, 3))));
    assert!(
        shown(&runner.plan_frame(&frame(&second, 4))),
        "предел общий на открытие: 4 скрытых кадра"
    );
    // Владелец переиздал заявку (новый номер, всё ещё HideFrames): уже показанное не гаснет.
    links.directives.publish(directive_of(
        5600.0,
        WhiteTransition::Immediate,
        Some(request_of(2, FAKE_ID, HIDE)),
    ));
    assert!(shown(&runner.plan_frame(&frame(&second, 5))));
    assert_eq!(runner.diagnostics().hidden_frames_cold_start, 4);
}

#[test]
fn a_stream_change_resubmits_the_pending_request_and_keeps_the_white() {
    let (registry, board) = fake_registry(WhiteInputKind::ColorHints);
    let (mut runner, links) = runner_with(WhiteBalanceConfig::default(), registry);
    links.directives.publish(directive_of(
        3300.0,
        WhiteTransition::Immediate,
        Some(request_of(4, FAKE_ID, HIDE)),
    ));
    let first = new_stream();
    runner.plan_frame(&frame(&first, 0));
    runner.forget_stream();
    let second = new_stream();
    let plan = runner.plan_frame(&frame(&second, 1));
    assert_eq!(
        board.requests(),
        vec![WhiteRequestId(4), WhiteRequestId(4)],
        "та же заявка подана новому потоку"
    );
    assert_eq!(plan.white, chromaticity_of(3300.0), "белая переживает смену потока");
}

#[test]
fn a_settled_white_survives_stream_changes_without_asking_the_source_again() {
    let (registry, board) = fake_registry(WhiteInputKind::ColorHints);
    let (mut runner, links) = runner_with(WhiteBalanceConfig::default(), registry);
    links
        .directives
        .publish(directive_of(3300.0, WhiteTransition::Immediate, None));
    let white_before = runner.plan_frame(&frame(&new_stream(), 0)).white;
    runner.forget_stream();
    let other_lens = new_stream();
    let after = runner.plan_frame(&frame(&other_lens, 1));
    assert_eq!(after.white, white_before);
    assert!(shown(&after));
    assert!(board.requests().is_empty(), "источник заново не спрошен");
}

#[test]
fn a_new_session_starts_from_the_latest_directive_at_once() {
    // Новая сессия показа (surface): ящик указаний тот же, раннер новый.
    let links = WhiteLinks::default();
    links.directives.publish(directive_of(4000.0, over(500), None));
    let mut first = WhiteBalanceRunner::new(WhiteBalanceParts {
        config: WhiteBalanceConfig::default(),
        links: links.clone(),
    })
    .unwrap();
    let stream = new_stream();
    let before = first.plan_frame(&frame(&stream, 0)).white;
    drop(first);
    let mut second = WhiteBalanceRunner::new(WhiteBalanceParts {
        config: WhiteBalanceConfig::default(),
        links,
    })
    .unwrap();
    let plan = second.plan_frame(&frame(&stream, 0));
    assert_eq!(plan.white, before);
    assert_eq!(plan.white, chromaticity_of(4000.0));
    assert!(shown(&plan));
}

#[test]
fn unregistered_and_wrong_input_sources_are_refused_without_changing_the_picture() {
    // Пустой реестр.
    let (mut empty, links) = runner_without_sources();
    links.directives.publish(directive_of(
        5600.0,
        WhiteTransition::Immediate,
        Some(request_of(1, WhiteSourceId::PHONE, HIDE)),
    ));
    let stream = new_stream();
    let plan = empty.plan_frame(&frame(&stream, 0));
    assert_eq!(plan.white, chromaticity_of(5600.0));
    let (_, answer) = links.answers.newer_than(None).expect("отказ опубликован");
    assert_eq!(
        answer.outcome,
        WhiteSourceOutcome::Unavailable {
            reason: WhiteUnavailableReason::NotRegistered
        }
    );
    // Источник, которому нужны плитки.
    let (registry, _) = fake_registry(WhiteInputKind::SceneTiles);
    let (mut tiles, links) = runner_with(WhiteBalanceConfig::default(), registry);
    links.directives.publish(directive_of(
        5600.0,
        WhiteTransition::Immediate,
        Some(request_of(1, FAKE_ID, HIDE)),
    ));
    tiles.plan_frame(&frame(&stream, 0));
    let (_, answer) = links.answers.newer_than(None).expect("отказ опубликован");
    assert_eq!(
        answer.outcome,
        WhiteSourceOutcome::Unavailable {
            reason: WhiteUnavailableReason::InputNotProvided
        }
    );
    assert_eq!(tiles.diagnostics().last_answer, Some(answer));
}

#[test]
fn a_silent_source_hides_exactly_the_limit_of_frames_and_then_the_picture_shows() {
    let limit = WhiteBalanceConfig::default().max_hidden_frames;
    let (registry, _board) = fake_registry(WhiteInputKind::ColorHints);
    let (mut runner, links) = runner_with(WhiteBalanceConfig::default(), registry);
    links.directives.publish(directive_of(
        4000.0,
        WhiteTransition::Immediate,
        Some(request_of(1, FAKE_ID, HIDE)),
    ));
    let stream = new_stream();
    let shown_frames: Vec<bool> = (0..i64::from(2 * limit))
        .map(|index| shown(&runner.plan_frame(&frame(&stream, index))))
        .collect();
    let hidden = shown_frames.iter().filter(|shown| !**shown).count();
    assert_eq!(hidden, limit as usize, "источник молчит, скрыто ровно предел");
    assert!(
        shown_frames[limit as usize..].iter().all(|shown| *shown),
        "после предела кадры показываются"
    );
}

#[test]
fn frames_without_metadata_get_the_directive_white_and_are_shown() {
    let (mut runner, links) = runner();
    links
        .directives
        .publish(directive_of(4000.0, WhiteTransition::Immediate, None));
    let stream = new_stream();
    runner.plan_frame(&frame(&stream, 0));
    let mut passport = frame(&stream, 5);
    passport.capture = FrameCaptureMeta::Missing {
        reason: MissingCaptureMeta::PairingTimedOut,
    };
    let plan = runner.plan_frame(&passport);
    assert!(shown(&plan));
    assert_eq!(plan.white, chromaticity_of(4000.0));
}

#[test]
fn a_stream_without_colour_calibration_still_gets_the_directive_white_and_has_no_hal_comparison() {
    let (mut runner, links) = runner();
    links
        .directives
        .publish(directive_of(4000.0, WhiteTransition::Immediate, None));
    let mut passport = (*new_stream()).clone();
    passport.color_calibration = Vec::new();
    let stream = Arc::new(passport);
    let plan = runner.plan_frame(&frame(&stream, 0));
    assert!(shown(&plan));
    assert_eq!(plan.white, chromaticity_of(4000.0));
    // Профиля цвета нет — белую HAL не из чего строить, но белая владельца идёт в картинку как обычно.
    assert_eq!(runner.diagnostics().hal_white, None);
}

#[test]
fn diagnostics_report_applied_target_transition_and_the_phone_comparison() {
    let (mut runner, links) = runner();
    links
        .directives
        .publish(directive_of(6500.0, WhiteTransition::Immediate, None));
    let stream = new_stream();
    runner.plan_frame(&frame_at(&stream, 0, phone_hints(4000.0)));
    links.directives.publish(directive_of(2800.0, over(500), None));
    runner.plan_frame(&frame_at(&stream, 1, phone_hints(4000.0)));
    let diagnostics = runner.diagnostics();
    assert!(diagnostics.transition_active);
    assert!((diagnostics.applied_white.unwrap().temperature_kelvin - 6500.0).abs() < 5.0);
    assert!((diagnostics.target_white.unwrap().temperature_kelvin - 2800.0).abs() < 5.0);
    assert_eq!(diagnostics.directive_generation, Some(2));
    let hal = diagnostics.hal_white.expect("подсказки телефона");
    assert!((hal.temperature_kelvin - 4000.0).abs() < 100.0);
    let delta = diagnostics.hal_delta.expect("разница");
    assert!(delta.mired.abs() > 50.0, "{delta:?}");
    assert_eq!(kelvin_of(chromaticity_of(2800.0)).round(), 2800.0);
}

#[test]
fn an_immediate_directive_is_applied_by_the_very_next_frame_on_a_running_stream() {
    // Рука на шкале: ползунок двигает белую по кадру, а не «плавно догоняет».
    let (mut runner, links) = runner_without_sources();
    links
        .directives
        .publish(directive_of(6500.0, WhiteTransition::Immediate, None));
    let stream = new_stream();
    for index in 0..5 {
        assert_eq!(runner.plan_frame(&frame(&stream, index)).white, chromaticity_of(6500.0));
    }
    links
        .directives
        .publish(directive_of(3000.0, WhiteTransition::Immediate, None));
    let next = runner.plan_frame(&frame(&stream, 5));
    assert_eq!(
        next.white,
        chromaticity_of(3000.0),
        "первый же кадр после публикации, без промежуточных"
    );
    assert!(shown(&next));
}

#[test]
fn many_directives_between_two_frames_collapse_to_the_last_one() {
    let (mut runner, links) = runner_without_sources();
    links
        .directives
        .publish(directive_of(6500.0, WhiteTransition::Immediate, None));
    let stream = new_stream();
    runner.plan_frame(&frame(&stream, 0));
    // Шкала отдала 120 указаний/с: между двумя кадрами их набралось много.
    for step in 0..40 {
        links.directives.publish(directive_of(
            2800.0 + 50.0 * f64::from(step),
            WhiteTransition::Immediate,
            None,
        ));
    }
    let last_kelvin = 2800.0 + 50.0 * 39.0;
    let next = runner.plan_frame(&frame(&stream, 1));
    assert_eq!(
        next.white,
        chromaticity_of(last_kelvin),
        "применяется последнее, не первое и не среднее"
    );
    assert_eq!(
        runner.plan_frame(&frame(&stream, 2)).white,
        chromaticity_of(last_kelvin)
    );
}

#[test]
fn the_phone_reading_reaches_the_board_without_making_an_answer() {
    let registry = WhiteSourceRegistry::with_phone(Default::default()).unwrap();
    let config = WhiteBalanceConfig {
        reading_every_frames: 1,
        ..WhiteBalanceConfig::default()
    };
    let (mut runner, links) = runner_with(config, registry);
    let stream = new_stream();
    // Заявки нет: показание живёт без неё.
    links
        .directives
        .publish(directive_of(5600.0, WhiteTransition::Immediate, None));
    for index in 0..5 {
        runner.plan_frame(&frame_at(&stream, index, phone_hints(6500.0)));
        assert_eq!(
            links.readings.latest_of(WhiteSourceId::PHONE),
            Some(WhiteReading::NotMeasuredYet),
            "белая прошлого сеанса неподвижна: числа нет, кадр {index}"
        );
    }
    runner.plan_frame(&frame_at(&stream, 5, phone_hints(2800.0)));
    let Some(WhiteReading::Measured { white }) = links.readings.latest_of(WhiteSourceId::PHONE) else {
        panic!("после сдвига показание измерено");
    };
    assert!((white.temperature.kelvin() - 2800.0).abs() < 100.0, "{white:?}");
    assert_eq!(links.answers.newer_than(None), None, "показание ответом не стало");
    assert_eq!(runner.diagnostics().answers_published, 0);
    // Новый поток (другой Arc паспорта): прежнее число забыто сразу, до первого кадра с подсказкой.
    runner.plan_frame(&frame_at(&new_stream(), 6, CaptureColorHints::default()));
    assert_eq!(
        links.readings.latest_of(WhiteSourceId::PHONE),
        Some(WhiteReading::NotMeasuredYet)
    );
}
