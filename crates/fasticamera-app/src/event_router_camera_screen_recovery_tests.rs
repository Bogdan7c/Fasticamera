//! Сквозные тесты экрана камеры, часть 2 (ревью P6e): рендер снял экран сам — сервис это замечает и ставит свежий;
//! долг разрыва касаний гасится по тику; отчёт экрана не публикуется на каждом пустом кадре. Помощники
//! (`screen_harness`, `RenderClock`, `touch`…) — из родительского модуля `camera_screen_e2e`.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use render_loop::ui_overlay::UiOverlayCommand;

use super::*;
use crate::camera_screen_service::ScreenEnvironment;
use crate::viewfinder_control::RenderControl;

// ---- Рендер снял экран сам (находка 1) -------------------------------------------------------------------------

/// Рендер снимает источник без ведома сервиса (паника источника, отказ создания рендера UI): источник уничтожается.
fn renderer_drops_the_screen(harness: &Harness) {
    let delivery = harness.control.ui_overlay(UiOverlayCommand::Remove);
    assert!(delivery.is_delivered());
    assert_eq!(
        harness.control.installed_ui_source_name(),
        None,
        "источника в рендере больше нет"
    );
}

/// Тик экрана в заданный момент (часы подставляет тест, а не стена: паузы проверяются без `sleep`).
fn maintain_at(harness: &mut Harness, now: Instant) {
    let environment = ScreenEnvironment {
        services: &harness.router.services,
        gpu: &harness.router.gpu,
        camera: &harness.router.camera,
        orientation: harness.router.display.orientation(),
    };
    harness.router.camera_screen.maintain(now, &environment);
}

fn reinstall_config(max_attempts: u32, pause_ms: u64) -> CameraScreenConfig {
    CameraScreenConfig {
        reinstall: crate::camera_screen_config::ScreenReinstallConfig {
            max_attempts,
            pause_ms,
            // Час: тесты двигают «время» на секунды, и экран не должен считаться прожившим здорово без их ведома.
            healthy_after_ms: 3_600_000,
        },
        ..instant_capture_config()
    }
}

fn ready_screen_with(config: CameraScreenConfig) -> (Harness, RenderClock) {
    let mut harness = screen_harness(config);
    start_streaming(&mut harness);
    let mut clock = RenderClock::new();
    clock.frames(&harness, 2);
    (harness, clock)
}

#[test]
fn a_screen_dropped_by_the_renderer_gets_an_explicit_status_and_a_fresh_working_screen() {
    let (mut harness, mut clock) = ready_screen_with(reinstall_config(3, 0));
    assert_eq!(harness.router.camera_screen().installs(), 1);

    renderer_drops_the_screen(&harness);
    // Сервис узнаёт об этом на ближайшем тике маршрутизатора, а не молча остаётся «Installed».
    assert_eq!(harness.router.step(Duration::ZERO), RouterStep::Idle);

    let service = harness.router.camera_screen();
    assert_eq!(
        service.status(),
        &CameraScreenStatus::Installed,
        "пауза 0: свежий экран уже стоит"
    );
    assert_eq!(service.installs(), 2);
    let recovery = service.recovery();
    assert_eq!(
        (recovery.renderer_drops, recovery.reinstalls, recovery.exhausted),
        (1, 1, 0)
    );
    assert_eq!(harness.control.installed_ui_source_name(), Some("camera_screen"));

    // Новый экран живой: касание проходит весь путь и считается жестом нового экрана.
    clock.frames(&harness, 2);
    let (x_pt, y_pt) = frame_point_pt(0.5, 0.5);
    touch(&mut harness, TouchPhase::Began, x_pt, y_pt, clock.now_ms);
    clock.frame(&harness);
    touch(&mut harness, TouchPhase::Ended, x_pt, y_pt, clock.now_ms + 50);
    clock.frames(&harness, 3);
    let report = harness.router.camera_screen().report().expect("отчёт нового экрана");
    assert_eq!(
        report.diagnostics.gestures.taps, 1,
        "тап дошёл до НОВОГО экрана: {report:?}"
    );
}

#[test]
fn the_status_while_waiting_for_the_pause_is_dropped_by_renderer_and_nothing_is_installed_early() {
    let (mut harness, _clock) = ready_screen_with(reinstall_config(3, 1_000));
    renderer_drops_the_screen(&harness);
    let dropped_at = Instant::now();

    maintain_at(&mut harness, dropped_at);
    assert!(
        matches!(
            harness.router.camera_screen().status(),
            CameraScreenStatus::DroppedByRenderer { attempt: 1, .. }
        ),
        "{:?}",
        harness.router.camera_screen().status()
    );
    // Касание на паузе — отброшено со счётчиком, не ушло в никуда.
    let touches_dropped_before = harness.router.camera_screen().dropped_input().touches;
    touch(&mut harness, TouchPhase::Began, 100.0, 300.0, 1);
    assert_eq!(
        harness.router.camera_screen().dropped_input().touches,
        touches_dropped_before + 1
    );

    maintain_at(&mut harness, dropped_at + Duration::from_millis(999));
    assert_eq!(harness.control.installed_ui_source_name(), None, "пауза ещё не вышла");
    maintain_at(&mut harness, dropped_at + Duration::from_millis(1_000));
    assert_eq!(harness.control.installed_ui_source_name(), Some("camera_screen"));
    assert_eq!(harness.router.camera_screen().status(), &CameraScreenStatus::Installed);
}

#[test]
fn repeated_drops_exhaust_the_attempts_and_end_in_an_explicit_refusal() {
    let (mut harness, _clock) = ready_screen_with(reinstall_config(2, 1_000));
    let start = Instant::now();

    for round in 0..2u32 {
        renderer_drops_the_screen(&harness);
        maintain_at(&mut harness, start + Duration::from_secs(u64::from(round) * 10));
        maintain_at(&mut harness, start + Duration::from_secs(u64::from(round) * 10 + 2));
        assert_eq!(
            harness.router.camera_screen().status(),
            &CameraScreenStatus::Installed,
            "попытка {} поставила экран",
            round + 1
        );
    }
    // Третье снятие подряд: попыток было две, больше не пробуем.
    renderer_drops_the_screen(&harness);
    maintain_at(&mut harness, start + Duration::from_secs(20));
    assert!(
        matches!(
            harness.router.camera_screen().status(),
            CameraScreenStatus::ReinstallExhausted { attempts: 2, .. }
        ),
        "{:?}",
        harness.router.camera_screen().status()
    );
    let recovery = harness.router.camera_screen().recovery();
    assert_eq!(
        (recovery.renderer_drops, recovery.reinstalls, recovery.exhausted),
        (3, 2, 1)
    );
    // Отказ окончательный: сколько ни ждать, экран не ставится (нет вечного цикла).
    maintain_at(&mut harness, start + Duration::from_secs(3_600));
    assert_eq!(harness.control.installed_ui_source_name(), None);
    assert_eq!(harness.router.camera_screen().installs(), 3);
}

#[test]
fn a_screen_that_lived_healthily_before_being_dropped_starts_a_fresh_series() {
    let (mut harness, _clock) = ready_screen_with(reinstall_config(1, 100));
    let start = Instant::now();
    // Каждое снятие приходится «через час» после установки: экран жил здорово, значит это новый сбой, а не повтор.
    for round in 1..=3u64 {
        renderer_drops_the_screen(&harness);
        maintain_at(&mut harness, start + Duration::from_secs(round * 7_200));
        assert!(
            matches!(
                harness.router.camera_screen().status(),
                CameraScreenStatus::DroppedByRenderer { attempt: 1, .. }
            ),
            "раунд {round}: {:?}",
            harness.router.camera_screen().status()
        );
        maintain_at(&mut harness, start + Duration::from_secs(round * 7_200 + 1));
        assert_eq!(harness.router.camera_screen().status(), &CameraScreenStatus::Installed);
    }
    assert_eq!(harness.router.camera_screen().recovery().exhausted, 0);
}

#[test]
fn a_scenario_started_while_the_screen_is_dropped_keeps_it_off_until_the_scenario_ends() {
    let (mut harness, _clock) = ready_screen_with(reinstall_config(3, 1_000));
    renderer_drops_the_screen(&harness);
    let now = Instant::now();
    maintain_at(&mut harness, now);

    let finish = Arc::new(Mutex::new(false));
    harness
        .starter
        .start(Box::new(ProbeScenario {
            policy: CameraScreenPolicy::SuspendForScenario,
            finish: Arc::clone(&finish),
            installs_own_ui: false,
        }))
        .expect("старт");
    harness.router.step(Duration::ZERO);
    assert!(matches!(
        harness.router.camera_screen().status(),
        CameraScreenStatus::SuspendedForScenario { .. }
    ));
    // Пауза вышла, но сценарий ещё идёт: экран не ставится поверх сценария.
    maintain_at(&mut harness, now + Duration::from_secs(5));
    assert_eq!(harness.control.installed_ui_source_name(), None);

    *finish.lock().expect("замок") = true;
    harness.router.step(Duration::ZERO);
    assert_eq!(harness.router.camera_screen().status(), &CameraScreenStatus::Installed);
}

// ---- Долг разрыва касаний по тику (находка 5) ----------------------------------------------------------------

#[test]
fn a_lost_touch_end_is_followed_by_a_gap_on_the_router_tick_without_any_new_touch() {
    let config = CameraScreenConfig {
        input_queue_capacity: 2,
        ..instant_capture_config()
    };
    let (mut harness, mut clock) = ready_screen_with(config);
    let (x_pt, y_pt) = frame_point_pt(0.5, 0.5);
    let gaps_before = harness
        .router
        .camera_screen()
        .report()
        .expect("отчёт")
        .diagnostics
        .touch_gaps;

    // Рендер «завис»: Began и Moved влезли, Ended потерян.
    touch(&mut harness, TouchPhase::Began, x_pt, y_pt, clock.now_ms);
    touch(&mut harness, TouchPhase::Moved, x_pt, y_pt, clock.now_ms + 10);
    touch(&mut harness, TouchPhase::Ended, x_pt, y_pt, clock.now_ms + 20);
    assert_eq!(harness.router.camera_screen().input_stats().overflowed_touches, 1);

    // Рендер ожил и разгрёб очередь; НОВЫХ касаний нет — только тик маршрутизатора.
    clock.frame(&harness);
    assert_eq!(harness.router.step(Duration::ZERO), RouterStep::Idle);
    clock.frame(&harness);

    assert_eq!(harness.router.camera_screen().input_stats().gaps_queued, 1);
    let diagnostics = harness.router.camera_screen().report().expect("отчёт").diagnostics;
    assert_eq!(
        diagnostics.touch_gaps,
        gaps_before + 1,
        "экран получил разрыв и больше не держит палец нажатым: {diagnostics:?}"
    );
}

// ---- Отчёт не на каждом кадре (находка 8) --------------------------------------------------------------------

#[test]
fn idle_frames_do_not_republish_the_report_before_the_interval_but_input_publishes_at_once() {
    let config = CameraScreenConfig {
        report_min_interval_ms: 1_000,
        ..instant_capture_config()
    };
    let (mut harness, mut clock) = ready_screen_with(config);
    let frames_published = |harness: &Harness| {
        harness
            .router
            .camera_screen()
            .report()
            .expect("отчёт")
            .diagnostics
            .frames_produced
    };
    // К этому моменту нарисовано 2 кадра; опубликован только первый (интервал 1 с ещё не вышел).
    let published = frames_published(&harness);
    assert_eq!(published, 1, "второй кадр отчёт не переписал");

    // 10 кадров по 16 мс: событий нет, интервал не вышел — доска не обновляется.
    clock.frames(&harness, 10);
    assert_eq!(
        frames_published(&harness),
        published,
        "отчёт не переписывается на пустых кадрах"
    );

    // Вход в кадре — публикуем сразу: теперь нарисовано 2 + 10 + 1 = 13 кадров.
    harness.deliver(ShellEvent::DeviceOrientation(DeviceOrientation::LandscapeLeft));
    clock.frame(&harness);
    assert_eq!(frames_published(&harness), 13, "кадр со входом опубликован немедленно");

    // Интервал вышел — публикация и без событий (70 кадров × 16 мс > 1 с).
    for _ in 0..70 {
        clock.frame(&harness);
    }
    assert!(frames_published(&harness) > 13, "по таймеру отчёт обновился");
}
