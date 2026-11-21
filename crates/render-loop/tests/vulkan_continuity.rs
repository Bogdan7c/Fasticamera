//! Непрерывность картинки при смене потока камеры (сессия 13, решения 110 и 113: «0 чёрных кадров») через
//! НАСТОЯЩИЙ бэкенд показа на реальном драйвере (RADV, headless-показ, полная validation) и настоящий
//! `RawFrameStream` с управляемым синтетическим RAW.
//!
//! # Что доказывается и чем
//! * Последовательность меток показанных кадров (`живой → переходный×N → живой`), фон под UI (`backdrop`
//!   запросов источника UI: `Live → HeldLastFrame{since} → Live`), счётчики удержаний и кадрирование каждого
//!   кадра берутся из диагностики бэкенда.
//! * Что именно лежит на экране в переходном кадре, читается обратно из удержанной картинки
//!   (`read_held_image`): она равна ПОСЛЕДНЕМУ показанному кадру потока A, а после показа B живое изображение
//!   равно эталону `isp-model` для B.
//! * Конечные ПИКСЕЛИ экрана (окно, зеркало, увеличение) проверены отдельно, на читаемой offscreen-цели:
//!   swapchain headless-стенда читать нельзя (`pipelines/screen_tests.rs`, `gpu-vulkan/tests/compute_kit.rs`).
//! * Кадрирование по камере кадра и увеличение удержанного кадра — `vulkan_continuity_zoom.rs`; поток,
//!   кадры которого приходят, но не показываются, — `vulkan_continuity_unpresentable.rs`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]

mod continuity_rig;
mod support;
mod vulkan_support;

use std::sync::Arc;

use camera_contract::auto_exposure::{AppliedExposure, ExposureDirective, ExposureProduct, ProductReach};
use continuity_rig::{
    MAIN, Rig, SIZE_A, SIZE_B, WIDE, assert_picture, content_size, dark, finish, gray, passport, reddish,
    reference_color, test_continuity,
};
use render_loop::backend::DisplayBackend;
use render_loop::config::RevealExposureCheck;
use render_loop::viewfinder::{BlackKind, FrameLabel, TransitionKind};
use ui_draw::frame_source::BackdropContinuity;
/// Главный сценарий: A показан → поток закрылся → N тиков без потока → B (другой размер) → B показан.
/// Ни одного чёрного кадра; во время ожидания на экране ровно последний кадр A; потом B.
#[test]
fn switching_streams_holds_the_last_picture_then_reveals_the_new_one_without_a_black_frame() {
    let Some(mut rig) = Rig::new(test_continuity(RevealExposureCheck::Ignore), false) else {
        return;
    };
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let passport_b = Arc::new(passport(WIDE, SIZE_B));
    let (gray_ref, red_ref) = (
        reference_color(&passport_a, gray()),
        reference_color(&passport_b, reddish()),
    );
    assert!(
        (gray_ref[0] - red_ref[0]).abs() + (gray_ref[1] - red_ref[1]).abs() > 0.1,
        "A и B различимы"
    );

    // 1. Поток A показан.
    let epoch_a = rig.begin(&passport_a);
    let a_ticks = rig.feed_until_live(epoch_a, gray());
    assert_picture(&rig.live(), content_size(SIZE_A), gray_ref, "A на экране");
    assert_eq!(
        a_ticks.last().map(|tick| tick.ui_backdrop),
        Some(BackdropContinuity::Live)
    );

    // 2. Поток A закрылся; потока нет, B ещё не пришёл: 6 тиков удержания.
    rig.stream.producer().end_stream(epoch_a);
    let mut held_ticks = Vec::new();
    for _ in 0..6 {
        held_ticks.push(rig.tick());
    }
    let first_held_present = held_ticks[0].record.present_time;
    for (index, tick) in held_ticks.iter().enumerate() {
        assert_eq!(
            tick.record.label,
            FrameLabel::Transition(TransitionKind::HeldLastFrame),
            "тик удержания {index}"
        );
        assert_eq!(
            tick.ui_backdrop,
            BackdropContinuity::HeldLastFrame {
                since: first_held_present
            },
            "UI знает, что фон удержан, и с какого кадра"
        );
        assert!(tick.holding);
        let held = tick.held.as_ref().expect("переходный кадр читается");
        assert_picture(
            held,
            content_size(SIZE_A),
            gray_ref,
            "удержанный кадр = последний кадр A",
        );
    }

    // 3. Поток B стартует: его кадры показываются, как только готовы (цвет известен, яркость не проверяется).
    let epoch_b = rig.begin(&passport_b);
    let b_ticks = rig.feed_until_live(epoch_b, reddish());
    assert_eq!(
        b_ticks.last().map(|tick| (tick.record.label, tick.ui_backdrop)),
        Some((FrameLabel::Live, BackdropContinuity::Live)),
        "удержание закончилось живым кадром B, UI вернулся к Live"
    );
    assert_picture(&rig.live(), content_size(SIZE_B), red_ref, "B на экране");
    assert!(
        rig.backend.read_held_image().expect("readback").is_none(),
        "удержанная картинка освобождена"
    );

    // 4. Итоги: ни одного чёрного кадра, пока картинка была; счётчики сходятся с метками.
    let counters = rig.counters();
    assert_eq!(counters.black_frames_while_picture_expected, 0, "{counters:?}");
    assert_eq!(counters.holds_started, 1, "{counters:?}");
    assert_eq!(counters.holds_revealed_by_condition, 1, "{counters:?}");
    assert_eq!(
        counters.holds_revealed_by_timeout + counters.holds_ended_without_stream,
        0,
        "{counters:?}"
    );
    assert_eq!(counters.held_frames_presented, held_ticks.len() as u64, "{counters:?}");
    assert!(counters.last_hold_duration_ms.is_some_and(|ms| ms >= 6), "{counters:?}");
    let diagnostics = rig.backend.diagnostics().viewfinder.expect("диагностика");
    assert!(
        diagnostics
            .continuity
            .recent_frames
            .iter()
            .all(|frame| !matches!(frame.label, FrameLabel::Black(BlackKind::PictureExpected))),
        "в ряду кадров нет чёрного при ожидаемой картинке"
    );

    finish(&mut rig, &[epoch_b]);
    // Удержанная картинка и картинки ISP не утекли: объектов не больше, чем до теста (плюс живая картинка
    // освобождена при detach вместе с конвейерами).
    assert_eq!(
        rig.backend.context().live_objects(),
        rig.baseline,
        "объекты Vulkan вернулись к исходным"
    );
}

/// Нового потока нет вовсе `max_hold_without_stream_ms`: честный чёрный, UI узнаёт `NoCameraFrame`.
#[test]
fn without_any_new_stream_the_held_picture_ends_in_an_honest_black() {
    let Some(mut rig) = Rig::new(test_continuity(RevealExposureCheck::Ignore), false) else {
        return;
    };
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let epoch_a = rig.begin(&passport_a);
    rig.feed_until_live(epoch_a, gray());
    rig.stream.producer().end_stream(epoch_a);
    let mut labels = Vec::new();
    let mut last = None;
    for _ in 0..80 {
        let tick = rig.tick();
        labels.push(tick.record.label);
        last = Some(tick);
    }
    let first_black = labels
        .iter()
        .position(|label| matches!(label, FrameLabel::Black(_)))
        .expect("честный чёрный наступил");
    assert!(
        labels[..first_black]
            .iter()
            .all(|label| *label == FrameLabel::Transition(TransitionKind::HeldLastFrame)),
        "до чёрного — только удержание: {labels:?}"
    );
    assert!(
        (59..=62).contains(&first_black),
        "чёрный не раньше max_hold_without_stream_ms (60 мс) и не сильно позже: кадр {first_black}"
    );
    assert!(
        labels[first_black..]
            .iter()
            .all(|label| *label == FrameLabel::Black(BlackKind::CameraUnavailable)),
        "после срока — «камера недоступна», а не «ожидалась картинка»: {labels:?}"
    );
    let last = last.expect("тики были");
    assert_eq!(last.ui_backdrop, BackdropContinuity::NoCameraFrame);
    assert!(!last.holding);
    assert!(
        rig.backend.read_held_image().expect("readback").is_none(),
        "удержанная картинка освобождена"
    );
    let counters = rig.counters();
    assert_eq!(counters.holds_ended_without_stream, 1, "{counters:?}");
    assert_eq!(counters.black_frames_while_picture_expected, 0, "{counters:?}");
    assert!(counters.black_frames_camera_unavailable > 0, "{counters:?}");
    finish(&mut rig, &[]);
}

/// Новый поток никогда не станет правдоподобным по яркости (сцена очень тёмная, наш AE меряет, а камера
/// не отвечает): кадры B обрабатываются, но НЕ показываются, пока не вышел `max_hold_ms`; потом показ «как есть».
#[test]
fn a_new_stream_that_never_looks_plausible_is_revealed_by_timeout_and_its_frames_stay_hidden_until_then() {
    let Some(mut rig) = Rig::new(test_continuity(RevealExposureCheck::MeasuredAndPlausible), true) else {
        return;
    };
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let passport_b = Arc::new(passport(WIDE, SIZE_B));
    let gray_ref = reference_color(&passport_a, gray());
    let epoch_a = rig.begin(&passport_a);
    rig.feed_until_live(epoch_a, gray());
    rig.stream.producer().end_stream(epoch_a);
    let _gap = (0..3).map(|_| rig.tick()).collect::<Vec<_>>();
    let processed_before = rig
        .backend
        .diagnostics()
        .viewfinder
        .expect("диагностика")
        .counters
        .frames_processed;

    let epoch_b = rig.begin(&passport_b);
    let b_ticks = rig.feed_until_live(epoch_b, dark());
    let (reveal, hidden) = b_ticks.split_last().expect("есть тики");
    assert!(!hidden.is_empty(), "до срока кадры B не показывались");
    for tick in hidden {
        assert_eq!(
            tick.record.label,
            FrameLabel::Transition(TransitionKind::HeldLastFrame),
            "кадр B обработан ISP, но на экране удержанный A"
        );
        assert!(tick.record.crop.is_some(), "кадр B действительно обработан ISP");
        let held = tick.held.as_ref().expect("переходный кадр читается");
        assert_picture(
            held,
            content_size(SIZE_A),
            gray_ref,
            "под скрытыми кадрами B — последний кадр A",
        );
    }
    assert_eq!(reveal.record.label, FrameLabel::Live);
    let counters = rig.counters();
    assert_eq!(counters.holds_revealed_by_timeout, 1, "{counters:?}");
    assert_eq!(counters.holds_revealed_by_condition, 0, "{counters:?}");
    assert!(
        counters.last_hold_duration_ms.is_some_and(|ms| (25..=45).contains(&ms)),
        "показ по сроку max_hold_ms = 25: {counters:?}"
    );
    assert_eq!(
        counters.frames_hidden_by_continuity,
        hidden.len() as u64,
        "{counters:?}"
    );
    assert_eq!(counters.black_frames_while_picture_expected, 0, "{counters:?}");
    let processed = rig
        .backend
        .diagnostics()
        .viewfinder
        .expect("диагностика")
        .counters
        .frames_processed;
    assert_eq!(
        processed - processed_before,
        b_ticks.len() as u64,
        "frames_processed сходится: скрытые удержанием кадры обработаны, как показанные"
    );
    finish(&mut rig, &[epoch_b]);
}

/// Тёмная комната: цель AE недостижима, поток B стоит на самом светлом краю своих пределов (ровно то, что
/// видно в логе телефона: `product = reach.brightest`, нехватка −4 EV). AE сделал всё возможное, поэтому кадры
/// B открываются по ГОТОВНОСТИ, а не по сроку `max_hold_ms` (до исправления каждая смена шла по таймауту).
#[test]
fn a_dark_room_stream_pinned_at_the_reach_edge_is_revealed_by_readiness_not_by_timeout() {
    let Some(mut rig) = Rig::new(test_continuity(RevealExposureCheck::MeasuredAndPlausible), true) else {
        return;
    };
    let passport_a = Arc::new(passport(MAIN, SIZE_A));
    let passport_b = Arc::new(passport(WIDE, SIZE_B));
    let epoch_a = rig.begin(&passport_a);
    rig.feed_until_live(epoch_a, gray());
    rig.stream.producer().end_stream(epoch_a);
    let _gap = (0..3).map(|_| rig.tick()).collect::<Vec<_>>();

    // «Движок» поставил запрос потока B: кадры идут с 1 с·ISO (10 мс × ISO 100 в стенде) — это самое светлое
    // произведение потока, темнее можно до 0,01 с·ISO.
    let product = |seconds_iso: f64| ExposureProduct::from_seconds_iso(seconds_iso).expect("произведение");
    rig.exposure_directives.publish(ExposureDirective {
        applied: Some(AppliedExposure {
            product: product(1.0),
            shortfall_ev: -4.0,
            reach: ProductReach::Between {
                darkest: product(0.01),
                brightest: product(1.0),
            },
        }),
        ..ExposureDirective::DEFAULT
    });

    let epoch_b = rig.begin(&passport_b);
    let b_ticks = rig.feed_until_live(epoch_b, dark());
    let (reveal, _hidden) = b_ticks.split_last().expect("есть тики");
    assert_eq!(reveal.record.label, FrameLabel::Live);
    let counters = rig.counters();
    assert_eq!(counters.holds_revealed_by_condition, 1, "{counters:?}");
    assert_eq!(counters.holds_revealed_by_timeout, 0, "{counters:?}");
    assert!(
        counters.last_hold_duration_ms.is_some_and(|ms| ms < 25),
        "открыли раньше max_hold_ms = 25: {counters:?}"
    );
    finish(&mut rig, &[epoch_b]);
}
