//! Функциональные тесты сценария `standby` (13b): исполнитель шагов ведёт НАСТОЯЩИЙ движок камеры (поток движка,
//! каталог OnePlus 11, настоящий селектор объективов и модуль тёплых) на fake-платформе. Тест играет роль платформы
//! и маршрутизатора: подаёт кадры и первый результат тёплой, кладёт намерения исполнителя в ящик движка, считает
//! частоту по синтетическим счётчикам потока кадров.
//!
//! Проверяется то, что сломалось бы при поломке: оба обмена исполнены повышением тёплой (а не переоткрытием),
//! тёплая закрыта в конце, а без поддержки тёплых бэкендом прогон падает с понятной причиной, а не висит.

use std::time::{Duration, Instant};

use camera_contract::zoom::StandbyDemand;
use camera_core::backend_standby::StandbySlotId;
use camera_core::lens_status::LensSwitchMethod;
use raw_frame_stream::RawStreamCounters;

use super::super::standby_config::StandbyScenarioConfig;
use super::super::standby_runner::{RunnerState, StandbyObservation, StandbyRunner};
use super::super::standby_views::StandbyEngineView;
use crate::camera_config::CameraConfig;
use crate::camera_test_support::{FakeCameraRig, FrameFeeder};

/// Настоящий движок на fake: быстрая пауза до закрытия тёплых, чтобы тест не ждал секунды.
fn fast_config() -> CameraConfig {
    let mut config = CameraConfig::default();
    config.engine.standby.release_delay_ms = 150;
    config
}

fn fast_scenario_config() -> StandbyScenarioConfig {
    StandbyScenarioConfig {
        alone_window_ms: 120,
        both_window_ms: 120,
        warm_timeout_ms: 2_000,
        swap_timeout_ms: 2_000,
        release_timeout_ms: 3_000,
        home_timeout_ms: 5_000,
        ..StandbyScenarioConfig::default()
    }
}

/// Как после запуска приложения: желаемое состояние продуктового видоискателя и передний план.
fn start_product_viewfinder(rig: &FakeCameraRig) {
    let _outcome = rig.services.start_product_viewfinder();
    rig.handle().enter_foreground().expect("очередь движка приняла команду");
    rig.wait_streaming();
}

/// Итог прогона на настоящем движке.
struct DriveOutcome {
    runner: StandbyRunner,
    state: RunnerState,
    counters: RawStreamCounters,
    demands_published: Vec<StandbyDemand>,
}

/// Гоняет исполнитель, пока он не закончит (или не выйдет общий срок теста).
fn drive(rig: &FakeCameraRig, config: StandbyScenarioConfig) -> DriveOutcome {
    let handle = rig.handle();
    let started = Instant::now();
    let mut runner = StandbyRunner::new(config, started);
    let mut counters = RawStreamCounters::default();
    let mut last_tick = started;
    let mut published_frames = 0.0_f64;
    let mut standby_frames = 0.0_f64;
    let mut demands_published = Vec::new();
    loop {
        std::thread::sleep(Duration::from_millis(5));
        let now = Instant::now();
        assert!(
            now.saturating_duration_since(started) < Duration::from_secs(30),
            "прогон завис на шаге {}",
            runner.stage_name()
        );
        let status = handle.status();
        let events = handle.drain_events(usize::MAX);
        let engine = StandbyEngineView::of(&status);
        // Платформа: тёплая отдаёт первый результат (одно `StandbyStreaming` на сессию).
        let _delivery = rig.control.complete_standby_frames(StandbySlotId::new(0));
        // Синтетические счётчики потока: 60 к/с показываемой, пока идёт поток, и 60 к/с тёплой, пока ячейка занята.
        let seconds = now.saturating_duration_since(last_tick).as_secs_f64();
        last_tick = now;
        if engine.streaming {
            published_frames += 60.0 * seconds;
        }
        if engine.any_slot_holds_camera() {
            standby_frames += 60.0 * seconds;
        }
        counters.frames_published = published_frames as u64;
        counters.standby_frames_returned = standby_frames as u64;
        let tick = runner.tick(&StandbyObservation {
            now,
            engine: &engine,
            events: &events,
            stream: &counters,
        });
        for intent in tick.intents {
            demands_published.push(intent.standby);
            let _published = rig.services.lens_intents().publish(intent);
        }
        if tick.state != RunnerState::Running {
            return DriveOutcome {
                runner,
                state: tick.state,
                counters,
                demands_published,
            };
        }
    }
}

#[test]
fn a_real_engine_promotes_the_warm_camera_twice_and_closes_it_after_the_release() {
    let rig = FakeCameraRig::with_product_config(fast_config());
    start_product_viewfinder(&rig);
    let _feeder = FrameFeeder::start(&rig, Duration::from_micros(16_667));

    let outcome = drive(&rig, fast_scenario_config());

    assert_eq!(outcome.state, RunnerState::AllDone, "{:?}", outcome.runner.record());
    let record = outcome.runner.record();
    assert_eq!(record.home_role, Some("wide"));
    assert_eq!(
        record.neighbour_role,
        Some("ultra_wide"),
        "соседа выбрал движок по каталогу"
    );
    assert_eq!(record.swaps.len(), 2, "туда и обратно");
    assert!(
        record
            .swaps
            .iter()
            .all(|swap| swap.method == LensSwitchMethod::PromotedStandby),
        "оба обмена — повышение тёплой, не переоткрытие: {:?}",
        record.swaps
    );
    let both: Vec<_> = record
        .windows
        .iter()
        .filter(|window| window.both_cameras_running)
        .collect();
    assert_eq!(both.len(), 3, "до обмена и после каждого");
    assert!(both.iter().all(|window| window.standby_fps > 30.0), "{both:?}");
    assert!(record.release_ms.is_some(), "тёплые закрылись после отпуска");
    assert_eq!(
        outcome.demands_published,
        vec![
            StandbyDemand::NotNeeded,
            StandbyDemand::KeepNeighbourWarm,
            StandbyDemand::KeepNeighbourWarm,
            StandbyDemand::KeepNeighbourWarm,
            StandbyDemand::NotNeeded,
        ],
        "дом, греть, обмен туда, обмен обратно, отпуск"
    );
    assert!(outcome.counters.standby_frames_returned > 0);

    let status = rig.handle().status();
    assert_eq!(status.lens.standby.counters.promotions, 2);
    assert_eq!(status.lens.standby.counters.promotion_fallbacks, 0);
    assert_eq!(status.lens.standby.counters.failures, 0);
    assert!(
        !StandbyEngineView::of(&status).any_slot_holds_camera(),
        "после отпуска ни одна ячейка не держит камеру"
    );
}

#[test]
fn a_backend_without_standby_support_fails_the_run_with_a_reason_instead_of_hanging() {
    let rig = FakeCameraRig::with_product_config(fast_config());
    rig.control.set_standby_capacity(None);
    start_product_viewfinder(&rig);
    let _feeder = FrameFeeder::start(&rig, Duration::from_micros(16_667));
    let config = StandbyScenarioConfig {
        warm_timeout_ms: 400,
        ..fast_scenario_config()
    };

    let outcome = drive(&rig, config);

    let RunnerState::Failed(reason) = outcome.state else {
        panic!("прогон без тёплых обязан провалиться, а не пройти: {:?}", outcome.state);
    };
    assert!(reason.starts_with("warm:"), "{reason}");
    assert!(
        reason.contains("ячеек нет"),
        "причина называет состояние ячеек: {reason}"
    );
    assert!(outcome.runner.record().swaps.is_empty());
}
