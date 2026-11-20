//! Источник кадра UI «экран камеры» (сессия 12): обёртка над `camera_screen::CameraScreen` для потока рендера.
//!
//! Экран сам ничего не знает ни о каналах, ни о движке камеры, ни о сервисах оболочки. Эта обёртка — «шов»,
//! который на КАЖДОМ показанном кадре делает по порядку:
//! 1. забирает вход из канала ([`ScreenInputReceiver`]): касания → `handle_touch`, действия → `handle_action`;
//! 2. снимает факты камеры; шлёт `CameraFactsChanged` ТОЛЬКО при реальном изменении;
//! 3. подаёт гравитацию (с прореживанием) → `GravityChanged`;
//! 4. просит у экрана кадр (`produce_frame`);
//! 5. забирает эффекты (`drain_effects`) и исполняет их ([`EffectExecutor`]); отвергнутый снимок возвращается
//!    экрану действием `CaptureFinished(Failed)`;
//! 6. публикует отчёт на «доску» ([`ScreenBoard`]) — оттуда его читают сценарии и тесты с другого потока.
//!    Не на каждом кадре: отчёт несёт клон диагностики (строки), а на 120 Гц это лишний мусор. Публикуем сразу, если
//!    в кадре был вход, новые факты или эффекты (состояние изменилось по делу), иначе не чаще
//!    `camera_screen.report_min_interval_ms` по часам кадров.
//!
//! Всё без ожиданий (правило потока рендера): каналы — `try_*`, датчик — чтение ящика, камера — снимок статуса.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use camera_screen::focus::FocusIndicator;
use camera_screen::modes::CaptureMode;
use camera_screen::shutter::{ShutterState, ThumbnailState};
use camera_screen::{
    CameraPresence, CameraScreen, PanelState, ScreenAction, ScreenDiagnostics, ScreenState, ShutterVisibility,
};
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};

use crate::camera_screen_effects::{EffectCounters, EffectExecutor};
use crate::camera_screen_facts::{CameraFactsSource, CameraFactsWatcher, FactsChange, FactsDiagnostics};
use crate::camera_screen_gravity::{GravityFeed, GravityPoll};
use crate::camera_screen_input::{ScreenInput, ScreenInputReceiver, ScreenInputStats};

/// Имя источника в диагностике рендера (`UiOverlayDiagnostics::active_source`).
pub const CAMERA_SCREEN_SOURCE_NAME: &str = "camera_screen";

/// Откуда экран берёт гравитацию.
pub enum GravityLink {
    /// Подача работает.
    Active(GravityFeed),
    /// Датчик не запустился (причина): экран живёт без наклона.
    Unavailable { reason: String },
}

/// Какая панель открыта на экране (нейтральная копия `PanelState` для отчёта: без инструмента подменю).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PanelKind {
    Closed,
    /// Меню Controls.
    ControlsMenu,
    /// Подменю какого-то инструмента.
    Submenu,
    /// Панель Exposure.
    ExposurePanel,
}

/// Виден ли затвор (решение 94).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ShutterPlacement {
    Visible,
    /// Меню Controls открыто: затвор ушёл под шторку.
    HiddenUnderDrawer,
    /// Открыта карточка инструмента (решение 103): ряд затвора поднят на 100 pt, затвор виден и снимает.
    RaisedAboveCard,
}

/// Что делает затвор (вид состояния без внутренних номеров запросов и времён).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ShutterActivity {
    Idle,
    Countdown,
    Capturing,
    CapturingLong,
    Burst,
    Recording,
    RecordingLocked,
}

/// Текущий режим съёмки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ModeKind {
    Photo,
    Video,
    Portrait,
}

/// Показан ли элемент экрана (плашка блокировки, индикатор фокуса).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Presence {
    Hidden,
    Shown,
}

impl Presence {
    fn from_visible(visible: bool) -> Self {
        if visible { Presence::Shown } else { Presence::Hidden }
    }
}

/// Снимок наблюдаемого состояния экрана: то, что пользователь видит на экране, без внутренностей
/// `ScreenState`. Сценарий `screen` доказывает по нему «меню открыто», «плашка видна» напрямую, а не по
/// следствиям. Читается только через публичные методы экрана (`ScreenStateSnapshot::observe`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ScreenStateSnapshot {
    pub panel: PanelKind,
    /// Плашка «AE/AF LOCK» (камера подтвердила блокировку).
    pub lock_plaque: Presence,
    /// Квадрат индикатора фокуса.
    pub focus_indicator: Presence,
    /// Шаги шкалы EV экрана (1/3 EV), 0 — нейтрально.
    pub exposure_steps: i32,
    pub mode: ModeKind,
    pub shutter_placement: ShutterPlacement,
    pub shutter_activity: ShutterActivity,
}

impl ScreenStateSnapshot {
    /// Снимок по состоянию экрана. `match` без `_`: новый вариант в `camera-screen` не пройдёт мимо отчёта.
    pub fn observe(state: &ScreenState) -> Self {
        Self {
            panel: match state.panel() {
                PanelState::Closed => PanelKind::Closed,
                PanelState::ControlsMenu => PanelKind::ControlsMenu,
                PanelState::Submenu(_) => PanelKind::Submenu,
                PanelState::ExposurePanel => PanelKind::ExposurePanel,
            },
            lock_plaque: Presence::from_visible(state.focus().is_lock_plaque_visible()),
            focus_indicator: match state.focus().indicator() {
                FocusIndicator::Hidden => Presence::Hidden,
                FocusIndicator::Shown { .. } => Presence::Shown,
            },
            exposure_steps: state.exposure().steps(),
            mode: match state.mode() {
                CaptureMode::Photo => ModeKind::Photo,
                CaptureMode::Video => ModeKind::Video,
                CaptureMode::Portrait => ModeKind::Portrait,
            },
            shutter_placement: match state.shutter_visibility() {
                ShutterVisibility::Visible => ShutterPlacement::Visible,
                ShutterVisibility::HiddenUnderDrawer => ShutterPlacement::HiddenUnderDrawer,
                ShutterVisibility::RaisedAboveCard => ShutterPlacement::RaisedAboveCard,
            },
            shutter_activity: match state.shutter() {
                ShutterState::Idle => ShutterActivity::Idle,
                ShutterState::Countdown { .. } => ShutterActivity::Countdown,
                ShutterState::Capturing { .. } => ShutterActivity::Capturing,
                ShutterState::CapturingLong { .. } => ShutterActivity::CapturingLong,
                ShutterState::Burst { .. } => ShutterActivity::Burst,
                ShutterState::Recording { .. } => ShutterActivity::Recording,
                ShutterState::RecordingLocked { .. } => ShutterActivity::RecordingLocked,
            },
        }
    }
}

/// Отчёт экрана: всё, что нужно сценарию и тесту, чтобы судить о работе экрана снаружи потока рендера.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenReport {
    /// Диагностика самого экрана (жесты, действия, эффекты, кадры).
    pub diagnostics: ScreenDiagnostics,
    /// Последний снимок для миниатюры.
    pub thumbnail: ThumbnailState,
    /// Наблюдаемое состояние экрана (панель, плашка, шаги EV, режим, затвор) на момент последнего кадра.
    pub state: ScreenStateSnapshot,
    /// Присутствие камеры глазами экрана.
    pub presence: CameraPresence,
    /// Целевой поворот значков по ориентации телефона, градусы по часовой.
    pub icon_rotation_clockwise_degrees: f32,
    /// Счётчики исполнителя эффектов.
    pub effects: EffectCounters,
    /// Счётчики канала входа.
    pub input: ScreenInputStats,
    /// Сколько раз экрану отправлены изменившиеся факты камеры.
    pub facts_sent: u64,
    /// Счётчики источника фактов (сборка паспорта потока, потоки без паспорта).
    pub facts: FactsDiagnostics,
    /// Сколько показаний гравитации отдано экрану.
    pub gravity_samples_sent: u64,
    /// Почему гравитации нет (`None` — подача работает).
    pub gravity_unavailable: Option<String>,
}

/// Доска отчёта: поток рендера пишет, остальные читают. Замок короткий, без вызовов наружу.
#[derive(Debug, Default)]
pub struct ScreenBoard {
    report: Mutex<Option<ScreenReport>>,
}

impl ScreenBoard {
    fn lock(&self) -> MutexGuard<'_, Option<ScreenReport>> {
        self.report.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Последний отчёт; `None` — экран ещё не нарисовал ни одного кадра.
    pub fn latest(&self) -> Option<ScreenReport> {
        self.lock().clone()
    }

    fn publish(&self, report: ScreenReport) {
        *self.lock() = Some(report);
    }
}

/// Части источника (собирает сервис экрана).
pub struct ScreenSourceParts {
    pub screen: CameraScreen,
    pub input: ScreenInputReceiver,
    pub facts: Box<dyn CameraFactsSource>,
    pub gravity: GravityLink,
    pub executor: EffectExecutor,
    pub board: Arc<ScreenBoard>,
    /// Минимальный интервал между публикациями «без событий» (0 — каждый кадр).
    pub report_min_interval: Duration,
}

/// Источник кадра UI «экран камеры».
pub struct CameraScreenSource {
    screen: CameraScreen,
    input: ScreenInputReceiver,
    facts: Box<dyn CameraFactsSource>,
    facts_watcher: CameraFactsWatcher,
    gravity: GravityLink,
    executor: EffectExecutor,
    board: Arc<ScreenBoard>,
    facts_sent: u64,
    gravity_samples_sent: u64,
    report_min_interval: Duration,
    /// Время кадра последней публикации отчёта; `None` — ещё не публиковали.
    last_published_at: Option<MonotonicNanos>,
}

/// Что случилось в кадре: из этого решается, публиковать ли отчёт немедленно.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct FrameActivity {
    input_applied: bool,
    facts_changed: bool,
    effects_executed: bool,
}

impl FrameActivity {
    fn any(self) -> bool {
        self.input_applied || self.facts_changed || self.effects_executed
    }
}

impl CameraScreenSource {
    pub fn new(parts: ScreenSourceParts) -> Self {
        Self {
            screen: parts.screen,
            input: parts.input,
            facts: parts.facts,
            facts_watcher: CameraFactsWatcher::new(),
            gravity: parts.gravity,
            executor: parts.executor,
            board: parts.board,
            facts_sent: 0,
            gravity_samples_sent: 0,
            report_min_interval: parts.report_min_interval,
            last_published_at: None,
        }
    }

    /// Шаг 1: вход из канала. Забираем всё, что накопилось к этому кадру (очередь ограничена конфигом).
    fn apply_pending_input(&mut self) -> bool {
        let mut any = false;
        while let Some(input) = self.input.try_next() {
            any = true;
            match input {
                ScreenInput::Touch(touch) => self.screen.handle_touch(touch),
                ScreenInput::Action(action) => self.screen.handle_action(action),
                ScreenInput::SystemGestureZones(zones) => self.screen.set_system_gesture_zones(zones),
            }
        }
        any
    }

    /// Шаг 2: факты камеры — только если изменились.
    fn sync_camera_facts(&mut self) -> bool {
        if let FactsChange::Changed(facts) = self.facts_watcher.observe(self.facts.current_facts()) {
            self.facts_sent += 1;
            self.screen.handle_action(ScreenAction::CameraFactsChanged(facts));
            return true;
        }
        false
    }

    /// Шаг 3: гравитация с прореживанием.
    fn feed_gravity(&mut self) {
        let GravityLink::Active(feed) = &mut self.gravity else {
            return;
        };
        if let GravityPoll::Fresh(vector) = feed.poll() {
            self.gravity_samples_sent += 1;
            self.screen.handle_action(ScreenAction::GravityChanged(vector));
        }
    }

    /// Шаг 5: исполнить эффекты; обратная связь исполнителя возвращается экрану.
    fn run_effects(&mut self) -> bool {
        // Не принятое ранее желаемое EV идёт первым: свежее значение этого кадра пойдёт после и победит.
        self.executor.resend_pending_exposure_bias();
        let effects = self.screen.drain_effects();
        if effects.is_empty() {
            return false;
        }
        for feedback in self.executor.execute(effects) {
            self.screen.handle_action(feedback);
        }
        true
    }

    /// Пора ли публиковать отчёт: есть события кадра, это первая публикация или вышел интервал по часам кадров.
    fn report_is_due(&self, now: MonotonicNanos, activity: FrameActivity) -> bool {
        if activity.any() {
            return true;
        }
        let Some(last) = self.last_published_at else {
            return true;
        };
        let interval_nanos = i64::try_from(self.report_min_interval.as_nanos()).unwrap_or(i64::MAX);
        now.as_nanos().saturating_sub(last.as_nanos()) >= interval_nanos
    }

    /// Шаг 6: отчёт на доску.
    fn publish_report(&mut self, now: MonotonicNanos) {
        self.last_published_at = Some(now);
        self.board.publish(ScreenReport {
            diagnostics: self.screen.diagnostics(),
            thumbnail: self.screen.state().thumbnail(),
            state: ScreenStateSnapshot::observe(self.screen.state()),
            presence: self.screen.state().camera_presence(),
            icon_rotation_clockwise_degrees: self.screen.state().icon_rotation_clockwise_degrees(),
            effects: self.executor.counters(),
            input: self.input.stats(),
            facts_sent: self.facts_sent,
            facts: self.facts.diagnostics(),
            gravity_samples_sent: self.gravity_samples_sent,
            gravity_unavailable: match &self.gravity {
                GravityLink::Active(_) => None,
                GravityLink::Unavailable { reason } => Some(reason.clone()),
            },
        });
    }
}

impl UiFrameSource for CameraScreenSource {
    fn name(&self) -> &'static str {
        CAMERA_SCREEN_SOURCE_NAME
    }

    fn produce_frame(&mut self, request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        // Сначала часы кадра: созревший ответ фейк-снимка попадёт в вход ЭТОГО кадра.
        self.executor.begin_frame(request.present_time);
        let mut activity = FrameActivity {
            input_applied: self.apply_pending_input(),
            ..FrameActivity::default()
        };
        activity.facts_changed = self.sync_camera_facts();
        self.feed_gravity();
        let frame = self.screen.produce_frame(request);
        // Эффекты исполняем и при отказе кадра: действие пользователя (например, тап по затвору) уже случилось.
        activity.effects_executed = self.run_effects();
        if self.report_is_due(request.present_time, activity) {
            self.publish_report(request.present_time);
        }
        frame
    }
}

#[cfg(test)]
#[path = "camera_screen_source_tests.rs"]
mod tests;
