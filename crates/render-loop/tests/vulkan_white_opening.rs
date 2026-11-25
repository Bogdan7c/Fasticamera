//! Белая при открытии на настоящем драйвере (RADV, полная validation): владелец белого (роль играет
//! `WhiteTestEngine`) просит у источника свет, кадры прячутся до ответа, и первый показанный кадр побитово равен
//! кадру эталонного прогона, у которого эта белая названа с самого начала. Дальше свет сцены и подсказки телефона
//! меняются, а указаний нет — картинка при той же сцене побитово прежняя.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod awb_support;
mod support;
mod vulkan_support;
mod white_engine;
mod white_rig;

use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::white::{
    SceneIlluminant, WhiteSourceFailure, WhiteSourceId, WhiteSourceOutcome, WhiteUnavailableReason, WhiteUnknownReason,
};
use camera_contract::white_reading::WhiteReading;
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::{WhiteLinks, WhiteSourceRegistry};
use scene_estimation::white_source::{WhiteInputKind, WhiteRequestRefusal};
use white_engine::{SCRIPTED_ID, Script, WhiteTestEngine, held_setting, lamp_outcome, scripted_registry, setting_of};
use white_rig::{FrameImage, Rig, rig_or_skip, scene_with_light_change};

/// Дневной свет из конфига владельца: запасная белая до ответа источника.
const FALLBACK_KELVIN: f64 = 5600.0;
const FRAMES: i64 = 40;
const LIGHT_CHANGES_AT: i64 = 20;

/// Подсказки телефона: первые кадры тихие, потом «дикие» — меняются на каждом кадре.
fn wild_hints_after(first_wild_frame: i64) -> impl Fn(i64) -> CaptureColorHints {
    move |frame| {
        if frame < first_wild_frame {
            CaptureColorHints::default()
        } else {
            awb_support::phone_hints_for_white(awb_support::planckian_white(2000.0 + 400.0 * (frame % 12) as f64))
        }
    }
}

fn start(links: WhiteLinks, config: WhiteBalanceConfig) -> Option<(Rig, std::sync::Arc<support::Journal>)> {
    rig_or_skip(
        scene_with_light_change(FRAMES.max(200), LIGHT_CHANGES_AT),
        links,
        CaptureColorHints::default(),
        config,
    )
}

/// Эталонный прогон: белая названа с первого кадра, заявок нет.
fn reference_images(setting: camera_contract::control::WhiteBalanceSetting, frames: i64) -> Vec<FrameImage> {
    let (mut rig, journal) = start(held_setting(setting), WhiteBalanceConfig::default()).expect("GPU есть");
    let images = rig.drive(0..frames, &wild_hints_after(10), &mut |_| {});
    rig.finish(&journal);
    images
}

/// Прогон с владельцем белого и источником из `registry`: заявка при открытии, ответ читает движок теста.
struct Opened {
    images: Vec<FrameImage>,
    engine: WhiteTestEngine,
    diagnostics: render_loop::viewfinder::WhiteBalanceDiagnostics,
}

fn opened_run(
    registry: WhiteSourceRegistry,
    source: WhiteSourceId,
    config: WhiteBalanceConfig,
    frames: i64,
    hints: &dyn Fn(i64) -> CaptureColorHints,
) -> Option<Opened> {
    let links = WhiteLinks {
        sources: registry,
        ..WhiteLinks::default()
    };
    let mut engine = WhiteTestEngine::open(&links, FALLBACK_KELVIN, source);
    let (mut rig, journal) = start(links, config)?;
    let images = rig.drive(0..frames, hints, &mut |_| engine.react());
    let diagnostics = rig.viewfinder().white_balance;
    rig.finish(&journal);
    Some(Opened {
        images,
        engine,
        diagnostics,
    })
}

fn first_shown(images: &[FrameImage]) -> usize {
    images.iter().position(Option::is_some).expect("картинка показана")
}

/// Все кадры от `from` совпадают побитово.
fn assert_same_from(actual: &[FrameImage], expected: &[FrameImage], from: usize, what: &str) {
    for index in from..actual.len() {
        assert!(actual[index].is_some(), "{what}: кадр {index} показан");
        assert_eq!(
            actual[index], expected[index],
            "{what}: кадр {index} побитово как у эталона"
        );
    }
}

#[test]
fn a_lamp_answer_at_opening_gives_the_reference_picture_and_nothing_moves_it_afterwards() {
    let lamp = setting_of(2800.0);
    let (registry, log) = scripted_registry(
        Script::AnswerAfter {
            observed_frames: 3,
            outcome: lamp_outcome(2800.0),
        },
        WhiteInputKind::ColorHints,
    );
    let Some(run) = opened_run(
        registry,
        SCRIPTED_ID,
        WhiteBalanceConfig::default(),
        FRAMES,
        &wild_hints_after(10),
    ) else {
        return;
    };
    let reference = reference_images(lamp, FRAMES);
    assert!(
        reference.iter().all(Option::is_some),
        "в эталоне белая известна с начала"
    );
    assert_ne!(
        reference[usize::try_from(LIGHT_CHANGES_AT).unwrap() - 1],
        reference[usize::try_from(LIGHT_CHANGES_AT).unwrap() + 1],
        "свет сцены в самом деле сменился: картинка при той же белой другая"
    );

    let first = first_shown(&run.images);
    assert!(
        (3..=6).contains(&first),
        "до ответа кадры скрыты, после — показаны: первый показанный {first}"
    );
    assert!(run.images[..first].iter().all(Option::is_none));
    assert_same_from(&run.images, &reference, first, "лампа от источника");
    assert_eq!(run.engine.current(), lamp);
    assert_eq!(run.engine.answers_accepted, 1);
    assert_eq!(run.diagnostics.hidden_frames_cold_start, first as u64);
    assert_eq!(run.diagnostics.cold_start_forced, 0);
    assert_eq!(run.diagnostics.answers_published, 1);
    assert!((run.diagnostics.applied_white.expect("белая").temperature_kelvin - 2800.0).abs() < 1.0);
    assert_eq!(log.lock().unwrap().requests.len(), 1, "источник спрошен ровно один раз");
}

#[test]
fn the_phone_source_through_colour_hints_gives_the_reference_picture_of_its_own_answer() {
    // Первые 5 кадров автоматика телефона несёт «прошлый» свет (6500 К), дальше мерит лампу 2800 К.
    let lamp = awb_support::planckian_white(2800.0);
    let past = awb_support::planckian_white(6500.0);
    let hints = move |frame: i64| {
        if frame >= 20 {
            // После открытия подсказки разгуливаются: на картинку не влияют.
            return awb_support::phone_hints_for_white(awb_support::planckian_white(
                2000.0 + 300.0 * (frame % 9) as f64,
            ));
        }
        awb_support::phone_hints_at_frame(frame, 5, past, lamp)
    };
    let registry = WhiteSourceRegistry::with_phone(WhiteBalanceConfig::default().phone).expect("числа верны");
    let Some(run) = opened_run(
        registry,
        WhiteSourceId::PHONE,
        WhiteBalanceConfig::default(),
        FRAMES,
        &hints,
    ) else {
        return;
    };
    let answered = run.engine.current();
    assert_eq!(
        run.engine.answers_accepted, 1,
        "телефон ответил готовой белой: {:?}",
        run.engine.answers_read
    );
    assert!(
        (answered.temperature.kelvin() - 2800.0).abs() < 120.0,
        "ответ телефона — лампа, а не «прошлый» свет: {answered:?}"
    );
    let reference = reference_images(answered, FRAMES);
    let first = first_shown(&run.images);
    assert!(first >= 6, "ответ не раньше устоявшегося свежего значения: {first}");
    assert!(first < 20, "в пределе скрытия: {first}");
    assert_same_from(&run.images, &reference, first, "белая от телефона");
}

/// Исходы, после которых белая обязана остаться запасной, а картинка — как у эталона с ней.
fn refusal_cases() -> Vec<(&'static str, WhiteSourceRegistry, WhiteSourceId, WhiteSourceOutcome)> {
    let scripted = |script, input| scripted_registry(script, input).0;
    let answer = |outcome| Script::AnswerAfter {
        observed_frames: 2,
        outcome,
    };
    let unknown = WhiteSourceOutcome::Unknown {
        reason: WhiteUnknownReason::SceneNotMeasurable,
    };
    let failed = WhiteSourceOutcome::Failed {
        failure: WhiteSourceFailure::Executor,
    };
    let silent = WhiteSourceOutcome::Unavailable {
        reason: WhiteUnavailableReason::AutomationSilent,
    };
    vec![
        (
            "не знаю",
            scripted(answer(unknown), WhiteInputKind::ColorHints),
            SCRIPTED_ID,
            unknown,
        ),
        (
            "сломался",
            scripted(answer(failed), WhiteInputKind::ColorHints),
            SCRIPTED_ID,
            failed,
        ),
        (
            "занят",
            scripted(Script::Refuse(WhiteRequestRefusal::Busy), WhiteInputKind::ColorHints),
            SCRIPTED_ID,
            WhiteSourceOutcome::Busy,
        ),
        (
            "источник отказал",
            scripted(
                Script::Refuse(WhiteRequestRefusal::Unavailable(
                    WhiteUnavailableReason::AutomationSilent,
                )),
                WhiteInputKind::ColorHints,
            ),
            SCRIPTED_ID,
            silent,
        ),
        (
            "не зарегистрирован",
            WhiteSourceRegistry::new(),
            SCRIPTED_ID,
            WhiteSourceOutcome::Unavailable {
                reason: WhiteUnavailableReason::NotRegistered,
            },
        ),
        (
            "нужны плитки",
            scripted(Script::Silent, WhiteInputKind::SceneTiles),
            SCRIPTED_ID,
            WhiteSourceOutcome::Unavailable {
                reason: WhiteUnavailableReason::InputNotProvided,
            },
        ),
        (
            "автоматика телефона молчит",
            WhiteSourceRegistry::with_phone(WhiteBalanceConfig::default().phone).expect("числа верны"),
            WhiteSourceId::PHONE,
            silent,
        ),
    ]
}

#[test]
fn every_refusal_leaves_the_picture_on_the_fallback_white_and_is_told_apart_in_the_answer() {
    let reference = reference_images(setting_of(FALLBACK_KELVIN), FRAMES);
    let mut seen_outcomes = Vec::new();
    for (name, registry, source, expected) in refusal_cases() {
        // Автоматика телефона молчит, только когда подсказок нет вовсе; остальным источникам подсказки безразличны.
        let hints: &dyn Fn(i64) -> CaptureColorHints = if source == WhiteSourceId::PHONE {
            &|_| CaptureColorHints::default()
        } else {
            &wild_hints_after(10)
        };
        let Some(run) = opened_run(registry, source, WhiteBalanceConfig::default(), FRAMES, hints) else {
            return;
        };
        let answer = run
            .engine
            .answers_read
            .first()
            .unwrap_or_else(|| panic!("{name}: ответа нет"));
        assert_eq!(answer.outcome, expected, "{name}: исход различим");
        assert_eq!(run.engine.answers_read.len(), 1, "{name}: на заявку один ответ");
        assert_eq!(run.engine.answers_accepted, 0, "{name}: белую отказ не меняет");
        assert_eq!(run.engine.current(), setting_of(FALLBACK_KELVIN), "{name}");
        assert_eq!(
            run.diagnostics.last_answer,
            Some(*answer),
            "{name}: ответ виден в диагностике"
        );
        let first = first_shown(&run.images);
        assert!(first <= 20, "{name}: скрытие ограничено: {first}");
        assert_same_from(&run.images, &reference, first, name);
        seen_outcomes.push(format!("{:?}", answer.outcome));
    }
    seen_outcomes.sort();
    seen_outcomes.dedup();
    assert_eq!(seen_outcomes.len(), 6, "исходы различимы: {seen_outcomes:?}");
}

#[test]
fn hiding_lasts_exactly_until_the_answer_and_a_silent_source_is_bounded_by_the_limit() {
    // Ответ после 7 кадров: ответ публикуется на 7-м кадре, движок читает его после него, указание видно на 8-м.
    let (registry, _) = scripted_registry(
        Script::AnswerAfter {
            observed_frames: 7,
            outcome: lamp_outcome(2800.0),
        },
        WhiteInputKind::ColorHints,
    );
    let Some(answered) = opened_run(registry, SCRIPTED_ID, WhiteBalanceConfig::default(), 20, &|_| {
        CaptureColorHints::default()
    }) else {
        return;
    };
    assert_eq!(first_shown(&answered.images), 7);
    assert_eq!(answered.diagnostics.hidden_frames_cold_start, 7);

    // Молчащий источник: скрыто ровно предел, потом принудительный показ с текущей (запасной) белой.
    let limit = 6;
    let config = WhiteBalanceConfig {
        max_hidden_frames: limit,
        ..WhiteBalanceConfig::default()
    };
    let (registry, _) = scripted_registry(Script::Silent, WhiteInputKind::ColorHints);
    let Some(silent) = opened_run(registry, SCRIPTED_ID, config, 30, &|_| CaptureColorHints::default()) else {
        return;
    };
    assert_eq!(first_shown(&silent.images), limit as usize);
    assert_eq!(silent.diagnostics.hidden_frames_cold_start, u64::from(limit));
    assert_eq!(silent.diagnostics.cold_start_forced, 1);
    let reference = reference_images(setting_of(FALLBACK_KELVIN), 30);
    assert_same_from(
        &silent.images,
        &reference,
        limit as usize,
        "показ после предела — запасная белая",
    );
}

#[test]
fn a_late_answer_after_the_limit_is_applied_smoothly_and_ends_bit_exactly_on_the_answered_white() {
    let limit = 4;
    let config = WhiteBalanceConfig {
        max_hidden_frames: limit,
        ..WhiteBalanceConfig::default()
    };
    // Источник отвечает после 12 кадров, когда картинка уже показана с запасной белой.
    let (registry, _) = scripted_registry(
        Script::AnswerAfter {
            observed_frames: 12,
            outcome: lamp_outcome(2800.0),
        },
        WhiteInputKind::ColorHints,
    );
    let frames = 60;
    let Some(run) = opened_run(registry, SCRIPTED_ID, config, frames, &|_| CaptureColorHints::default()) else {
        return;
    };
    let fallback = reference_images(setting_of(FALLBACK_KELVIN), frames);
    let lamp = reference_images(setting_of(2800.0), frames);
    assert_eq!(first_shown(&run.images), limit as usize);
    let in_between: Vec<usize> = (limit as usize..run.images.len())
        .filter(|&index| run.images[index] != fallback[index] && run.images[index] != lamp[index])
        .collect();
    assert!(
        in_between.len() >= 5,
        "кадры между старой и новой белой есть: плавно, а не скачком ({in_between:?})"
    );
    // 0,3 с ≈ 18 кадров после указания: дальше каждый кадр побитово эталон лампы.
    let settled_from = *in_between.last().expect("есть промежуточные") + 1;
    assert!(settled_from < 45, "переход закончился: {settled_from}");
    assert_same_from(&run.images, &lamp, settled_from, "после плавного перехода");
    assert_eq!(run.engine.answers_accepted, 1);
    assert_eq!(run.diagnostics.cold_start_forced, 1);
}

/// Прогон с настоящим телефоном и показаниями на каждом кадре: что лежало на доске после каждого кадра.
fn phone_reading_run(hints: &dyn Fn(i64) -> CaptureColorHints) -> Option<(WhiteTestEngine, Vec<Option<WhiteReading>>)> {
    let registry = WhiteSourceRegistry::with_phone(WhiteBalanceConfig::default().phone).expect("числа верны");
    let links = WhiteLinks {
        sources: registry,
        ..WhiteLinks::default()
    };
    let readings = links.readings.clone();
    let mut engine = WhiteTestEngine::open(&links, FALLBACK_KELVIN, WhiteSourceId::PHONE);
    let config = WhiteBalanceConfig {
        reading_every_frames: 1,
        ..WhiteBalanceConfig::default()
    };
    let (mut rig, journal) = start(links, config)?;
    let mut on_board = Vec::new();
    rig.drive(0..FRAMES, hints, &mut |_| {
        engine.react();
        on_board.push(readings.latest_of(WhiteSourceId::PHONE));
    });
    rig.finish(&journal);
    Some((engine, on_board))
}

#[test]
fn the_phone_reading_on_the_board_is_the_number_its_answer_gives_and_not_a_leftover() {
    // Кадры 0–4: автоматика телефона несёт «прошлый» свет 6500 К неподвижно; с 5-го мерит лампу 2800 К.
    let lamp = awb_support::planckian_white(2800.0);
    let past = awb_support::planckian_white(6500.0);
    let hints = move |frame: i64| awb_support::phone_hints_at_frame(frame, 5, past, lamp);
    let Some((engine, on_board)) = phone_reading_run(&hints) else {
        return;
    };
    for (frame, reading) in on_board.iter().take(5).enumerate() {
        assert_eq!(
            *reading,
            Some(WhiteReading::NotMeasuredYet),
            "кадр {frame}: белая прошлого сеанса показанием не бывает"
        );
    }
    let Some(WhiteReading::Measured { white: first }) = on_board[5] else {
        panic!("первый сдвинувшийся кадр уже измерен: {:?}", on_board[5]);
    };
    assert!((first.temperature.kelvin() - 2800.0).abs() < 120.0, "{first:?}");
    // Показание не ждёт ровных кадров, а ответ ждёт: когда ответ дан, число то же, что в нём.
    assert_eq!(engine.answers_accepted, 1, "телефон ответил: {:?}", engine.answers_read);
    let answer = engine.answers_read[0];
    let WhiteSourceOutcome::Ready {
        illuminant: SceneIlluminant::Uniform(answered),
        ..
    } = answer.outcome
    else {
        panic!("ожидался готовый ответ: {answer:?}");
    };
    let answered_at = on_board
        .iter()
        .position(|reading| *reading == Some(WhiteReading::Measured { white: answered }))
        .expect("число ответа когда-то лежало на доске");
    assert!(answered_at >= 5, "не раньше первого измеренного кадра: {answered_at}");
}

#[test]
fn while_the_phone_hints_stand_still_the_board_never_shows_a_number() {
    let past = awb_support::planckian_white(6500.0);
    let hints = move |_frame: i64| awb_support::phone_hints_for_white(past);
    let Some((engine, on_board)) = phone_reading_run(&hints) else {
        return;
    };
    assert_eq!(on_board.len(), FRAMES as usize);
    assert!(
        on_board
            .iter()
            .all(|reading| *reading == Some(WhiteReading::NotMeasuredYet)),
        "{on_board:?}"
    );
    assert_eq!(engine.answers_accepted, 0, "белой от телефона нет: она и не показана");
}
