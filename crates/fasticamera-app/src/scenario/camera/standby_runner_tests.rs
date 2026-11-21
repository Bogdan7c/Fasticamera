//! Исполнитель `standby` на подставных наблюдениях: весь ход, и каждая причина провала отдельно.

use std::sync::Arc;

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::zoom::ZoomCatalog;
use camera_core::backend_standby::StandbySlotId;
use camera_core::lens_status::{LensCounters, LensSwitchFailure};
use camera_core::standby_status::{StandbyCounters, StandbyFailureReason, StandbySlotState, StandbySlotStatus};

use super::super::standby_views::ActiveLensView;
use super::*;
use crate::camera_test_support::oneplus_zoom_catalog;

const STEP_MS: u64 = 250;

fn catalog() -> Arc<ZoomCatalog> {
    oneplus_zoom_catalog()
}

fn camera_of(role: LensRole) -> CameraId {
    let catalog = catalog();
    let group = catalog.back.as_ref().expect("задняя шкала");
    group.lens_for_role(role).expect("объектив").camera.clone()
}

fn engine(active: LensRole, steady: bool, slots: Vec<StandbySlotStatus>) -> StandbyEngineView {
    StandbyEngineView {
        streaming: true,
        catalog: catalog(),
        active: Some(ActiveLensView {
            role: active,
            camera: camera_of(active),
            facing: LensFacing::Back,
        }),
        steady_role: steady.then_some(active),
        slots,
        standby_counters: StandbyCounters::default(),
        lens_counters: LensCounters::default(),
        stale_backend_events: 0,
        dropped_capture_events: 0,
    }
}

fn slot(role: LensRole, state: StandbySlotState) -> StandbySlotStatus {
    StandbySlotStatus {
        slot: StandbySlotId::new(0),
        camera: Some(camera_of(role)),
        state,
    }
}

/// Стенд: часы и счётчики идут сами (60 к/с показываемой, 60 к/с тёплой, пока `standby_running`).
struct Rig {
    runner: StandbyRunner,
    now: Instant,
    stream: RawStreamCounters,
    standby_running: bool,
}

impl Rig {
    fn new() -> Self {
        let now = Instant::now();
        Self {
            runner: StandbyRunner::new(StandbyScenarioConfig::default(), now),
            now,
            stream: RawStreamCounters::default(),
            standby_running: false,
        }
    }

    fn tick(&mut self, engine: &StandbyEngineView, events: &[CameraEvent]) -> StandbyTick {
        self.now += Duration::from_millis(STEP_MS);
        let frames = 60 * STEP_MS / 1000;
        self.stream.frames_published += frames;
        if self.standby_running {
            self.stream.standby_frames_returned += frames;
        }
        self.runner.tick(&StandbyObservation {
            now: self.now,
            engine,
            events,
            stream: &self.stream,
        })
    }

    /// Тикает, пока исполнитель не сменит шаг; возвращает собранные намерения.
    fn tick_until_stage_changes(&mut self, engine: &StandbyEngineView) -> Vec<LensIntent> {
        let before = self.runner.stage_name();
        let mut intents = Vec::new();
        for _ in 0..200 {
            intents.extend(self.tick(engine, &[]).intents);
            if self.runner.stage_name() != before {
                return intents;
            }
        }
        panic!("шаг {before} не закончился за 200 тиков");
    }

    fn tick_failure(&mut self, engine: &StandbyEngineView, events: &[CameraEvent]) -> String {
        match self.tick(engine, events).state {
            RunnerState::Failed(reason) => reason,
            other => panic!("ждали провал, получили {other:?}"),
        }
    }
}

fn switched(to: LensRole, method: LensSwitchMethod) -> CameraEvent {
    CameraEvent::LensSwitched {
        from: LensRole::Wide,
        to,
        elapsed: Duration::from_millis(90),
        method,
    }
}

/// Доводит стенд до шага «Обе камеры» (дом → одна камера → тёплая готова).
fn rig_with_ready_standby() -> Rig {
    let mut rig = Rig::new();
    let home = engine(LensRole::Wide, true, Vec::new());
    rig.tick_until_stage_changes(&home); // home → alone_window
    rig.tick_until_stage_changes(&home); // alone → warm
    rig.standby_running = true;
    let ready = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&ready); // warm → both_window
    rig
}

#[test]
fn the_whole_run_promotes_twice_measures_both_cameras_and_ends_with_one_open_camera() {
    let mut rig = Rig::new();
    let home = engine(LensRole::Wide, true, Vec::new());

    // Дом: первое намерение — 1× без спроса, плавности нет.
    let first = rig.tick(&home, &[]);
    assert_eq!(first.intents.len(), 1);
    assert_eq!(first.intents[0].standby, StandbyDemand::NotNeeded);
    assert_eq!(first.intents[0].gesture, ZoomGesture::Discrete);
    assert_eq!(first.intents[0].facing, LensFacing::Back);
    assert_eq!(rig.runner.stage_name(), "alone_window");

    // Окно одной камеры; затем на первом тике шага «тёплая» — просьба: 1×, плавный жест, спрос на соседа.
    let no_intents = rig.tick_until_stage_changes(&home);
    assert!(no_intents.is_empty(), "окно одной камеры ничего не публикует");
    assert_eq!(rig.runner.stage_name(), "warm");
    rig.standby_running = true;
    let warming = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Warming)],
    );
    let warm_intents = rig.tick(&warming, &[]).intents;
    assert_eq!(warm_intents.len(), 1);
    assert_eq!(warm_intents[0].standby, StandbyDemand::KeepNeighbourWarm);
    assert_eq!(warm_intents[0].gesture, ZoomGesture::Continuous);
    assert!((warm_intents[0].ratio.value() - 1.0).abs() < 1e-6);
    assert!(
        rig.tick(&warming, &[]).intents.is_empty(),
        "просьба публикуется один раз"
    );

    // Тёплая готова.
    let ready = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&ready);
    assert_eq!(rig.runner.stage_name(), "both_window");
    assert_eq!(rig.runner.record().neighbour_role, Some("ultra_wide"));
    assert!(rig.runner.record().time_to_ready_ms.is_some());

    // Окно «обе»; затем намерение ниже порога соседа со спросом и событие обмена.
    rig.tick_until_stage_changes(&ready);
    assert_eq!(rig.runner.stage_name(), "swap_to_neighbour");
    let out_intents = rig.tick(&ready, &[]).intents;
    assert_eq!(out_intents.len(), 1);
    assert_eq!(out_intents[0].standby, StandbyDemand::KeepNeighbourWarm);
    assert_eq!(out_intents[0].gesture, ZoomGesture::Continuous);
    assert!(out_intents[0].ratio.value() < 1.0);
    let tick = rig.tick(
        &ready,
        &[switched(LensRole::UltraWide, LensSwitchMethod::PromotedStandby)],
    );
    assert_eq!(tick.state, RunnerState::Running);
    assert_eq!(rig.runner.stage_name(), "after_to_neighbour");

    // После обмена бывшая основная — тёплая (готова); окно, затем обратно: выше порога гистерезиса.
    let swapped = engine(
        LensRole::UltraWide,
        true,
        vec![slot(LensRole::Wide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&swapped);
    assert_eq!(rig.runner.stage_name(), "swap_back_home");
    let back_intents = rig.tick(&swapped, &[]).intents;
    assert_eq!(back_intents.len(), 1);
    assert!(
        back_intents[0].ratio.value() >= 1.05,
        "выше гистерезиса вверх: {:?}",
        back_intents[0].ratio
    );
    assert_eq!(back_intents[0].standby, StandbyDemand::KeepNeighbourWarm);
    rig.tick(&swapped, &[switched(LensRole::Wide, LensSwitchMethod::PromotedStandby)]);
    assert_eq!(rig.runner.stage_name(), "after_back_home");

    // Дом снова; окно; «не нужно» — 1×, без спроса, явный выбор.
    rig.tick_until_stage_changes(&ready);
    assert_eq!(rig.runner.stage_name(), "release");
    let release_intents = rig.tick(&ready, &[]).intents;
    assert_eq!(release_intents.len(), 1);
    assert_eq!(release_intents[0].standby, StandbyDemand::NotNeeded);
    assert_eq!(release_intents[0].gesture, ZoomGesture::Discrete);
    assert!((release_intents[0].ratio.value() - 1.0).abs() < 1e-6);

    // Ячейка освободилась — прогон закончен.
    let closed = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Idle)],
    );
    assert_eq!(rig.tick(&closed, &[]).state, RunnerState::AllDone);

    let record = rig.runner.record();
    assert_eq!(record.swaps.len(), 2);
    assert!(
        record
            .swaps
            .iter()
            .all(|swap| swap.method == LensSwitchMethod::PromotedStandby)
    );
    assert_eq!(record.windows.len(), 4);
    let both: Vec<_> = record
        .windows
        .iter()
        .filter(|window| window.both_cameras_running)
        .collect();
    assert_eq!(both.len(), 3);
    for window in both {
        assert!((window.primary_fps - 60.0).abs() < 1.0, "{window:?}");
        assert!((window.standby_fps - 60.0).abs() < 1.0, "{window:?}");
    }
    assert!(record.release_ms.is_some());
    assert_eq!(rig.tick(&closed, &[]).state, RunnerState::AllDone, "итог не меняется");
}

#[test]
fn a_reopened_swap_is_recorded_as_such_for_the_verdict_to_judge() {
    let mut rig = rig_with_ready_standby();
    let ready = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&ready); // both → swap
    rig.tick(&ready, &[switched(LensRole::UltraWide, LensSwitchMethod::Reopened)]);
    assert_eq!(rig.runner.record().swaps[0].method, LensSwitchMethod::Reopened);
}

#[test]
fn home_that_never_comes_fails_with_the_reason_and_the_stage() {
    let mut rig = Rig::new();
    let wrong_lens = engine(LensRole::UltraWide, true, Vec::new());
    let mut reason = None;
    for _ in 0..200 {
        if let RunnerState::Failed(text) = rig.tick(&wrong_lens, &[]).state {
            reason = Some(text);
            break;
        }
    }
    let reason = reason.expect("срок «дома» вышел");
    assert!(reason.starts_with("home:"), "{reason}");
    assert!(reason.contains("задняя основная камера на 1×"), "{reason}");
}

#[test]
fn home_waits_while_a_standby_from_the_previous_run_still_holds_a_camera() {
    let mut rig = Rig::new();
    let leftover = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    rig.tick(&leftover, &[]);
    assert_eq!(rig.runner.stage_name(), "home");
}

#[test]
fn a_standby_that_never_gets_ready_times_out_and_names_the_slot_state() {
    let mut rig = Rig::new();
    let home = engine(LensRole::Wide, true, Vec::new());
    rig.tick_until_stage_changes(&home);
    rig.tick_until_stage_changes(&home);
    let warming = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Warming)],
    );
    let mut reason = None;
    for _ in 0..200 {
        if let RunnerState::Failed(text) = rig.tick(&warming, &[]).state {
            reason = Some(text);
            break;
        }
    }
    let reason = reason.expect("срок готовности вышел");
    assert!(reason.starts_with("warm:"), "{reason}");
    assert!(reason.contains("warming"), "состояние ячейки в тексте: {reason}");
}

#[test]
fn a_failed_slot_fails_the_run_at_once_with_its_reason() {
    let mut rig = Rig::new();
    let home = engine(LensRole::Wide, true, Vec::new());
    rig.tick_until_stage_changes(&home);
    rig.tick_until_stage_changes(&home);
    let failed = engine(
        LensRole::Wide,
        true,
        vec![slot(
            LensRole::UltraWide,
            StandbySlotState::Failed {
                reason: StandbyFailureReason::DeviceDisconnected,
            },
        )],
    );
    let reason = rig.tick_failure(&failed, &[]);
    assert!(reason.contains("тёплая отказала"), "{reason}");
    assert!(reason.contains("device_disconnected"), "{reason}");
}

#[test]
fn a_failed_lens_switch_event_fails_the_run() {
    let mut rig = rig_with_ready_standby();
    let ready = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    let failure = CameraEvent::LensSwitchFailed {
        from: LensRole::Wide,
        to: LensRole::UltraWide,
        reason: LensSwitchFailure::Unavailable {
            reason: CameraUnavailableReason::Evicted,
        },
    };
    let reason = rig.tick_failure(&ready, &[failure]);
    assert!(reason.contains("не удалась"), "{reason}");
}

#[test]
fn a_swap_event_that_never_comes_times_out() {
    let mut rig = rig_with_ready_standby();
    let ready = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&ready);
    let mut reason = None;
    for _ in 0..200 {
        if let RunnerState::Failed(text) = rig.tick(&ready, &[]).state {
            reason = Some(text);
            break;
        }
    }
    let reason = reason.expect("срок обмена вышел");
    assert!(reason.starts_with("swap_to_neighbour:"), "{reason}");
    assert!(reason.contains("ultra_wide"), "{reason}");
}

#[test]
fn a_swap_event_for_another_role_is_not_taken_for_the_awaited_one() {
    let mut rig = rig_with_ready_standby();
    let ready = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&ready);
    rig.tick(&ready, &[switched(LensRole::Wide, LensSwitchMethod::PromotedStandby)]);
    assert_eq!(rig.runner.stage_name(), "swap_to_neighbour");
    assert!(rig.runner.record().swaps.is_empty());
}

#[test]
fn after_the_swap_the_active_lens_must_be_the_awaited_one() {
    let mut rig = rig_with_ready_standby();
    let ready = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&ready);
    rig.tick(
        &ready,
        &[switched(LensRole::UltraWide, LensSwitchMethod::PromotedStandby)],
    );
    // Событие было, а движок всё ещё показывает основную: окно «после» это ловит.
    let mut reason = None;
    for _ in 0..200 {
        if let RunnerState::Failed(text) = rig.tick(&ready, &[]).state {
            reason = Some(text);
            break;
        }
    }
    assert!(reason.expect("провал после окна").contains("после обмена активна роль"));
}

#[test]
fn standbys_that_stay_open_after_the_release_request_time_out() {
    let mut rig = Rig::new();
    let home = engine(LensRole::Wide, true, Vec::new());
    rig.tick_until_stage_changes(&home);
    rig.tick_until_stage_changes(&home);
    rig.standby_running = true;
    let ready = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::UltraWide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&ready);
    rig.tick_until_stage_changes(&ready);
    rig.tick(
        &ready,
        &[switched(LensRole::UltraWide, LensSwitchMethod::PromotedStandby)],
    );
    let swapped = engine(
        LensRole::UltraWide,
        true,
        vec![slot(LensRole::Wide, StandbySlotState::Ready)],
    );
    rig.tick_until_stage_changes(&swapped);
    rig.tick(&swapped, &[switched(LensRole::Wide, LensSwitchMethod::PromotedStandby)]);
    rig.tick_until_stage_changes(&ready);
    assert_eq!(rig.runner.stage_name(), "release");
    let mut reason = None;
    for _ in 0..200 {
        if let RunnerState::Failed(text) = rig.tick(&ready, &[]).state {
            reason = Some(text);
            break;
        }
    }
    let reason = reason.expect("срок отпуска вышел");
    assert!(reason.starts_with("release:"), "{reason}");
    assert!(reason.contains("ready"), "{reason}");
}

#[test]
fn a_warm_camera_equal_to_home_is_rejected_instead_of_swapping_with_itself() {
    let mut rig = Rig::new();
    let home = engine(LensRole::Wide, true, Vec::new());
    rig.tick_until_stage_changes(&home);
    rig.tick_until_stage_changes(&home);
    let odd = engine(
        LensRole::Wide,
        true,
        vec![slot(LensRole::Wide, StandbySlotState::Ready)],
    );
    let reason = rig.tick_failure(&odd, &[]);
    assert!(reason.contains("совпала с «домом»"), "{reason}");
}
