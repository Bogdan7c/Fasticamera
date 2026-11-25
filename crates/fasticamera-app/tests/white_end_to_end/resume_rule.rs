//! Правило возобновления белого (WB36) сквозь настоящий движок и настоящий показ, с пикселями.
//!
//! `keep_last`: после ответа телефона белая не меняется ни при паузе и возврате, ни после перезапуска процесса;
//! кадры не скрываются, источник не спрашивается, картинка побитово равна эталону той же белой. Кнопка «Phone»
//! (команда `TakeWhite`) работает и под `keep_last`. Выбор правила приходит так же, как в продукте: параметром
//! запуска `camera.white_resume_choice` (команда движку до первого возобновления) или командой на ходу.

use camera_core::white::{WhiteMemoryStartup, WhiteResumeRule};
use fasticamera::camera_config::CameraConfig;
use fasticamera::white_memory::{WhiteMemoryConfig, WhiteMemoryHandoff, open_white_memory};

use super::fake_source::{FakeSourceControl, Reaction, registry_with};
use super::stand::{Stand, StandOptions};
use super::*;

/// Конфиг камеры с выбором человека: как будто приложение запущено с параметром `white_resume_rule`.
fn config_choosing(rule: WhiteResumeRule) -> CameraConfig {
    CameraConfig {
        white_resume_choice: Some(rule),
        ..CameraConfig::default()
    }
}

fn stand_with(
    reaction: Reaction,
    config: CameraConfig,
    memory: WhiteMemoryHandoff,
) -> Option<(Stand, std::sync::Arc<FakeSourceControl>)> {
    let source = FakeSourceControl::new(reaction);
    let stand = Stand::start_with_config(
        StandOptions {
            sources: registry_with(PHONE, &source),
            memory,
        },
        config,
    )?;
    Some((stand, source))
}

#[test]
fn keep_last_returning_from_the_background_keeps_the_exact_picture_hides_nothing_and_asks_nobody() {
    let reference_phone = reference_images(setting_of(2800.0), 40);
    let Some((mut stand, source)) = stand_with(
        lamp_after(2, 2800.0),
        config_choosing(WhiteResumeRule::KeepLast),
        WhiteMemoryHandoff::default(),
    ) else {
        return;
    };

    // Самый первый запуск: помнить нечего, белая запасная — телефон спрошен один раз, кадры скрыты до ответа (WB38).
    stand.resume();
    let mut images = stand.run_frames(15, &wild_hints_after(10));
    let first = first_shown(&images);
    assert!(images[..first].iter().all(Option::is_none), "до ответа кадры скрыты");
    assert_eq!(
        stand.status().three_a.white.resume_rule,
        WhiteResumeRule::KeepLast,
        "выбор параметра запуска дошёл до владельца"
    );
    let held_before = stand.status().three_a.white.held;
    assert_eq!(held_before.origin, WhiteOrigin::Source(PHONE));

    // Свет у телефона теперь другой (и подсказки гуляют): под keep_last это не должно ничего значить.
    source.react_with(lamp_after(0, 7000.0));
    stand.pause();
    stand.resume();
    images.extend(stand.run_frames(15, &wild_hints_after(10)));

    let status = stand.status();
    assert_eq!(
        status.three_a.white.held, held_before,
        "белая та же бит-в-бит, с тем же происхождением"
    );
    assert_eq!(source.requests().len(), 1, "после ответа источник не спрошен ни разу");
    assert!(
        matches!(status.three_a.white.request, WhiteRequestState::Idle { .. }),
        "заявки нет: {:?}",
        status.three_a.white.request
    );
    assert!(
        images[15..].iter().all(Option::is_some),
        "после возврата ни один кадр не скрыт"
    );
    assert_same(
        &images,
        &reference_phone,
        first,
        30,
        "до паузы и после возврата — одна и та же картинка",
    );
    stand.finish();
}

#[test]
fn keep_last_still_lets_the_phone_button_take_a_new_white() {
    let reference_old = reference_images(setting_of(2800.0), 40);
    let reference_new = reference_images(setting_of(4500.0), 40);
    let Some((mut stand, source)) = stand_with(
        lamp_after(2, 2800.0),
        config_choosing(WhiteResumeRule::KeepLast),
        WhiteMemoryHandoff::default(),
    ) else {
        return;
    };
    stand.resume();
    let mut images = stand.run_frames(14, &wild_hints_after(10));
    stand.pause();
    stand.resume();
    images.extend(stand.run_frames(4, &wild_hints_after(10)));
    assert_eq!(source.requests().len(), 1, "возврат источник не спрашивал");

    // Тап «Phone»: белая взята по просьбе, картинка переехала на неё.
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
    assert_eq!(status.three_a.white.held.origin, WhiteOrigin::Source(PHONE));
    assert_eq!(source.requests().len(), 2, "второй вопрос — именно просьба человека");
    let first = first_shown(&images);
    let switch = (first..images.len())
        .find(|&index| images[index] != reference_old[index])
        .expect("картинка сменилась на новую белую");
    assert!(switch >= 18, "смена только после просьбы: {switch}");
    assert_same(&images, &reference_old, first, switch, "до просьбы — прежняя белая");
    assert_same(
        &images,
        &reference_new,
        switch,
        images.len(),
        "после просьбы — новая белая",
    );
    stand.finish();
}

#[test]
fn choosing_keep_last_on_the_fly_takes_effect_on_the_next_return_and_not_before() {
    let reference = reference_images(setting_of(2800.0), 40);
    // Параметра запуска нет, памяти нет: действует умолчание конфига take_from_source.
    let Some((mut stand, source)) = stand_with(
        lamp_after(2, 2800.0),
        CameraConfig::default(),
        WhiteMemoryHandoff::default(),
    ) else {
        return;
    };
    stand.resume();
    let mut images = stand.run_frames(14, &wild_hints_after(10));
    assert_eq!(
        stand.status().three_a.white.resume_rule,
        WhiteResumeRule::TakeFromSource
    );

    // Команда белую не трогает: картинка и белая те же сразу после неё.
    let held = stand.status().three_a.white.held;
    stand
        .handle()
        .set_white_resume_rule(WhiteResumeRule::KeepLast)
        .expect("команда принята");
    stand.wait_status("выбор принят", |status| {
        status.three_a.white.resume_rule == WhiteResumeRule::KeepLast
    });
    assert_eq!(stand.status().three_a.white.held, held);
    images.extend(stand.run_frames(2, &wild_hints_after(10)));

    // Возврат: теперь правило другое. Источник сказал бы 9000, но не спрошен.
    source.react_with(lamp_after(0, 9000.0));
    stand.pause();
    stand.resume();
    images.extend(stand.run_frames(14, &wild_hints_after(10)));

    assert_eq!(stand.status().three_a.white.held, held);
    assert_eq!(source.requests().len(), 1);
    let first = first_shown(&images);
    assert!(
        images[16..].iter().all(Option::is_some),
        "кадры после возврата не скрыты"
    );
    assert_same(&images, &reference, first, 30, "картинка не менялась");
    stand.finish();
}

#[test]
fn keep_last_and_the_phone_white_survive_a_process_restart_through_the_real_file() {
    let (directory, path) = app_directory();
    let reference = reference_images(setting_of(2800.0), 12);

    // Первый «процесс»: человек выбрал keep_last (параметр запуска), телефон дал 2800 К.
    {
        let memory = WhiteMemoryHandoff::holding(open_white_memory(&directory, &WhiteMemoryConfig::default()));
        let Some((mut stand, _source)) = stand_with(
            lamp_after(2, 2800.0),
            config_choosing(WhiteResumeRule::KeepLast),
            memory.clone(),
        ) else {
            return;
        };
        stand.resume();
        let _ = stand.run_frames(8, &wild_hints_after(10));
        Stand::wait_until(
            "белая и выбор правила записаны на диск",
            || {
                std::fs::read_to_string(path.join("white.json"))
                    .is_ok_and(|text| text.contains("\"keep_last\"") && text.contains("\"source\""))
            },
        );
        assert_eq!(memory.diagnostics().snapshot().failed, 0);
        stand.finish();
    }

    // Второй «процесс»: параметра запуска нет, конфиг по умолчанию — take_from_source, но выбор человека из файла
    // сильнее. Телефон ответил бы 7000 К, однако не спрашивается: картинка с первого кадра та же.
    let memory = WhiteMemoryHandoff::holding(open_white_memory(&directory, &WhiteMemoryConfig::default()));
    let Some((mut stand, source)) = stand_with(lamp_after(0, 7000.0), CameraConfig::default(), memory) else {
        return;
    };
    let before = stand
        .wait_status("память применена при старте", |status| {
            status.three_a.white.held.origin == WhiteOrigin::Source(PHONE)
        })
        .three_a
        .white;
    assert_eq!(before.memory.startup, WhiteMemoryStartup::Restored);
    assert_eq!(
        before.resume_rule,
        WhiteResumeRule::KeepLast,
        "правило из файла сильнее конфига"
    );
    assert!((before.held.setting.temperature.kelvin() - 2800.0).abs() < 1e-6);
    stand.resume();
    let images = stand.run_frames(12, &wild_hints_after(10));

    assert!(source.requests().is_empty(), "источник не спрашивали");
    assert!(images.iter().all(Option::is_some), "ни один кадр не скрыт");
    assert_same(
        &images,
        &reference,
        0,
        12,
        "та же картинка с первого кадра после перезапуска",
    );
    assert_eq!(stand.status().three_a.white.held, before.held, "белая бит-в-бит");
    stand.finish();
    std::fs::remove_dir_all(path).expect("уборка");
}
