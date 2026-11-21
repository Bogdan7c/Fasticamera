//! Бесшовные шаги в исполнителе `zoom`: «телефон» в тесте отвечает на настоящие образцы касаний (палец → значение
//! на экране → тёплая готова → смена объектива), а исполнитель ведёт жест до конца. Проверяется итог: записи шагов и
//! вердикт — бесшовная смена проходит, переоткрытие камеры проваливает.

use camera_core::lens_status::LensSwitchMethod;
use camera_core::standby_status::{StandbySlotState, StandbySlotStatus};

use super::*;
use crate::scenario::camera::zoom_plan::{SeamlessGesture, SeamlessLeg};
use crate::scenario::camera::zoom_seamless_verdict::seamless_step_violations;
use camera_core::backend_standby::StandbySlotId;

/// Во сколько раз изменится значение при сдвиге пальца на всю ширину экрана (в степени e).
const WHEEL_GAIN: f64 = 8.0;
/// Через сколько мс после нажатия экран открывает колесо.
const WHEEL_OPEN_MS: i64 = 500;
/// Через сколько мс после нажатия тёплая открыта и когда готова.
const SLOT_OPEN_MS: i64 = 200;
const SLOT_READY_MS: i64 = 400;
/// Через сколько мс после подъёма пальца колесо уходит и тёплая закрывается.
const WHEEL_CLOSE_MS: i64 = 600;
const SLOT_CLOSE_MS: i64 = 1_200;

/// Что «телефон» делает с тёплой.
#[derive(Clone, Copy, PartialEq)]
enum StandbyBehaviour {
    Healthy,
    /// Тёплая падает в отказ, едва открывшись.
    Dies,
}

struct Phone {
    rig: Rig,
    method: LensSwitchMethod,
    standby: StandbyBehaviour,
    ratio: f64,
    active_camera: &'static str,
    pressed_at: Option<i64>,
    lifted_at: Option<i64>,
    last_x: Option<f64>,
    pending: Option<CameraEvent>,
    touches: Vec<TouchInjection>,
}

fn seamless_steps() -> Vec<ZoomStep> {
    vec![
        step(
            "11_wheel_seamless_out",
            ZoomAction::Seamless {
                gesture: SeamlessGesture::Wheel,
                leg: SeamlessLeg::Out,
            },
        ),
        step(
            "12_wheel_seamless_back",
            ZoomAction::Seamless {
                gesture: SeamlessGesture::Wheel,
                leg: SeamlessLeg::Back,
            },
        ),
        step(
            "13_wheel_seamless_release",
            ZoomAction::Seamless {
                gesture: SeamlessGesture::Wheel,
                leg: SeamlessLeg::Release,
            },
        ),
    ]
}

impl Phone {
    fn new(method: LensSwitchMethod, standby: StandbyBehaviour) -> Self {
        let rig = Rig::new(
            seamless_steps(),
            engine(LensRole::Wide, "2", LensFacing::Back),
            screen(1_000, LensFacing::Back, 0),
        );
        Self {
            rig,
            method,
            standby,
            ratio: 1_000.0,
            active_camera: "2",
            pressed_at: None,
            lifted_at: None,
            last_x: None,
            pending: None,
            touches: Vec::new(),
        }
    }

    fn neighbour_camera(&self) -> &'static str {
        if self.active_camera == "2" { "3" } else { "2" }
    }

    /// Мир на момент `ms`: колесо, ячейка тёплой, кадры видоискателя, счётчики потока.
    fn update_world(&mut self, ms: i64) {
        let wheel_open = match (self.pressed_at, self.lifted_at) {
            (Some(pressed), None) => ms - pressed >= WHEEL_OPEN_MS,
            (Some(pressed), Some(lifted)) => ms - pressed >= WHEEL_OPEN_MS && ms - lifted < WHEEL_CLOSE_MS,
            _ => false,
        };
        let slot_state = match (self.pressed_at, self.lifted_at) {
            (Some(pressed), None) | (Some(pressed), Some(_))
                if self.lifted_at.is_none_or(|lifted| ms - lifted < SLOT_CLOSE_MS) =>
            {
                match ms - pressed {
                    elapsed if elapsed < SLOT_OPEN_MS => None,
                    _ if self.standby == StandbyBehaviour::Dies => Some(StandbySlotState::Failed {
                        reason: camera_core::standby_status::StandbyFailureReason::DeviceDisconnected,
                    }),
                    elapsed if elapsed < SLOT_READY_MS => Some(StandbySlotState::Warming),
                    _ => Some(StandbySlotState::Ready),
                }
            }
            _ => None,
        };
        let slots = slot_state
            .map(|state| {
                vec![StandbySlotStatus {
                    slot: StandbySlotId::new(0),
                    camera: Some(camera_contract::fixtures::camera_id(self.neighbour_camera())),
                    state,
                }]
            })
            .unwrap_or_default();
        let ready = slots.iter().any(|slot| slot.state == StandbySlotState::Ready);
        self.rig.engine.standby.slots = slots;
        self.rig.screen.zoom.wheel_open = wheel_open;
        self.rig.screen.zoom.ratio_milli = self.ratio.round() as u32;
        self.rig
            .frames
            .push(live_frame(ms, self.active_camera, PreviewMirror::None));
        if self.rig.frames.len() > 240 {
            self.rig.frames.remove(0);
        }
        self.rig.raw_stream.frames_published += 1;
        if ready {
            self.rig.raw_stream.standby_frames_returned += 1;
        }
    }

    /// Образец касания → значение на экране; пересечение порога объективов → событие смены.
    fn apply(&mut self, injections: Vec<Injection>, ms: i64) {
        for injection in injections {
            let Injection::Touch(touch) = injection else { continue };
            self.touches.push(touch);
            match touch.phase {
                TouchPhase::Began => {
                    self.pressed_at = Some(ms);
                    self.lifted_at = None;
                    self.last_x = Some(touch.point.x);
                }
                TouchPhase::Moved => {
                    let last = self.last_x.replace(touch.point.x).unwrap_or(touch.point.x);
                    if touch.point.x != last {
                        self.ratio = (self.ratio * (-(touch.point.x - last) * WHEEL_GAIN).exp()).clamp(593.0, 10_000.0);
                        self.rig.screen.lens_published += 1;
                        self.cross_threshold();
                    }
                }
                TouchPhase::Ended | TouchPhase::Cancelled => self.lifted_at = Some(ms),
            }
        }
    }

    fn cross_threshold(&mut self) {
        let (switch_to, role_from, role_to) = match (self.active_camera, self.ratio) {
            ("2", ratio) if ratio < 1_000.0 => ("3", LensRole::Wide, LensRole::UltraWide),
            ("3", ratio) if ratio >= 1_050.0 => ("2", LensRole::UltraWide, LensRole::Wide),
            _ => return,
        };
        self.active_camera = switch_to;
        self.rig.engine = {
            let mut next = engine(role_to, switch_to, LensFacing::Back);
            next.standby = self.rig.engine.standby.clone();
            next
        };
        self.pending = Some(CameraEvent::LensSwitched {
            from: role_from,
            to: role_to,
            elapsed: Duration::from_millis(40),
            method: self.method,
        });
        if self.method == LensSwitchMethod::Reopened {
            // Переоткрытие: удержанный кадр, а не живой.
            self.rig.counters.held_frames_presented += 8;
        }
    }

    /// Один тик по 16 мс.
    fn tick(&mut self) -> ZoomTick {
        let ms = self.rig.elapsed_ms + 16;
        self.update_world(ms);
        let events: Vec<CameraEvent> = self.pending.take().into_iter().collect();
        let tick = self.rig.tick(16, &events);
        self.apply(tick.injections.clone(), ms);
        tick
    }

    fn run_to_end(&mut self) -> RunnerState {
        for _ in 0..4_000 {
            let tick = self.tick();
            if tick.state != RunnerState::Running {
                return tick.state;
            }
        }
        panic!("прогон не закончился: {:?}", self.rig.runner.unfinished_names());
    }
}

#[test]
fn a_promoted_wheel_gesture_runs_out_back_and_release_with_clean_records() {
    let mut phone = Phone::new(LensSwitchMethod::PromotedStandby, StandbyBehaviour::Healthy);
    assert_eq!(phone.run_to_end(), RunnerState::AllDone);

    let records = phone.rig.runner.records();
    assert_eq!(records.len(), 3);
    let config = ZoomScenarioConfig::default();
    for record in records {
        assert_eq!(record.failure, None, "{}", record.name);
        assert_eq!(
            seamless_step_violations(&config.seamless, record),
            Vec::<String>::new(),
            "{}",
            record.name
        );
    }
    let out = records[0].seamless.as_ref().expect("запись жеста");
    assert_eq!(
        records[0].switch.expect("смена").method,
        LensSwitchMethod::PromotedStandby
    );
    assert_eq!(out.switch_events, 1);
    let ready = out.ready_ms.expect("время готовности измерено");
    assert!(
        (SLOT_READY_MS as f64..SLOT_READY_MS as f64 + 100.0).contains(&ready),
        "{ready}"
    );
    let fps = out.fps.expect("частота измерена");
    assert!(fps.primary > 55.0 && fps.standby > 55.0, "{fps:?}");
    let gap = out.gap.expect("разрыв измерен");
    assert!(gap.periods() <= 2.0, "{gap:?}");
    assert_eq!(records[1].new_camera.as_deref(), Some("2"), "обратно на основную");
    let closed = records[2]
        .seamless
        .as_ref()
        .expect("запись")
        .closed_ms
        .expect("время закрытия");
    assert!(
        closed >= SLOT_CLOSE_MS as f64,
        "тёплая закрылась после паузы, а не сразу: {closed}"
    );
    // Пальцы подняты ровно один раз, после всех движений.
    let ended = phone
        .touches
        .iter()
        .filter(|touch| touch.phase == TouchPhase::Ended)
        .count();
    assert_eq!(ended, 1);
    assert_eq!(phone.touches.last().map(|touch| touch.phase), Some(TouchPhase::Ended));
}

#[test]
fn a_reopened_switch_fails_the_seamless_verdict_even_though_the_gesture_completes() {
    let mut phone = Phone::new(LensSwitchMethod::Reopened, StandbyBehaviour::Healthy);
    assert_eq!(phone.run_to_end(), RunnerState::AllDone);

    let config = ZoomScenarioConfig::default();
    let out = &phone.rig.runner.records()[0];
    let violations = seamless_step_violations(&config.seamless, out);
    assert!(
        violations.iter().any(|text| text.contains("Reopened")),
        "{violations:?}"
    );
    assert!(
        violations.iter().any(|text| text.contains("удержанных кадров 8")),
        "{violations:?}"
    );
}

#[test]
fn a_standby_failure_fails_the_step_and_lifts_the_fingers() {
    let mut phone = Phone::new(LensSwitchMethod::PromotedStandby, StandbyBehaviour::Dies);
    let RunnerState::Failed(reason) = phone.run_to_end() else {
        panic!("ждали провал");
    };
    assert!(reason.contains("тёплая камера в отказе"), "{reason}");
    let last = phone.touches.last().expect("касания были");
    assert_eq!(last.phase, TouchPhase::Ended, "пальцы подняты после провала");
}

#[test]
fn a_back_leg_without_lying_fingers_fails_before_touching_the_screen() {
    let mut rig = Rig::new(
        vec![step(
            "12_wheel_seamless_back",
            ZoomAction::Seamless {
                gesture: SeamlessGesture::Wheel,
                leg: SeamlessLeg::Back,
            },
        )],
        engine(LensRole::UltraWide, "3", LensFacing::Back),
        screen(593, LensFacing::Back, 0),
    );
    let tick = rig.tick(0, &[]);
    let RunnerState::Failed(reason) = tick.state else {
        panic!("ждали провал");
    };
    assert!(reason.contains("нечего вести"), "{reason}");
    assert!(tick.injections.is_empty());
}
