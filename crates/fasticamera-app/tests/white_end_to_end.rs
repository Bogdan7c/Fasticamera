//! Сквозной тест белой (план white-balance, сессия 01): НАСТОЯЩИЙ движок камеры (`CameraRuntime` на fake-бэкенде,
//! собранный продуктовым `start_camera_with_links`) + НАСТОЯЩИЙ бэкенд показа (RADV headless, полная validation)
//! на общих ящиках белой (`WhiteAnswerChannel`, указания) и источники белого, зарегистрированные в реестре. Тест
//! играет только роль камеры (кадры в рендер, результаты кадров в движок) и света (фальшивый источник).
//!
//! Что доказывается ПИКСЕЛЯМИ: кадр побитово равен кадру эталонного прогона, у которого та же белая названа с
//! самого начала и заявок нет (`white_engine::held_setting`). Совпадение — это и есть «белая в ISP такая»; несовпадение
//! при той же сцене — белая ушла. Ни рука, ни подсказки телефона, ни смена света сцены кадр не двигают.
//!
//! Стенды рендера подключены по пути из `render-loop/tests` (второй копии не заводим), как в `look_end_to_end.rs`.
//! Шов «новый источник»: фальшивый источник — один файл `white_end_to_end/fake_source.rs` + одна строка регистрации.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]
#![allow(
    dead_code,
    reason = "стенды render-loop общие: каждый тестовый файл берёт свою часть"
)]

#[path = "../../render-loop/tests/awb_support/mod.rs"]
mod awb_support;
#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;
#[path = "../../render-loop/tests/white_engine/mod.rs"]
mod white_engine;
#[path = "../../render-loop/tests/white_rig/mod.rs"]
mod white_rig;

#[path = "white_end_to_end/fake_source.rs"]
mod fake_source;
#[path = "white_end_to_end/follow.rs"]
mod follow;
#[path = "white_end_to_end/resume_rule.rs"]
mod resume_rule;
#[path = "white_end_to_end/stand.rs"]
mod stand;

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use camera_contract::auto_exposure::CameraTag;
use camera_contract::control::WhiteBalanceSetting;
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::white::{
    SceneIlluminant, WhiteAnswer, WhiteConfidence, WhiteOrigin, WhiteSourceFailure, WhiteSourceId, WhiteSourceOutcome,
    WhiteTransition, WhiteUnavailableReason, WhiteUnknownReason,
};
use camera_core::three_a_status::AdjustmentReason;
use camera_core::white::{WhiteMemoryStartup, WhiteRequestOutcome, WhiteRequestState};
use fake_source::{FakeSourceControl, Reaction, registry_with};
use fasticamera::app_directory::AppDirectory;
use fasticamera::white_memory::{WhiteMemoryConfig, WhiteMemoryHandoff, open_white_memory};
use render_loop::config::WhiteBalanceConfig;
use render_loop::viewfinder::WhiteSourceRegistry;
use scene_estimation::white_source::WhiteRequestRefusal;
use stand::{LIGHT_CHANGES_AT, Stand, StandOptions};
use white_engine::{held_setting, lamp_outcome, setting_of};
use white_rig::{FrameImage, rig_or_skip, scene_with_light_change};

const PHONE: WhiteSourceId = WhiteSourceId::PHONE;
/// Запасной свет движка: дневной, 5600 К.
const FALLBACK_KELVIN: f64 = 5600.0;

/// Подсказки телефона «разгулялись» с кадра `first_wild_frame`: на картинку влиять не должны.
fn wild_hints_after(first_wild_frame: i64) -> impl Fn(i64) -> CaptureColorHints {
    move |frame| {
        if frame < first_wild_frame {
            CaptureColorHints::default()
        } else {
            awb_support::phone_hints_for_white(awb_support::planckian_white(2000.0 + 400.0 * (frame % 12) as f64))
        }
    }
}

fn lamp_after(frames: u32, kelvin: f64) -> Reaction {
    Reaction::Answer {
        outcome: lamp_outcome(kelvin),
        after_frames: frames,
    }
}

/// Эталонный прогон: белая названа с первого кадра, заявок нет.
fn reference_images(setting: WhiteBalanceSetting, frames: i64) -> Vec<FrameImage> {
    let (mut rig, journal) = rig_or_skip(
        scene_with_light_change(200, LIGHT_CHANGES_AT),
        held_setting(setting),
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    )
    .expect("GPU есть (стенд уже поднимался)");
    let images = rig.drive(0..frames, &wild_hints_after(10), &mut |_| {});
    rig.finish(&journal);
    images
}

fn first_shown(images: &[FrameImage]) -> usize {
    images.iter().position(Option::is_some).expect("картинка показана")
}

/// Все кадры в `from..to` показаны и побитово равны эталону.
fn assert_same(actual: &[FrameImage], expected: &[FrameImage], from: usize, to: usize, what: &str) {
    for index in from..to {
        assert!(actual[index].is_some(), "{what}: кадр {index} показан");
        assert_eq!(
            actual[index], expected[index],
            "{what}: кадр {index} побитово как у эталона"
        );
    }
}

fn kelvin_of(status: &camera_core::CameraStatus) -> f64 {
    status.three_a.white.held.setting.temperature.kelvin()
}

fn last_outcome(status: &camera_core::CameraStatus) -> Option<WhiteRequestOutcome> {
    match status.three_a.white.request {
        WhiteRequestState::Idle { last } => last.map(|result| result.result),
        WhiteRequestState::Waiting { .. } => None,
    }
}

/// Стенд с одним фальшивым источником под именем «телефон» (его спрашивает правило открытия).
fn stand_with_fake_phone(reaction: Reaction) -> Option<(Stand, std::sync::Arc<FakeSourceControl>)> {
    let source = FakeSourceControl::new(reaction);
    let stand = Stand::start(StandOptions {
        // Единственная строка, которой источник встаёт в систему.
        sources: registry_with(PHONE, &source),
        memory: WhiteMemoryHandoff::default(),
    })?;
    Some((stand, source))
}

// --- Открытие ---

#[test]
fn the_lamp_the_source_names_at_opening_is_the_picture_and_nothing_moves_it_afterwards() {
    let Some((mut stand, source)) = stand_with_fake_phone(lamp_after(3, 2800.0)) else {
        return;
    };
    stand.resume();
    let mut images = stand.run_frames(40, &wild_hints_after(10));
    // Заявка закрыта, белая — лампа от источника «телефон».
    let status = stand.status();
    assert_eq!(status.three_a.white.held.origin, WhiteOrigin::Source(PHONE));
    assert!(
        (kelvin_of(&status) - 2800.0).abs() < 1e-6,
        "{:?}",
        status.three_a.white.held
    );
    assert!(matches!(
        last_outcome(&status),
        Some(WhiteRequestOutcome::Applied { .. })
    ));

    // Источник теперь говорит другое, свет сцены сменился (кадр 20), подсказки телефона гуляют: белая та же.
    source.react_with(lamp_after(0, 9000.0));
    images.extend(stand.run_frames(20, &wild_hints_after(10)));

    let reference = reference_images(setting_of(2800.0), 60);
    assert_ne!(
        reference[LIGHT_CHANGES_AT as usize - 1],
        reference[LIGHT_CHANGES_AT as usize + 1],
        "свет сцены в самом деле сменился: картинка при той же белой другая"
    );
    let first = first_shown(&images);
    assert!(
        (3..=8).contains(&first),
        "до ответа кадры скрыты, после показаны: {first}"
    );
    assert!(images[..first].iter().all(Option::is_none));
    assert_same(&images, &reference, first, 60, "лампа от источника");
    let applied = stand
        .rig
        .viewfinder()
        .white_balance
        .applied_white
        .expect("рендер держит белую");
    assert!((applied.temperature_kelvin - 2800.0).abs() < 1.0, "{applied:?}");
    assert_eq!(
        source.requests().len(),
        1,
        "источник спрошен ровно один раз, без заявки не спрашивается"
    );
    stand.finish();
}

// --- «Взять у источника» ---

/// Стенд, у которого после открытия держится лампа 2800 К, кадры 0..`FRAMES_BEFORE` показаны.
const FRAMES_BEFORE: i64 = 12;

#[test]
fn take_white_ready_moves_the_picture_to_the_new_white_and_every_refusal_leaves_it_exactly_as_it_was() {
    let reference_old = reference_images(setting_of(2800.0), 40);
    let reference_new = reference_images(setting_of(4500.0), 40);

    // Готово: белая новая, картинка — эталон новой белой.
    {
        let Some((mut stand, source)) = stand_with_fake_phone(lamp_after(2, 2800.0)) else {
            return;
        };
        stand.resume();
        let mut images = stand.run_frames(FRAMES_BEFORE, &wild_hints_after(10));
        source.react_with(lamp_after(2, 4500.0));
        stand
            .handle()
            .take_white(PHONE, WhiteTransition::Immediate)
            .expect("команда принята");
        stand.wait_status("заявка поставлена", |status| {
            matches!(status.three_a.white.request, WhiteRequestState::Waiting { .. })
        });
        images.extend(stand.run_frames(12, &wild_hints_after(10)));
        let status = stand.status();
        assert!(
            (kelvin_of(&status) - 4500.0).abs() < 1e-6,
            "{:?}",
            status.three_a.white.held
        );
        assert!(matches!(
            last_outcome(&status),
            Some(WhiteRequestOutcome::Applied { .. })
        ));

        let first = first_shown(&images);
        // Кадры идут видимыми всё время (KeepShowing): белая меняется на лету, один раз, и дальше держится.
        assert!(images.iter().skip(first).all(Option::is_some), "картинка не скрывалась");
        let switch = (first..images.len())
            .find(|&index| images[index] != reference_old[index])
            .expect("картинка сменилась на новую белую");
        assert!(
            switch > usize::try_from(FRAMES_BEFORE).expect("кадры"),
            "смена после команды: {switch}"
        );
        assert_same(&images, &reference_old, first, switch, "до ответа — прежняя белая");
        assert_same(
            &images,
            &reference_new,
            switch,
            images.len(),
            "после ответа — новая белая",
        );
        stand.finish();
    }

    // Отказы: белая и пиксели прежние, исход различим в статусе.
    let cases: [(&str, Reaction, WhiteRequestOutcome); 4] = [
        (
            "не знаю",
            Reaction::Answer {
                outcome: WhiteSourceOutcome::Unknown {
                    reason: WhiteUnknownReason::SceneNotMeasurable,
                },
                after_frames: 1,
            },
            WhiteRequestOutcome::Unknown {
                reason: WhiteUnknownReason::SceneNotMeasurable,
            },
        ),
        (
            "занят",
            Reaction::Refuse(WhiteRequestRefusal::Busy),
            WhiteRequestOutcome::Busy,
        ),
        (
            "источника нет",
            Reaction::Refuse(WhiteRequestRefusal::Unavailable(
                WhiteUnavailableReason::AutomationSilent,
            )),
            WhiteRequestOutcome::Unavailable {
                reason: WhiteUnavailableReason::AutomationSilent,
            },
        ),
        (
            "сломался",
            Reaction::Answer {
                outcome: WhiteSourceOutcome::Failed {
                    failure: WhiteSourceFailure::Executor,
                },
                after_frames: 1,
            },
            WhiteRequestOutcome::Failed {
                failure: WhiteSourceFailure::Executor,
            },
        ),
    ];
    for (name, reaction, expected) in cases {
        let Some((mut stand, source)) = stand_with_fake_phone(lamp_after(2, 2800.0)) else {
            return;
        };
        stand.resume();
        let mut images = stand.run_frames(FRAMES_BEFORE, &wild_hints_after(10));
        source.react_with(reaction);
        stand
            .handle()
            .take_white(PHONE, WhiteTransition::Immediate)
            .expect("команда принята");
        images.extend(stand.run_frames(10, &wild_hints_after(10)));

        let status = stand.status();
        assert_eq!(
            last_outcome(&status),
            Some(expected),
            "{name}: исход различим в статусе"
        );
        assert!((kelvin_of(&status) - 2800.0).abs() < 1e-6, "{name}: белая прежняя");
        assert_eq!(status.three_a.white.held.origin, WhiteOrigin::Source(PHONE), "{name}");
        let first = first_shown(&images);
        assert_same(&images, &reference_old, first, images.len(), name);
        stand.finish();
    }
}

// --- Рука сильнее ---

#[test]
fn a_manual_white_during_the_wait_wins_and_the_late_answer_is_dropped() {
    let Some((mut stand, source)) = stand_with_fake_phone(Reaction::Silent) else {
        return;
    };
    stand.resume();
    // Источник молчит: заявка ждёт, кадры скрыты.
    let mut images = stand.run_frames(3, &wild_hints_after(10));
    assert!(images.iter().all(Option::is_none), "до ответа кадры скрыты");
    let WhiteRequestState::Waiting {
        request: old_request, ..
    } = stand.status().three_a.white.request
    else {
        panic!("ждём ответа: {:?}", stand.status().three_a.white.request);
    };

    let manual = setting_of(3300.0);
    stand
        .handle()
        .set_white(manual, WhiteTransition::Immediate)
        .expect("команда принята");
    stand.wait_status("рука поставила белую", |status| {
        status.three_a.white.held.origin == WhiteOrigin::Manual
    });

    // Поздний ответ на ПРЕЖНЮЮ заявку (другой свет) приходит в общий ящик: владелец узнаёт его по номеру и отбрасывает.
    let (_, delivery) = stand.answers.publish(WhiteAnswer {
        request: old_request,
        camera: CameraTag::of(&camera_id("2")),
        measured_frame: None,
        outcome: WhiteSourceOutcome::Ready {
            illuminant: SceneIlluminant::Uniform(setting_of(9000.0)),
            confidence: WhiteConfidence::FULL,
        },
    });
    assert_eq!(
        delivery,
        camera_contract::auto_exposure::SignalDelivery::Delivered,
        "толчок привязан"
    );
    let status = stand.wait_status("поздний ответ признан чужим", |status| {
        status.three_a.white.stale_answers >= 1
    });
    assert_eq!(status.three_a.white.stale_answers, 1);
    assert_eq!(status.three_a.white.held.origin, WhiteOrigin::Manual);
    assert!((kelvin_of(&status) - 3300.0).abs() < 1e-6, "белая осталась ручной");
    assert_eq!(
        last_outcome(&status),
        Some(WhiteRequestOutcome::Cancelled {
            by: camera_core::white::WhiteCancelCause::ManualWhite
        }),
        "заявка закрыта рукой"
    );

    images.extend(stand.run_frames(10, &wild_hints_after(10)));
    // Рендер узнаёт об отмене из следующего указания (кадр) и передаёт её источнику.
    assert_eq!(source.cancels(), vec![old_request], "источнику сказано забыть заявку");
    let reference = reference_images(manual, 13);
    assert_same(&images, &reference, first_shown(&images), 13, "пиксели ручные");
    stand.finish();
}

// --- Возобновление ---

#[test]
fn resuming_takes_the_phone_white_anew_but_keeps_a_manual_one() {
    let reference_a = reference_images(setting_of(2800.0), 60);
    let reference_b = reference_images(setting_of(4500.0), 60);
    let reference_manual = reference_images(setting_of(3300.0), 60);

    // «Телефон»: белая взята заново при каждом возобновлении (WB22).
    {
        let Some((mut stand, source)) = stand_with_fake_phone(lamp_after(2, 2800.0)) else {
            return;
        };
        stand.resume();
        let mut images = stand.run_frames(15, &wild_hints_after(10));
        assert!((kelvin_of(&stand.status()) - 2800.0).abs() < 1e-6);
        source.react_with(lamp_after(2, 4500.0));
        stand.pause();
        stand.resume();
        images.extend(stand.run_frames(15, &wild_hints_after(10)));

        let status = stand.status();
        assert!(
            (kelvin_of(&status) - 4500.0).abs() < 1e-6,
            "взято заново: {:?}",
            status.three_a.white
        );
        assert_eq!(
            source.requests().len(),
            2,
            "источник спрошен при открытии и при возобновлении"
        );
        let first = first_shown(&images);
        assert_same(&images, &reference_a, first, 15, "до ухода в фон");
        let second_first = 15 + first_shown(&images[15..]);
        assert!(
            images[15..second_first].iter().all(Option::is_none),
            "после возврата кадры снова скрыты до ответа"
        );
        assert_same(&images, &reference_b, second_first, 30, "после возврата — новая белая");
        stand.finish();
    }

    // «Ручное»: держится, источник не спрашивается, кадры не скрываются.
    {
        let Some((mut stand, source)) = stand_with_fake_phone(lamp_after(2, 2800.0)) else {
            return;
        };
        stand.resume();
        let _ = stand.run_frames(8, &wild_hints_after(10));
        stand
            .handle()
            .set_white(setting_of(3300.0), WhiteTransition::Immediate)
            .expect("команда принята");
        stand.wait_status("рука поставила белую", |status| {
            status.three_a.white.held.origin == WhiteOrigin::Manual
        });
        let asked_before = source.requests().len();
        let mut images = stand.run_frames(4, &wild_hints_after(10));
        source.react_with(lamp_after(0, 7000.0));
        stand.pause();
        stand.resume();
        images.extend(stand.run_frames(10, &wild_hints_after(10)));

        let status = stand.status();
        assert_eq!(status.three_a.white.held.origin, WhiteOrigin::Manual);
        assert!((kelvin_of(&status) - 3300.0).abs() < 1e-6);
        assert_eq!(source.requests().len(), asked_before, "ручное источник не спрашивает");
        assert!(images.iter().all(Option::is_some), "после возврата кадры не скрываются");
        // Кадры хранятся с индекса 8 сцены (первые 8 кадров прошли до ручной команды).
        let from = images.len() - 10;
        for (offset, image) in images.iter().enumerate().skip(from) {
            assert_eq!(
                image.as_ref(),
                reference_manual[8 + offset].as_ref(),
                "кадр {offset} после возврата — ручная белая"
            );
        }
        stand.finish();
    }
}

// --- Перезапуск процесса: память белой из настоящего файла ---

static NEXT_DIRECTORY: AtomicU32 = AtomicU32::new(0);

fn app_directory() -> (AppDirectory, PathBuf) {
    let path = std::env::temp_dir().join(format!(
        "fasticamera-white-e2e-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("папка создана");
    (
        AppDirectory::new(path.to_string_lossy().into_owned()).expect("папка годится"),
        path,
    )
}

fn stand_with_memory(
    directory: &AppDirectory,
    reaction: Reaction,
) -> Option<(Stand, std::sync::Arc<FakeSourceControl>, WhiteMemoryHandoff)> {
    let source = FakeSourceControl::new(reaction);
    let memory = WhiteMemoryHandoff::holding(open_white_memory(directory, &WhiteMemoryConfig::default()));
    let stand = Stand::start(StandOptions {
        sources: registry_with(PHONE, &source),
        memory: memory.clone(),
    })?;
    Some((stand, source, memory))
}

#[test]
fn a_manual_white_survives_a_process_restart_through_the_real_file() {
    let (directory, path) = app_directory();
    let reference = reference_images(setting_of(3300.0), 12);

    // Первый «процесс»: открытие берёт лампу у источника, потом рука ставит 3300 К.
    {
        let Some((mut stand, _source, memory)) = stand_with_memory(&directory, lamp_after(2, 2800.0)) else {
            return;
        };
        stand.resume();
        let _ = stand.run_frames(8, &wild_hints_after(10));
        stand
            .handle()
            .set_white(setting_of(3300.0), WhiteTransition::Immediate)
            .expect("команда принята");
        stand.wait_status("рука поставила белую", |status| {
            status.three_a.white.held.origin == WhiteOrigin::Manual
        });
        Stand::wait_until("ручная белая записана на диск", || {
            std::fs::read_to_string(path.join("white.json")).is_ok_and(|text| text.contains("manual"))
        });
        assert_eq!(memory.diagnostics().snapshot().failed, 0);
        stand.finish();
    }

    // Второй «процесс»: новый движок, память из того же файла. Источник теперь ответил бы другое, но не спрашивается.
    let Some((mut stand, source, _memory)) = stand_with_memory(&directory, lamp_after(0, 7000.0)) else {
        return;
    };
    // Движок публикует первый статус уже с памятью: ждём его (до этого в статусе стартовое значение).
    let before = stand
        .wait_status("память применена при старте", |status| {
            status.three_a.white.held.origin == WhiteOrigin::Manual
        })
        .three_a
        .white;
    assert_eq!(before.memory.startup, WhiteMemoryStartup::Restored);
    assert_eq!(before.held.origin, WhiteOrigin::Manual);
    assert!((before.held.setting.temperature.kelvin() - 3300.0).abs() < 1e-6);
    stand.resume();
    let images = stand.run_frames(12, &wild_hints_after(10));
    assert!(source.requests().is_empty(), "ручное держится: источник не спрашивали");
    assert_same(
        &images,
        &reference,
        0,
        12,
        "ручная белая после перезапуска с первого кадра",
    );
    stand.finish();
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_corrupt_memory_file_falls_back_to_the_daylight_without_crashing_and_the_error_is_visible() {
    let (directory, path) = app_directory();
    std::fs::write(path.join("white.json"), b"{\"schema_version\":1,\"kelvin\":").expect("битый файл записан");
    let reference = reference_images(setting_of(FALLBACK_KELVIN), 12);

    // Источник «не знает»: белая остаётся запасной (дневной свет из конфига).
    let Some((mut stand, _source, _memory)) = stand_with_memory(
        &directory,
        Reaction::Answer {
            outcome: WhiteSourceOutcome::Unknown {
                reason: WhiteUnknownReason::NoFreshMeasurement,
            },
            after_frames: 1,
        },
    ) else {
        return;
    };
    stand.resume();
    let images = stand.run_frames(12, &wild_hints_after(10));

    let white = stand.status().three_a.white;
    assert_eq!(white.held.origin, WhiteOrigin::Fallback);
    assert!((white.held.setting.temperature.kelvin() - FALLBACK_KELVIN).abs() < 1e-6);
    match &white.memory.startup {
        WhiteMemoryStartup::Unreadable { reason } => assert!(reason.contains(".corrupt-"), "{reason}"),
        other => panic!("ошибка памяти должна быть видна в статусе: {other:?}"),
    }
    assert_same(&images, &reference, first_shown(&images), 12, "запасной свет 5600 К");
    assert!(!path.join("white.json").exists(), "битый файл убран в карантин");
    stand.finish();
    std::fs::remove_dir_all(path).expect("уборка");
}

// --- Пределы ---

#[test]
fn a_white_outside_the_limits_is_clamped_visibly_and_the_picture_is_the_clamped_white() {
    let Some((mut stand, _source)) = stand_with_fake_phone(Reaction::Answer {
        outcome: WhiteSourceOutcome::Unknown {
            reason: WhiteUnknownReason::NoFreshMeasurement,
        },
        after_frames: 1,
    }) else {
        return;
    };
    stand.resume();
    let mut images = stand.run_frames(6, &wild_hints_after(10));
    stand
        .handle()
        .set_white(setting_of(15_000.0), WhiteTransition::Immediate)
        .expect("команда принята");
    let status = stand.wait_status("предел применён", |status| {
        status.three_a.white.held.origin == WhiteOrigin::Manual
    });

    assert!((kelvin_of(&status) - 12_000.0).abs() < 1e-6, "применено 12000 К");
    let limit = status
        .three_a
        .white
        .limits
        .iter()
        .find(|adjustment| adjustment.reason == AdjustmentReason::AboveWhiteLimit)
        .expect("прижим виден вызывающему");
    assert_eq!((limit.requested, limit.applied), (15_000.0, 12_000.0));

    images.extend(stand.run_frames(8, &wild_hints_after(10)));
    let reference = reference_images(setting_of(12_000.0), 14);
    assert_same(&images, &reference, 8, 14, "прижатая белая 12000 К");
    stand.finish();
}

// --- Настоящий «телефон» ---

#[test]
fn the_real_phone_source_skips_the_stale_hints_of_the_past_light_and_takes_the_fresh_one() {
    // Первые 5 кадров автоматика несёт белую прошлого сеанса (6500 К), потом мерит лампу 2800 К.
    let lamp = awb_support::planckian_white(2800.0);
    let past = awb_support::planckian_white(6500.0);
    let hints = move |frame: i64| {
        if frame >= 20 {
            // После открытия подсказки гуляют: на картинку не влияют.
            return awb_support::phone_hints_for_white(awb_support::planckian_white(
                2000.0 + 300.0 * (frame % 9) as f64,
            ));
        }
        awb_support::phone_hints_at_frame(frame, 5, past, lamp)
    };
    let Some(mut stand) = Stand::start(StandOptions {
        sources: WhiteSourceRegistry::with_phone(WhiteBalanceConfig::default().phone).expect("числа верны"),
        memory: WhiteMemoryHandoff::default(),
    }) else {
        return;
    };
    stand.resume();
    let images = stand.run_frames(40, &hints);

    let status = stand.status();
    assert_eq!(status.three_a.white.held.origin, WhiteOrigin::Source(PHONE));
    let taken = status.three_a.white.held.setting;
    assert!(
        (taken.temperature.kelvin() - 2800.0).abs() < 120.0,
        "взята свежая лампа, а не прошлый свет 6500 К: {taken:?}"
    );
    assert!(matches!(
        last_outcome(&status),
        Some(WhiteRequestOutcome::Applied { .. })
    ));
    let first = first_shown(&images);
    assert!(first >= 6, "ответ не раньше устоявшегося свежего значения: {first}");
    assert!(first < 20, "в пределе скрытия: {first}");
    let reference = reference_images(taken, 40);
    assert_same(&images, &reference, first, 40, "белая от телефона");
    stand.finish();
}
