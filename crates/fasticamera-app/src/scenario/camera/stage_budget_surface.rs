//! Выход на место замера и возврат с него для `viewfinder-stage-budget` (сессия 16a).
//!
//! На продуктовом экране сценарий сам ничего не нажимает: он просит жест у скрипта-драйвера (стадия
//! `budget_gesture` с полем `gesture`, тот же формат, что у сценария `screen`) и ждёт следствий в диагностике.
//! * **Вход** ([`SurfaceEntry`]): экран увидел живую камеру и нарисовал кадры UI → для CALIBRATE жест
//!   `modes_swipe_left` → режим экрана Calibrate И приборы живы (запрошены, объекты GPU созданы, проходы
//!   записываются). Не дождались за `surface_ready_timeout_ms` — провал с названием того, чего не хватило.
//! * **Выход** ([`SurfaceExit`]): для CALIBRATE жест `modes_swipe_right` и ожидание режима Photo, чтобы экран остался
//!   в нейтральном состоянии (как делает `screen`).
//!
//! Оба автомата чистые: время и наблюдаемое состояние приходят аргументами ([`SurfaceView`]), поэтому логика
//! проверяется без маршрутизатора и телефона.

use std::time::{Duration, Instant};

use camera_screen::CameraPresence;
use render_loop::viewfinder::ScopeResources;

use super::screen_gesture::Gesture;
use crate::camera_presentation::CameraPresentation;
use crate::camera_screen_source::ModeKind;
use crate::scenario::observing::ScenarioContext;
use crate::screen_scenario_config::SwipeGesture;
use crate::stage_budget_surface_config::StageBudgetSurface;

/// Имя стадии, на которой драйвер выполняет жест входа и выхода (контракт с `scripts/smoke_driver.py`).
pub const SURFACE_GESTURE_STAGE: &str = "budget_gesture";

/// Приборы глазами замера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScopesView {
    /// Экран запросил прибор.
    pub requested: bool,
    /// Объекты GPU приборов созданы.
    pub resources_live: bool,
    /// Накопительный счётчик записанных проходов.
    pub passes_recorded: u64,
}

/// Что видно снаружи про экран и приборы в один момент.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SurfaceView {
    /// Экран камеры стоит на рендере.
    pub screen_installed: bool,
    /// Экран увидел живую камеру.
    pub presence_live: bool,
    /// Режим экрана; `None` — отчёта экрана ещё нет.
    pub mode: Option<ModeKind>,
    /// Источник UI установлен, и рендер сообщил о нём.
    pub ui_overlay_active: bool,
    /// Сколько кадров UI нарисовано в GPU за жизнь рендера.
    pub ui_frames_drawn: u64,
    /// Сколько кадров нарисовал сам экран (счётчик его отчёта).
    pub screen_frames: u64,
    /// Приборы; `None` — диагностики видоискателя нет.
    pub scopes: Option<ScopesView>,
}

impl SurfaceView {
    /// Снимок по отчёту экрана и последнему снимку показа рендера.
    pub fn read(context: &ScenarioContext<'_>, presentation: &CameraPresentation) -> Self {
        let report = context.camera_screen.report();
        let viewfinder = presentation.viewfinder.as_ref();
        Self {
            screen_installed: context.camera_screen.status().is_installed(),
            presence_live: report
                .as_ref()
                .is_some_and(|report| report.presence == CameraPresence::Live),
            mode: report.as_ref().map(|report| report.state.mode),
            ui_overlay_active: viewfinder.is_some_and(|viewfinder| viewfinder.ui_overlay.active_source.is_some()),
            ui_frames_drawn: viewfinder.map_or(0, |viewfinder| viewfinder.ui_overlay.frames_drawn),
            screen_frames: report.as_ref().map_or(0, |report| report.diagnostics.frames_produced),
            scopes: viewfinder.map(|viewfinder| ScopesView {
                requested: viewfinder.scopes.requested.is_some(),
                resources_live: viewfinder.scopes.residency.resources == ScopeResources::Live,
                passes_recorded: viewfinder.scopes.counters.passes_recorded,
            }),
        }
    }
}

/// Жест, который сценарий просит у драйвера.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceGesture {
    /// Имя шага для отчёта и поля `step` стадии.
    pub step: &'static str,
    /// Зачем (по-русски, в отчёт).
    pub purpose: &'static str,
    pub swipe: SwipeGesture,
}

impl SurfaceGesture {
    /// Жест в виде поля стадии.
    pub fn to_stage_value(self) -> serde_json::Value {
        Gesture::swipe(self.swipe).to_stage_value()
    }
}

/// Свайпы входа и выхода из плана касаний `screen` (сценарий их не придумывает, а берёт те же числа).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceSwipes {
    /// Справа налево: Photo → Calibrate.
    pub enter_calibrate: SwipeGesture,
    /// Слева направо по «чистой» зоне раздела: Calibrate → Photo.
    pub leave_calibrate: SwipeGesture,
}

/// Чем кончился тик входа.
#[derive(Debug, Clone, PartialEq)]
pub enum EntryTick {
    Waiting,
    /// Выдать драйверу этот жест (один раз).
    Gesture(SurfaceGesture),
    /// Место замера достигнуто.
    Ready,
    /// Не дождались: текст провала.
    Failed(String),
}

/// Чем кончился тик выхода.
#[derive(Debug, Clone, PartialEq)]
pub enum ExitTick {
    Waiting,
    Gesture(SurfaceGesture),
    /// Экран в Photo (или возвращать нечего).
    Done,
    Failed(String),
}

#[derive(Debug, Clone, Copy)]
enum EntryPhase {
    /// Ждём живой экран с нарисованными кадрами.
    AwaitingScreen { frames_at_start: u64 },
    /// Жест выдан, ждём режим Calibrate и живые приборы.
    AwaitingCalibrate { passes_at_gesture: u64 },
}

/// Автомат входа на место замера.
#[derive(Debug)]
pub struct SurfaceEntry {
    surface: StageBudgetSurface,
    swipes: SurfaceSwipes,
    timeout: Duration,
    /// Сколько кадров UI экран обязан нарисовать, прежде чем замер начнётся (как прогрев сценария `screen`).
    warmup_ui_frames: u64,
    phase: EntryPhase,
    phase_started_at: Instant,
}

impl SurfaceEntry {
    pub fn begin(
        surface: StageBudgetSurface,
        swipes: SurfaceSwipes,
        timeout: Duration,
        warmup_ui_frames: u64,
        now: Instant,
        initial: &SurfaceView,
    ) -> Self {
        Self {
            surface,
            swipes,
            timeout,
            warmup_ui_frames,
            phase: EntryPhase::AwaitingScreen {
                frames_at_start: initial.screen_frames,
            },
            phase_started_at: now,
        }
    }

    /// Тик входа.
    pub fn tick(&mut self, now: Instant, view: &SurfaceView) -> EntryTick {
        if !self.surface.keeps_product_screen() {
            return EntryTick::Ready;
        }
        match self.phase {
            EntryPhase::AwaitingScreen { frames_at_start } => self.tick_awaiting_screen(now, view, frames_at_start),
            EntryPhase::AwaitingCalibrate { passes_at_gesture } => {
                self.tick_awaiting_calibrate(now, view, passes_at_gesture)
            }
        }
    }

    fn tick_awaiting_screen(&mut self, now: Instant, view: &SurfaceView, frames_at_start: u64) -> EntryTick {
        let warmed = view.screen_frames.saturating_sub(frames_at_start) >= self.warmup_ui_frames;
        let mode_ok = match (self.surface, view.mode) {
            (StageBudgetSurface::PhotoScreen, Some(ModeKind::Photo)) => true,
            // Экран мог остаться в Calibrate с прошлого прогона: свайп не нужен, ждём только приборы.
            (StageBudgetSurface::CalibrateScreen, Some(ModeKind::Photo | ModeKind::Calibrate)) => true,
            _ => false,
        };
        let ready = view.screen_installed && view.presence_live && view.ui_overlay_active && warmed && mode_ok;
        if !ready {
            return self.fail_if_late(now, || {
                let mut missing: Vec<String> = Vec::new();
                if !view.screen_installed {
                    missing.push("экран камеры не стоит на рендере".to_owned());
                }
                if !view.presence_live {
                    missing.push("экран не видит живую камеру".to_owned());
                }
                if !view.ui_overlay_active {
                    missing.push("источник UI не работает".to_owned());
                }
                if !warmed {
                    missing.push(format!(
                        "экран нарисовал {} кадров из {}",
                        view.screen_frames.saturating_sub(frames_at_start),
                        self.warmup_ui_frames
                    ));
                }
                if !mode_ok {
                    missing.push(format!("режим экрана {:?}, нужен Photo", view.mode));
                }
                format!("экран не вышел на место замера: {}", missing.join("; "))
            });
        }
        if self.surface == StageBudgetSurface::PhotoScreen {
            return EntryTick::Ready;
        }
        let passes_at_gesture = view.scopes.map_or(0, |scopes| scopes.passes_recorded);
        self.phase = EntryPhase::AwaitingCalibrate { passes_at_gesture };
        self.phase_started_at = now;
        if view.mode == Some(ModeKind::Calibrate) {
            // Раздел уже открыт: жест не нужен, сразу ждём приборы (проверка ниже на следующем тике).
            return EntryTick::Waiting;
        }
        EntryTick::Gesture(SurfaceGesture {
            step: "modes_swipe_left",
            purpose: "свайп справа налево открывает раздел CALIBRATE: живой видоискатель, цветовой прибор, стекло UI",
            swipe: self.swipes.enter_calibrate,
        })
    }

    fn tick_awaiting_calibrate(&mut self, now: Instant, view: &SurfaceView, passes_at_gesture: u64) -> EntryTick {
        let in_calibrate = view.mode == Some(ModeKind::Calibrate);
        let scopes = view.scopes;
        let scopes_alive = scopes.is_some_and(|scopes| {
            scopes.requested && scopes.resources_live && scopes.passes_recorded > passes_at_gesture
        });
        if in_calibrate && scopes_alive {
            return EntryTick::Ready;
        }
        self.fail_if_late(now, || {
            let mut missing: Vec<String> = Vec::new();
            if !in_calibrate {
                missing.push(format!(
                    "режим экрана {:?}, нужен Calibrate (свайп не дошёл?)",
                    view.mode
                ));
            }
            match scopes {
                None => missing.push("в статусе рендера нет диагностики приборов".to_owned()),
                Some(scopes) => {
                    if !scopes.requested {
                        missing.push("экран не запросил прибор".to_owned());
                    }
                    if !scopes.resources_live {
                        missing.push("объекты GPU приборов не созданы".to_owned());
                    }
                    if scopes.passes_recorded <= passes_at_gesture {
                        missing.push("проходы приборов не записываются".to_owned());
                    }
                }
            }
            format!("раздел калибровки не стал живым: {}", missing.join("; "))
        })
    }

    fn fail_if_late(&self, now: Instant, reason: impl FnOnce() -> String) -> EntryTick {
        if now.saturating_duration_since(self.phase_started_at) >= self.timeout {
            EntryTick::Failed(format!("{} (за {} мс)", reason(), self.timeout.as_millis()))
        } else {
            EntryTick::Waiting
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitPhase {
    /// Жест ещё не выдавали.
    Pending,
    /// Жест выдан, ждём режим Photo.
    AwaitingPhoto,
}

/// Автомат возврата экрана в нейтральное состояние (Photo).
#[derive(Debug)]
pub struct SurfaceExit {
    surface: StageBudgetSurface,
    swipes: SurfaceSwipes,
    timeout: Duration,
    /// Раньше этого момента жест выхода не выдаётся: до него держится выданное сценарием нажатие пальца, а два
    /// «пальца» от двух процессов `input` конфликтуют (свайп проглатывается).
    not_before: Instant,
    phase: ExitPhase,
    phase_started_at: Instant,
}

impl SurfaceExit {
    /// `not_before` — конец удержания пальца плюс запас (`now`, если ничего не удерживается).
    pub fn begin(
        surface: StageBudgetSurface,
        swipes: SurfaceSwipes,
        timeout: Duration,
        now: Instant,
        not_before: Instant,
    ) -> Self {
        Self {
            surface,
            swipes,
            timeout,
            not_before,
            phase: ExitPhase::Pending,
            phase_started_at: now,
        }
    }

    /// Тик выхода.
    pub fn tick(&mut self, now: Instant, view: &SurfaceView) -> ExitTick {
        // Возвращать есть что только из CALIBRATE: PHOTO и голый видоискатель остаются как были.
        if self.surface != StageBudgetSurface::CalibrateScreen {
            return ExitTick::Done;
        }
        if view.mode == Some(ModeKind::Photo) {
            return ExitTick::Done;
        }
        match self.phase {
            ExitPhase::Pending => {
                if now < self.not_before {
                    return ExitTick::Waiting;
                }
                self.phase = ExitPhase::AwaitingPhoto;
                self.phase_started_at = now;
                ExitTick::Gesture(SurfaceGesture {
                    step: "modes_swipe_right",
                    purpose: "свайп слева направо по чистой зоне раздела возвращает экран в PHOTO",
                    swipe: self.swipes.leave_calibrate,
                })
            }
            ExitPhase::AwaitingPhoto => {
                if now.saturating_duration_since(self.phase_started_at) >= self.timeout {
                    ExitTick::Failed(format!(
                        "режим экрана {:?}, нужен Photo (за {} мс)",
                        view.mode,
                        self.timeout.as_millis()
                    ))
                } else {
                    ExitTick::Waiting
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "stage_budget_surface_tests.rs"]
mod tests;
