//! Машина одной части бесшовного жеста сценария `zoom` (13b): нажатие → ждём тёплую → окно частоты → ведём значение
//! через порог объективов → (позже) отпускаем и ждём, пока тёплая закроется.
//!
//! Жест настоящий: палец (или два) ложится на экран образцами касаний, экран сам распознаёт долгое нажатие, открывает
//! колесо и просит у движка тёплого соседа. Машина ничего не знает о пикселях на единицу зума: каждый образец она
//! выбирает по тому, что показывает экран (`ratio_milli`), и двигает пальцы дальше, пока значение не дошло до цели из
//! каталога. Каждая фаза ведётся наблюдением и имеет срок, а не пустой «поспать».
//!
//! # Фазы
//! 1. **WaitIdle** (только `Out`): ждём, пока колесо ушло, а тёплых нет, — иначе «время до готовности» меряло бы
//!    остаток прошлого окна, а не холодный старт.
//! 2. **Arming**: пальцы легли. Колесо — ждём, пока экран откроет его долгим нажатием; щипок — понемногу разводим
//!    пальцы, пока движок не начнёт греть тёплую (спрос появляется, когда арена признала щипок).
//! 3. **WaitReady**: ждём готовую тёплую камеру нужного объектива, пальцы неподвижны. Записывается время «нажатие →
//!    готова».
//! 4. **FpsWindow**: пока стримят обе камеры, меряем частоту показываемой и тёплой.
//! 5. **Crossing**: ведём значение к цели каталога по образцу за интервал; дошли — ещё несколько неподвижных
//!    образцов («продолжаем немного»). С этого момента отсчитываются времена шага (сигнал [`SeamlessSignal::CrossingStarted`]).
//! 6. **WaitClosed** (только `Release`): пальцы подняты; ждём, пока колесо ушло и ни одна ячейка не держит камеру.
//!
//! Пальцы между частями жеста остаются лежать ([`SeamlessProgress::into_held`]).

use std::time::{Duration, Instant};

use camera_contract::ids::CameraId;
use camera_contract::zoom::{LensGroup, LensZoomPoint};
use raw_frame_stream::RawStreamCounters;

use super::standby_plan::{ratio_at_native_mark, window_fps};
use super::zoom_plan::{SeamlessGesture, SeamlessLeg, SwitchExpectation};
use super::zoom_seamless_fingers::{Fingers, ZoomDirection};
use super::zoom_seamless_record::{FpsPair, SeamlessRecord};
use super::zoom_tick::{Injection, ZoomObservation};
use crate::zoom_scenario_config::{SeamlessPlan, ZoomScenarioConfig};

/// Допуск «значение дошло до цели вниз» (тысячные): экран округляет, а нижний предел шкалы может не совпасть с
/// собственным коэффициентом объектива до тысячной.
const REACHED_TOLERANCE_MILLI: u32 = 8;

/// Цель жеста: объектив и коэффициент экрана, при котором он включается.
#[derive(Debug, Clone, PartialEq)]
pub struct SeamlessTarget {
    /// Камера объектива, на который должен перейти показ.
    pub camera: CameraId,
    /// Коэффициент цели в тысячных.
    pub ratio_milli: u32,
    /// Куда двигать значение, чтобы дойти.
    pub direction: ZoomDirection,
}

/// Фаза части жеста.
#[derive(Debug, Clone)]
enum Phase {
    WaitIdle,
    Arming,
    WaitReady,
    FpsWindow {
        since: Instant,
        before: Box<RawStreamCounters>,
    },
    Crossing {
        dwell: u32,
        moves: u32,
    },
    /// Колебания около отметки главной камеры (замер дребезга, `SeamlessLeg::Hover`).
    Hovering {
        /// Когда колебания начались.
        since: Instant,
        /// Сейчас палец идёт вниз (к нижней границе полосы) или вверх.
        descending: bool,
    },
    WaitClosed,
    Done,
}

/// Что машина сообщает исполнителю сверх образцов.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeamlessSignal {
    /// Ничего особенного.
    None,
    /// Начато движение через порог: с этого момента меряются времена шага.
    CrossingStarted,
    /// Часть жеста не удалась.
    Failed(String),
}

/// Итог тика машины.
#[derive(Debug)]
pub struct SeamlessTick {
    pub injections: Vec<Injection>,
    pub signal: SeamlessSignal,
}

/// Начало части жеста: что ожидать от объектива и что вложить сразу.
pub struct SeamlessStart {
    pub expectation: SwitchExpectation,
    pub injections: Vec<Injection>,
    pub progress: SeamlessProgress,
}

/// Идущая часть бесшовного жеста.
#[derive(Debug)]
pub struct SeamlessProgress {
    leg: SeamlessLeg,
    plan: SeamlessPlan,
    fingers: Fingers,
    /// Лежат ли пальцы на экране сейчас (для возврата и уборки после провала).
    fingers_down: bool,
    phase: Phase,
    started_at: Instant,
    /// Когда пальцы легли (отсчёт «нажатие → готова»).
    pressed_at: Option<Instant>,
    /// Когда подняты пальцы (отсчёт «закрылись»).
    lifted_at: Option<Instant>,
    next_sample_at: Instant,
    target: Option<SeamlessTarget>,
    /// Отметка главной камеры в тысячных, около которой колеблются (только у `SeamlessLeg::Hover`).
    hover_center_milli: Option<u32>,
    /// Камера, которую показ покидает.
    from_camera: Option<CameraId>,
    ready_ms: Option<f64>,
    fps: Option<FpsPair>,
    closed_ms: Option<f64>,
}

/// Объективы по обе стороны текущего на шкале: `(выше, ниже)` с «из какого в какой» для расчёта коэффициента.
fn neighbour<'a>(
    group: &'a LensGroup,
    current: &CameraId,
    below: bool,
) -> Option<(&'a LensZoomPoint, &'a LensZoomPoint)> {
    let index = group.lenses.iter().position(|lens| &lens.camera == current)?;
    let from = &group.lenses[index];
    let to = if below {
        group.lenses.get(index.checked_sub(1)?)?
    } else {
        group.lenses.get(index + 1)?
    };
    Some((from, to))
}

/// Начать часть жеста. `held` — пальцы, оставшиеся лежать от прошлой части; забираются только при успехе.
pub fn begin(
    gesture: SeamlessGesture,
    leg: SeamlessLeg,
    config: &ZoomScenarioConfig,
    observation: &ZoomObservation<'_>,
    held: &mut Option<Fingers>,
) -> Result<SeamlessStart, String> {
    let plan = config.seamless;
    let mut progress = SeamlessProgress {
        leg,
        plan,
        fingers: Fingers::new(gesture, &plan, config.wheel_drag.start),
        fingers_down: false,
        phase: Phase::WaitIdle,
        started_at: observation.now,
        pressed_at: None,
        lifted_at: None,
        next_sample_at: observation.now,
        target: None,
        hover_center_milli: None,
        from_camera: None,
        ready_ms: None,
        fps: None,
        closed_ms: None,
    };
    let mut injections = Vec::new();
    let mut expectation = SwitchExpectation::NoSwitch;
    match leg {
        SeamlessLeg::Out | SeamlessLeg::Back => {
            let below = leg == SeamlessLeg::Out;
            match (below, held.is_some()) {
                (true, true) => {
                    return Err("палец уже лежит на экране: жест «Out» начинается с чистого экрана".to_owned());
                }
                (false, false) => return Err("жест «Back» без лежащих пальцев: нечего вести".to_owned()),
                _ => {}
            }
            let active = observation.engine.active.as_ref().ok_or("активного объектива нет")?;
            let group = observation
                .engine
                .catalog
                .back
                .as_ref()
                .ok_or("в каталоге зума нет задних камер")?;
            let (_from, to) =
                neighbour(group, &active.camera, below).ok_or("на шкале нет соседнего объектива в нужную сторону")?;
            let ratio = ratio_at_native_mark(group, to);
            progress.target = Some(SeamlessTarget {
                camera: to.camera.clone(),
                ratio_milli: (f64::from(ratio.value()) * 1000.0).round() as u32,
                direction: if below { ZoomDirection::Out } else { ZoomDirection::In },
            });
            progress.from_camera = Some(active.camera.clone());
            expectation = SwitchExpectation::ToRole(to.role);
            if let Some(fingers) = held.take() {
                if fingers.gesture() != gesture {
                    *held = Some(fingers);
                    return Err("лежат пальцы другого жеста".to_owned());
                }
                progress.fingers = fingers;
                progress.fingers_down = true;
                progress.pressed_at = Some(observation.now);
                progress.phase = Phase::WaitReady;
            }
        }
        SeamlessLeg::Hover => {
            let Some(fingers) = held.take() else {
                return Err("жест «Hover» без лежащих пальцев: нечем колебаться у отметки".to_owned());
            };
            if fingers.gesture() != gesture {
                *held = Some(fingers);
                return Err("лежат пальцы другого жеста".to_owned());
            }
            let active = observation.engine.active.as_ref().ok_or("активного объектива нет")?;
            let group = observation
                .engine
                .catalog
                .back
                .as_ref()
                .ok_or("в каталоге зума нет задних камер")?;
            let lens = group
                .lenses
                .iter()
                .find(|lens| lens.camera == active.camera)
                .ok_or("активной камеры нет на шкале зума")?;
            progress.hover_center_milli = Some((f64::from(lens.native_ratio.value()) * 1000.0).round() as u32);
            progress.fingers = fingers;
            progress.fingers_down = true;
            progress.phase = Phase::Hovering {
                since: observation.now,
                descending: true,
            };
            expectation = SwitchExpectation::MaybeSwitchesBack;
        }
        SeamlessLeg::Release => {
            let Some(mut fingers) = held.take() else {
                return Err("жест «Release» без лежащих пальцев: нечего отпускать".to_owned());
            };
            injections.extend(fingers.lift(observation.render_now).into_iter().map(Injection::Touch));
            progress.fingers = fingers;
            progress.lifted_at = Some(observation.now);
            progress.phase = Phase::WaitClosed;
        }
    }
    Ok(SeamlessStart {
        expectation,
        injections,
        progress,
    })
}

impl SeamlessProgress {
    /// Часть жеста дошла до конца своей работы (дальше исполнитель ждёт смены объектива и живого кадра).
    pub fn is_done(&self) -> bool {
        matches!(self.phase, Phase::Done)
    }

    /// Действует ли общий срок шага: только после начала движения через порог и не у отпускания (у него свой срок).
    pub fn generic_timeout_applies(&self) -> bool {
        matches!(self.phase, Phase::Crossing { .. } | Phase::Done) && self.leg != SeamlessLeg::Release
    }

    /// Камера, которую показ покидает.
    pub fn leaving_camera(&self) -> Option<&CameraId> {
        self.from_camera.as_ref()
    }

    /// Пальцы, которые остаются лежать после этой части (`None` — подняты или ещё не ложились).
    pub fn into_held(self) -> Option<Fingers> {
        self.fingers_down.then_some(self.fingers)
    }

    /// Запись шага: общие поля заполняет исполнитель.
    pub fn record(&self, switch_events: u32) -> SeamlessRecord {
        SeamlessRecord {
            gesture: self.fingers.gesture().name(),
            leg: self.leg.name(),
            switch_events,
            ready_ms: self.ready_ms,
            fps: self.fps,
            gap: None,
            target_ratio_milli: self.target.as_ref().map(|target| target.ratio_milli),
            ratio_at_switch_milli: None,
            closed_ms: self.closed_ms,
        }
    }

    /// Один тик наблюдения.
    pub fn tick(&mut self, observation: &ZoomObservation<'_>) -> SeamlessTick {
        let mut injections = Vec::new();
        let signal = self.advance(observation, &mut injections);
        SeamlessTick {
            injections: injections.into_iter().map(Injection::Touch).collect(),
            signal,
        }
    }

    fn advance(
        &mut self,
        observation: &ZoomObservation<'_>,
        out: &mut Vec<super::zoom_tick::TouchInjection>,
    ) -> SeamlessSignal {
        if !matches!(self.phase, Phase::WaitIdle | Phase::Done)
            && let Some(failed) = observation.engine.standby.failed_slot()
        {
            return SeamlessSignal::Failed(format!("тёплая камера в отказе: {failed}"));
        }
        let interval = Duration::from_millis(self.plan.sample_interval_ms);
        let due = observation.now >= self.next_sample_at;
        match self.phase.clone() {
            Phase::WaitIdle => self.wait_idle(observation, out),
            Phase::Arming => self.arming(observation, interval, due, out),
            Phase::WaitReady => self.wait_ready(observation, interval, due, out),
            Phase::FpsWindow { since, before } => self.fps_window(observation, interval, due, since, &before, out),
            Phase::Crossing { dwell, moves } => self.crossing(observation, interval, due, dwell, moves, out),
            Phase::Hovering { since, descending } => self.hovering(observation, interval, due, since, descending, out),
            Phase::WaitClosed => self.wait_closed(observation),
            Phase::Done => SeamlessSignal::None,
        }
    }

    fn timed_out(&self, observation: &ZoomObservation<'_>, limit_ms: u64) -> bool {
        observation.now.saturating_duration_since(self.started_at) >= Duration::from_millis(limit_ms)
    }

    fn wait_idle(
        &mut self,
        observation: &ZoomObservation<'_>,
        out: &mut Vec<super::zoom_tick::TouchInjection>,
    ) -> SeamlessSignal {
        let zoom = observation.screen.zoom;
        if !zoom.wheel_open && !zoom.gliding && !observation.engine.standby.any_slot_holds_camera() {
            out.extend(self.fingers.press(observation.render_now));
            self.fingers_down = true;
            self.pressed_at = Some(observation.now);
            self.next_sample_at = observation.now + Duration::from_millis(self.plan.sample_interval_ms);
            self.phase = Phase::Arming;
        } else if self.timed_out(observation, self.plan.release_timeout_ms) {
            return SeamlessSignal::Failed(
                "перед жестом колесо не ушло или тёплая не закрылась: начинать с холодного старта нельзя".to_owned(),
            );
        }
        SeamlessSignal::None
    }

    /// Первая готовая тёплая запоминается сразу (даже пока колесо ещё не открылось): время считается от нажатия.
    fn note_ready(&mut self, observation: &ZoomObservation<'_>) {
        if self.ready_ms.is_none()
            && observation.engine.standby.ready_standby_camera().is_some()
            && let Some(pressed) = self.pressed_at
        {
            self.ready_ms = Some(observation.now.saturating_duration_since(pressed).as_secs_f64() * 1000.0);
        }
    }

    fn arming(
        &mut self,
        observation: &ZoomObservation<'_>,
        interval: Duration,
        due: bool,
        out: &mut Vec<super::zoom_tick::TouchInjection>,
    ) -> SeamlessSignal {
        self.note_ready(observation);
        let woke = match self.fingers.gesture() {
            SeamlessGesture::Wheel => observation.screen.zoom.wheel_open,
            SeamlessGesture::Pinch => observation.engine.standby.any_slot_holds_camera(),
        };
        if woke {
            self.phase = Phase::WaitReady;
            return SeamlessSignal::None;
        }
        if self.timed_out(observation, self.plan.ready_timeout_ms) {
            return SeamlessSignal::Failed(match self.fingers.gesture() {
                SeamlessGesture::Wheel => "долгое нажатие не открыло колесо".to_owned(),
                SeamlessGesture::Pinch => "щипок не разбудил тёплую камеру".to_owned(),
            });
        }
        if due {
            self.next_sample_at = observation.now + interval;
            if self.fingers.gesture() == SeamlessGesture::Pinch {
                // Щипок распознаётся по первому заметному изменению расстояния: разводим пальцы чуть-чуть
                // (приближение, порог объективов не затрагивает).
                out.extend(
                    self.fingers
                        .nudge(ZoomDirection::In, &self.plan, observation.render_now),
                );
            }
        }
        SeamlessSignal::None
    }

    fn wait_ready(
        &mut self,
        observation: &ZoomObservation<'_>,
        interval: Duration,
        due: bool,
        out: &mut Vec<super::zoom_tick::TouchInjection>,
    ) -> SeamlessSignal {
        self.note_ready(observation);
        if let (Some(ready), Some(target)) = (observation.engine.standby.ready_standby_camera(), &self.target) {
            if ready != &target.camera {
                return SeamlessSignal::Failed(format!(
                    "готова тёплая камера {ready}, а переход ждёт камеру {}",
                    target.camera
                ));
            }
            self.phase = Phase::FpsWindow {
                since: observation.now,
                before: Box::new(*observation.raw_stream),
            };
            return SeamlessSignal::None;
        }
        if self.timed_out(observation, self.plan.ready_timeout_ms) {
            return SeamlessSignal::Failed(format!("тёплая не стала готова за {} мс", self.plan.ready_timeout_ms));
        }
        self.keep_alive(observation, interval, due, out);
        SeamlessSignal::None
    }

    fn fps_window(
        &mut self,
        observation: &ZoomObservation<'_>,
        interval: Duration,
        due: bool,
        since: Instant,
        before: &RawStreamCounters,
        out: &mut Vec<super::zoom_tick::TouchInjection>,
    ) -> SeamlessSignal {
        let elapsed = observation.now.saturating_duration_since(since);
        if elapsed < Duration::from_millis(self.plan.fps_window_ms) {
            self.keep_alive(observation, interval, due, out);
            return SeamlessSignal::None;
        }
        self.fps = window_fps(before, observation.raw_stream, elapsed).map(|window| FpsPair {
            primary: window.primary,
            standby: window.standby,
        });
        self.next_sample_at = observation.now;
        self.phase = Phase::Crossing { dwell: 0, moves: 0 };
        SeamlessSignal::CrossingStarted
    }

    fn crossing(
        &mut self,
        observation: &ZoomObservation<'_>,
        interval: Duration,
        due: bool,
        dwell: u32,
        moves: u32,
        out: &mut Vec<super::zoom_tick::TouchInjection>,
    ) -> SeamlessSignal {
        if !due {
            return SeamlessSignal::None;
        }
        self.next_sample_at = observation.now + interval;
        let Some(target) = self.target.clone() else {
            return SeamlessSignal::Failed("у жеста нет цели".to_owned());
        };
        let shown = observation.screen.zoom.ratio_milli;
        let reached = match target.direction {
            ZoomDirection::Out => shown <= target.ratio_milli + REACHED_TOLERANCE_MILLI,
            // Вверх — не меньше самой отметки: остановка чуть ниже неё оставила бы смену несостоявшейся.
            ZoomDirection::In => shown >= target.ratio_milli,
        };
        if reached {
            out.extend(self.fingers.hold(observation.render_now));
            let dwell = dwell + 1;
            self.phase = if dwell >= self.plan.dwell_samples {
                Phase::Done
            } else {
                Phase::Crossing { dwell, moves }
            };
            return SeamlessSignal::None;
        }
        if moves >= self.plan.max_drag_samples || self.fingers.is_at_limit(target.direction) {
            return SeamlessSignal::Failed(format!(
                "пальцы не довели значение до {} тыс. (экран показывает {shown} тыс.) за {moves} образцов",
                target.ratio_milli
            ));
        }
        out.extend(self.fingers.nudge(target.direction, &self.plan, observation.render_now));
        self.phase = Phase::Crossing {
            dwell,
            moves: moves + 1,
        };
        SeamlessSignal::None
    }

    /// Колебания около отметки: вниз до нижней границы полосы, вверх до верхней, и так `hover_ms`. Что смена
    /// делает на каждом пересечении — решает движок; шаг только считает события.
    fn hovering(
        &mut self,
        observation: &ZoomObservation<'_>,
        interval: Duration,
        due: bool,
        since: Instant,
        descending: bool,
        out: &mut Vec<super::zoom_tick::TouchInjection>,
    ) -> SeamlessSignal {
        if !due {
            return SeamlessSignal::None;
        }
        self.next_sample_at = observation.now + interval;
        let Some(center) = self.hover_center_milli else {
            return SeamlessSignal::Failed("у колебаний нет центра".to_owned());
        };
        if observation.now.saturating_duration_since(since) >= Duration::from_millis(self.plan.hover_ms) {
            out.extend(self.fingers.hold(observation.render_now));
            self.phase = Phase::Done;
            return SeamlessSignal::None;
        }
        let shown = observation.screen.zoom.ratio_milli;
        let band = self.plan.hover_band_milli;
        // Развернуться на границе полосы; между границами палец идёт в текущую сторону.
        let descending = match descending {
            true => shown > center.saturating_sub(band),
            false => shown >= center + band,
        };
        let direction = if descending {
            ZoomDirection::Out
        } else {
            ZoomDirection::In
        };
        out.extend(self.fingers.nudge(direction, &self.plan, observation.render_now));
        self.phase = Phase::Hovering { since, descending };
        SeamlessSignal::None
    }

    fn wait_closed(&mut self, observation: &ZoomObservation<'_>) -> SeamlessSignal {
        let closed = !observation.screen.zoom.wheel_open && !observation.engine.standby.any_slot_holds_camera();
        if closed {
            if let Some(lifted) = self.lifted_at {
                self.closed_ms = Some(observation.now.saturating_duration_since(lifted).as_secs_f64() * 1000.0);
            }
            self.phase = Phase::Done;
        } else if self.timed_out(observation, self.plan.release_timeout_ms) {
            return SeamlessSignal::Failed(format!(
                "после подъёма пальцев за {} мс колесо не ушло или тёплая осталась открытой (колесо: {}, ячейки держат камеру: {})",
                self.plan.release_timeout_ms,
                observation.screen.zoom.wheel_open,
                observation.engine.standby.any_slot_holds_camera()
            ));
        }
        SeamlessSignal::None
    }

    /// Пальцы неподвижны, но жест жив: образец на месте раз в интервал.
    fn keep_alive(
        &mut self,
        observation: &ZoomObservation<'_>,
        interval: Duration,
        due: bool,
        out: &mut Vec<super::zoom_tick::TouchInjection>,
    ) {
        if due {
            self.next_sample_at = observation.now + interval;
            out.extend(self.fingers.hold(observation.render_now));
        }
    }
}
