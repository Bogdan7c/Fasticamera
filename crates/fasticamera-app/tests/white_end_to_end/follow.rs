//! Слежение за белым телефона (WB44–WB56) сквозь настоящий движок, НАСТОЯЩИЙ источник «телефон» и настоящий показ, с
//! пикселями. Свет задают подсказки автоматики на кадрах, остальное — продуктовый путь: кадр → результат в движок →
//! вопрос слежения → источник → ответ → указание → переезд по меткам сенсора → ISP.
//!
//! Эталон — прогон с той же белой, названной с первого кадра: «стоит» и «доехала» значит «побитово равна эталону».

use camera_contract::ids::LensRole;
use camera_contract::three_a::SensorPoint;
use camera_core::white::{WhiteFollowSpeed, WhiteFollowState, WhiteResumeRule};
use fasticamera::camera_config::CameraConfig;

use super::awb_support::{phone_hints_at_frame, phone_hints_for_white, planckian_white};
use super::stand::{Stand, StandOptions};
use super::white_rig::second_lens_passport;
use super::*;

/// Кадров между вопросами слежения (100 мс по меткам 60 к/с).
const ASK_EVERY_FRAMES: usize = 6;
/// Запас на путь «вопрос → ответ → указание → кадр»: каждый шаг занимает не больше кадра.
const ROUND_TRIP_FRAMES: usize = 4;
/// К этому кадру открытие давно закрыто (ответ — к 10–12-му), слежение включаем здесь.
const FOLLOW_FROM: i64 = 20;
const LAMP: f64 = 2_800.0;
const DAYLIGHT: f64 = 5_600.0;

/// Переезд по умолчанию, 0,3 с, в кадрах.
const DEFAULT_MOVE_FRAMES: usize = 18;

fn phone_sources() -> WhiteSourceRegistry {
    WhiteSourceRegistry::with_phone(WhiteBalanceConfig::default().phone).expect("числа верны")
}

fn stand_with(config: CameraConfig, memory: WhiteMemoryHandoff) -> Option<Stand> {
    Stand::start_with_config(
        StandOptions {
            sources: phone_sources(),
            memory,
        },
        config,
    )
}

fn stand() -> Option<Stand> {
    stand_with(CameraConfig::default(), WhiteMemoryHandoff::default())
}

/// Подсказки: первые пять кадров — белая прошлого сеанса, дальше свет `light(кадр)` в кельвинах.
fn hints_of(light: impl Fn(i64) -> f64) -> impl Fn(i64) -> CaptureColorHints {
    move |frame| phone_hints_at_frame(frame, 5, planckian_white(6_500.0), planckian_white(light(frame)))
}

/// Свет меняется с лампы на дневной с кадра `at`.
fn lamp_then_daylight(at: i64) -> impl Fn(i64) -> CaptureColorHints {
    hints_of(move |frame| if frame < at { LAMP } else { DAYLIGHT })
}

/// Открытие под лампой до кадра [`FOLLOW_FROM`], потом «Auto» включён.
fn open_and_follow(stand: &mut Stand, hints: &dyn Fn(i64) -> CaptureColorHints) -> Vec<FrameImage> {
    stand.resume();
    let images = stand.run_frames(FOLLOW_FROM, hints);
    let held = stand.status().three_a.white.held;
    assert_eq!(held.origin, WhiteOrigin::Source(PHONE), "открытие взяло белое телефона");
    turn_auto_on(stand);
    images
}

fn turn_auto_on(stand: &Stand) {
    stand.handle().follow_white(PHONE).expect("команда принята");
    stand.wait_status("слежение включено", |status| {
        status.three_a.white.follow.state.is_on()
    });
}

fn lock(stand: &Stand) {
    stand
        .handle()
        .lock_focus_and_exposure_at(SensorPoint::new(0.5, 0.5).expect("центр"))
        .expect("команда принята");
    stand.wait_status("белая заморожена", |status| {
        matches!(status.three_a.white.follow.state, WhiteFollowState::HeldByLock { .. })
    });
}

fn unlock(stand: &Stand) {
    stand.handle().reset_point_of_interest().expect("команда принята");
    stand.wait_status("блокировка снята", |status| {
        matches!(status.three_a.white.follow.state, WhiteFollowState::Following { .. })
    });
}

/// Первый кадр в `from..`, отличный от эталона.
fn first_different(images: &[FrameImage], reference: &[FrameImage], from: usize) -> Option<usize> {
    (from..images.len()).find(|index| images[*index] != reference[*index])
}

/// Свет сменился на кадре `light_at`: белая трогается за время опроса, едет `move_frames` кадров (не короче) и встаёт
/// побитово на белую, которую назвал телефон.
fn assert_moved(
    images: &[FrameImage],
    before: &[FrameImage],
    after: &[FrameImage],
    light_at: usize,
    move_frames: usize,
) {
    let started = first_different(images, before, first_shown(images)).expect("белая поехала");
    assert!(started > light_at, "раньше смены света ехать не с чего: {started}");
    assert!(
        started <= light_at + ASK_EVERY_FRAMES + ROUND_TRIP_FRAMES,
        "тронулась на кадре {started}, свет сменился на {light_at}"
    );
    // Кадр, принявший указание, несёт ещё прежнюю белую: переезд начинается с него.
    let accepted = started - 1;
    for index in started..accepted + move_frames {
        assert_ne!(
            images[index], after[index],
            "кадр {index}: переезд плавный, рано вставать"
        );
        assert_ne!(images[index], before[index], "кадр {index}: уже едет");
    }
    assert_same(images, after, accepted + move_frames, images.len(), "доехала и стоит");
}

fn kelvin_is_near(stand: &Stand, kelvin: f64) -> WhiteBalanceSetting {
    let held = stand.status().three_a.white.held;
    assert_eq!(held.origin, WhiteOrigin::Source(PHONE));
    let gap_mired = (1.0e6 / held.setting.temperature.kelvin() - 1.0e6 / kelvin).abs();
    assert!(gap_mired < 6.0, "белая около {kelvin} К: {held:?}");
    held.setting
}

#[test]
fn when_the_light_changes_the_white_goes_smoothly_to_the_phone_value_in_the_configured_time() {
    let Some(mut stand) = stand() else { return };
    let hints = lamp_then_daylight(40);
    let mut images = open_and_follow(&mut stand, &hints);
    let lamp = kelvin_is_near(&stand, LAMP);
    images.extend(stand.run_frames(60, &hints));
    let daylight = kelvin_is_near(&stand, DAYLIGHT);

    let (before, after) = (reference_images(lamp, 80), reference_images(daylight, 80));
    assert_moved(&images, &before, &after, 40, DEFAULT_MOVE_FRAMES);
    assert_eq!(stand.status().three_a.white.follow.moved, 1, "один свет — один переезд");
    stand.finish();
}

#[test]
fn a_tenth_of_a_second_chosen_by_the_launch_parameter_moves_three_times_faster() {
    let config = CameraConfig {
        white_follow_speed_choice: Some(WhiteFollowSpeed::from_millis(100).expect("0,1 с")),
        ..CameraConfig::default()
    };
    let Some(mut stand) = stand_with(config, WhiteMemoryHandoff::default()) else {
        return;
    };
    let hints = lamp_then_daylight(40);
    let mut images = open_and_follow(&mut stand, &hints);
    let lamp = kelvin_is_near(&stand, LAMP);
    images.extend(stand.run_frames(40, &hints));
    let daylight = kelvin_is_near(&stand, DAYLIGHT);
    let (before, after) = (reference_images(lamp, 60), reference_images(daylight, 60));
    assert_moved(&images, &before, &after, 40, DEFAULT_MOVE_FRAMES / 3);
    stand.finish();
}

#[test]
fn jitter_inside_the_dead_zone_does_not_change_a_single_bit_of_the_picture() {
    let Some(mut stand) = stand() else { return };
    // Дрожь ±2 майреда вокруг лампы, на каждом кадре другая.
    let hints = hints_of(|frame| {
        let jitter_mired = if frame < FOLLOW_FROM {
            0.0
        } else {
            ((frame * 7) % 5 - 2) as f64
        };
        1.0e6 / (1.0e6 / LAMP + jitter_mired)
    });
    let mut images = open_and_follow(&mut stand, &hints);
    let lamp = kelvin_is_near(&stand, LAMP);
    images.extend(stand.run_frames(60, &hints));

    let status = stand.status().three_a.white;
    assert!(
        status.follow.asked >= 8,
        "телефон действительно спрашивали: {}",
        status.follow.asked
    );
    assert_eq!(status.follow.moved, 0);
    assert_eq!(status.held.setting, lamp, "белая бит-в-бит");
    let reference = reference_images(lamp, 80);
    assert_same(&images, &reference, first_shown(&images), 80, "дрожь не видна");
    stand.finish();
}

#[test]
fn the_ae_af_lock_freezes_every_bit_and_releasing_it_continues() {
    let Some(mut stand) = stand() else { return };
    let hints = lamp_then_daylight(40);
    let mut images = open_and_follow(&mut stand, &hints);
    let lamp = kelvin_is_near(&stand, LAMP);
    lock(&stand);
    images.extend(stand.run_frames(60, &hints));
    let before = reference_images(lamp, 120);
    assert_same(
        &images,
        &before,
        first_shown(&images),
        80,
        "под блокировкой свет сменился, картинка нет",
    );

    unlock(&stand);
    images.extend(stand.run_frames(40, &hints));
    let after = reference_images(kelvin_is_near(&stand, DAYLIGHT), 120);
    // Свет для слежения «сменился» в момент снятия блокировки.
    assert_moved(&images, &before, &after, 80, DEFAULT_MOVE_FRAMES);
    stand.finish();
}

#[test]
fn a_move_already_started_finishes_under_the_lock_and_the_next_light_is_ignored() {
    let Some(mut stand) = stand() else { return };
    // Лампа → дневной на 40, под блокировкой свет меняется ещё раз, на тень.
    let hints = hints_of(|frame| match frame {
        ..40 => LAMP,
        40..60 => DAYLIGHT,
        _ => 8_000.0,
    });
    let mut images = open_and_follow(&mut stand, &hints);
    images.extend(stand.run_frames(35, &hints));
    let daylight = kelvin_is_near(&stand, DAYLIGHT);
    lock(&stand);
    images.extend(stand.run_frames(45, &hints));
    assert_eq!(stand.status().three_a.white.held.setting, daylight);
    let after = reference_images(daylight, 100);
    assert_ne!(images[54], after[54], "блокировка встала посреди переезда");
    assert_same(&images, &after, 75, 100, "переезд закончился, нового нет (WB54)");
    stand.finish();
}

#[test]
fn taking_once_turns_auto_off_and_the_picture_stops_following() {
    let Some(mut stand) = stand() else { return };
    let hints = lamp_then_daylight(40);
    let mut images = open_and_follow(&mut stand, &hints);
    images.extend(stand.run_frames(15, &hints));
    stand
        .handle()
        .take_white(PHONE, WhiteTransition::Immediate)
        .expect("команда принята");
    stand.wait_status("слежение выключено", |status| {
        status.three_a.white.follow.state == WhiteFollowState::Off
    });
    images.extend(stand.run_frames(45, &hints));
    let taken = kelvin_is_near(&stand, LAMP);
    let reference = reference_images(taken, 80);
    assert_same(&images, &reference, 39, 80, "взято разово и держится");
    stand.finish();
}

#[test]
fn a_hand_white_under_auto_is_shown_at_once_and_stays() {
    let Some(mut stand) = stand() else { return };
    let hints = lamp_then_daylight(40);
    let mut images = open_and_follow(&mut stand, &hints);
    images.extend(stand.run_frames(30, &hints));
    let hand = setting_of(3_300.0);
    stand
        .handle()
        .set_white(hand, WhiteTransition::Immediate)
        .expect("команда принята");
    stand.wait_status("рука сильнее", |status| {
        status.three_a.white.held.origin == WhiteOrigin::Manual
    });
    images.extend(stand.run_frames(40, &hints));
    assert_same(&images, &reference_images(hand, 90), 51, 90, "ручная белая");
    stand.finish();
}

#[test]
fn returning_under_keep_last_with_auto_hides_until_the_answer_and_shows_the_new_light_at_once() {
    let config = CameraConfig {
        white_resume_choice: Some(WhiteResumeRule::KeepLast),
        ..CameraConfig::default()
    };
    let Some(mut stand) = stand_with(config, WhiteMemoryHandoff::default()) else {
        return;
    };
    // Вернулись в другой свет: первые пять кадров нового потока автоматика повторяет прошлый.
    let hints = hints_of(|frame| if frame < 35 { LAMP } else { DAYLIGHT });
    let mut images = open_and_follow(&mut stand, &hints);
    images.extend(stand.run_frames(10, &hints));
    stand.pause();
    stand.resume();
    images.extend(stand.run_frames(40, &hints));

    let after_return = &images[30..];
    let shown_at = 30 + first_shown(after_return);
    assert!(shown_at > 35, "до ответа кадры скрыты: {shown_at}");
    assert!(shown_at < 30 + 20, "в пределе скрытия: {shown_at}");
    let daylight = kelvin_is_near(&stand, DAYLIGHT);
    assert_same(
        &images,
        &reference_images(daylight, 70),
        shown_at,
        70,
        "первый же показанный кадр — верного цвета",
    );
    stand.finish();
}

#[test]
fn auto_survives_a_process_restart_through_the_real_file_and_follows_again() {
    let (directory, path) = app_directory();
    let open_memory = || WhiteMemoryHandoff::holding(open_white_memory(&directory, &WhiteMemoryConfig::default()));
    {
        let Some(mut stand) = stand_with(CameraConfig::default(), open_memory()) else {
            return;
        };
        let _ = open_and_follow(&mut stand, &hints_of(|_| LAMP));
        Stand::wait_until("выбор слежения записан на диск", || {
            std::fs::read_to_string(path.join("white.json")).is_ok_and(|text| text.contains("follow_source"))
        });
        stand.finish();
    }
    let Some(mut stand) = stand_with(CameraConfig::default(), open_memory()) else {
        return;
    };
    stand.wait_status("слежение вспомнилось", |status| {
        status.three_a.white.follow.state.is_on()
    });
    let hints = lamp_then_daylight(30);
    stand.resume();
    let images = stand.run_frames(80, &hints);
    let daylight = kelvin_is_near(&stand, DAYLIGHT);
    assert_same(
        &images,
        &reference_images(daylight, 80),
        65,
        80,
        "следит и после перезапуска",
    );
    stand.finish();
    std::fs::remove_dir_all(path).expect("уборка");
}

#[test]
fn a_lens_switch_shows_the_held_white_at_once_and_then_follows_the_new_camera() {
    let Some(mut stand) = stand() else { return };
    // Новая линза (кадры с 40) пять кадров повторяет прошлое, потом видит дневной свет.
    let hints = |frame: i64| match frame {
        ..40 => phone_hints_at_frame(frame, 5, planckian_white(6_500.0), planckian_white(LAMP)),
        40..45 => phone_hints_for_white(planckian_white(LAMP)),
        _ => phone_hints_for_white(planckian_white(DAYLIGHT)),
    };
    let mut images = open_and_follow(&mut stand, &hints);
    images.extend(stand.run_frames(20, &hints));
    let lamp = kelvin_is_near(&stand, LAMP);
    stand.handle().switch_to(LensRole::UltraWide).expect("команда принята");
    stand.wait_status("новая линза стримит", |status| {
        status.state.name() == "streaming"
            && status
                .configured
                .as_ref()
                .is_some_and(|configured| configured.stream.camera.as_str() == "3")
    });
    stand.rig.begin_stream(second_lens_passport());
    images.extend(stand.run_frames(50, &hints));
    assert!(
        images[40..].iter().all(Option::is_some),
        "ни один кадр новой линзы не скрыт"
    );

    let held = stand.status().three_a.white.held;
    assert_ne!(held.setting, lamp, "новая камера назвала свой свет");
    let before = reference_after_lens_switch(lamp);
    let after = reference_after_lens_switch(held.setting);
    assert_same(
        &images,
        &before,
        40,
        45,
        "пока новая камера не мерила, белая прежняя (WB52)",
    );
    assert_same(&images, &after, 80, 90, "доехала до белой новой камеры");
    let started = first_different(&images, &before, 40).expect("поехала");
    assert_ne!(images[started + 8], after[started + 8], "доезжает плавно, а не скачком");
    stand.finish();
}

/// Эталон с той же сменой линзы на кадре 40 и белой, названной с начала.
fn reference_after_lens_switch(setting: WhiteBalanceSetting) -> Vec<FrameImage> {
    let (mut rig, journal) = rig_or_skip(
        scene_with_light_change(200, LIGHT_CHANGES_AT),
        held_setting(setting),
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    )
    .expect("GPU есть (стенд уже поднимался)");
    let quiet = |_: i64| CaptureColorHints::default();
    let mut images = rig.drive(0..40, &quiet, &mut |_| {});
    rig.begin_stream(second_lens_passport());
    images.extend(rig.drive(40..90, &quiet, &mut |_| {}));
    rig.finish(&journal);
    images
}
