//! Источник кадра UI «экран камеры» (сессия 12): обёртка над `camera_screen::CameraScreen` для потока рендера.
//!
//! Экран сам ничего не знает ни о каналах, ни о движке камеры, ни о сервисах оболочки. Эта обёртка — «шов»,
//! который на КАЖДОМ показанном кадре делает по порядку:
//! 1. забирает вход из канала ([`ScreenInputReceiver`]): касания → `handle_touch`, действия → `handle_action`;
//! 2. снимает факты камеры; шлёт `CameraFactsChanged` ТОЛЬКО при реальном изменении;
//!    Факты о виде (список пресетов, ручки, проблема) идут тем же швом: [`LookFactsSource`] сравнивает снимок с
//!    последним отправленным и шлёт `LookFactsChanged` ТОЛЬКО при изменении (без библиотеки пресетов фактов нет);
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

use camera_contract::ids::LensFacing;
use camera_screen::focus::FocusIndicator;
use camera_screen::modes::CaptureMode;
use camera_screen::shutter::{ShotRequestId, ShutterState, ThumbnailState};
use camera_screen::thumbnail::PictureTiming;
use camera_screen::{
    CameraPresence, CameraScreen, PanelState, ScreenAction, ScreenDiagnostics, ScreenState, ShutterVisibility,
};
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;
use shell_contract::safe_area::SafeAreaInsets;
use ui_draw::draw_list::DrawList;
use ui_draw::frame_source::{UiFrameError, UiFrameRequest, UiFrameSource};

use crate::camera_screen_effects::{EffectCounters, EffectExecutor};
use crate::camera_screen_facts::{CameraFactsSource, CameraFactsWatcher, FactsChange, FactsDiagnostics};
use crate::camera_screen_gravity::{GravityFeed, GravityPoll};
use crate::camera_screen_input::{ScreenInput, ScreenInputReceiver, ScreenInputStats};
use crate::camera_screen_white_view::WhiteView;
use crate::look_facts::{LookFactsChange, LookFactsSource};
use crate::look_white_stops::LookWhiteStopsWatcher;

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
    /// Раздел калибровки вида (14b): ручки, пресеты, приборы; затвора и меню Controls в нём нет.
    Calibrate,
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
    /// Зум, который показывает экран (сессия 13).
    pub zoom: ZoomView,
    /// Безопасная зона, внутри которой экран раскладывает раздел калибровки (14k): что на самом деле сообщил Android.
    pub safe_area: SafeAreaInsets,
    /// Белое глазами экрана (white-balance 02): число, происхождение и счёт отказов.
    pub white: WhiteView,
}

/// Зум глазами отчёта: коэффициент (в тысячных, чтобы снимок оставался `Eq`), направление, открыто ли колесо.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ZoomView {
    /// Коэффициент × 1000, округлённый (0,593 → 593).
    pub ratio_milli: u32,
    pub facing: LensFacing,
    pub wheel_open: bool,
    /// Идёт плавный переход к упору после тапа по кнопке ленты (значение ещё не дошло до цели).
    pub gliding: bool,
}

impl ZoomView {
    fn observe(state: &ScreenState) -> Self {
        let zoom = state.zoom();
        Self {
            ratio_milli: (f64::from(zoom.ratio().value()) * 1000.0).round() as u32,
            facing: zoom.facing(),
            wheel_open: zoom.is_wheel_open(),
            gliding: zoom.is_gliding(),
        }
    }
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
                CaptureMode::Calibrate => ModeKind::Calibrate,
            },
            shutter_placement: match state.shutter_visibility() {
                ShutterVisibility::Visible => ShutterPlacement::Visible,
                ShutterVisibility::HiddenUnderDrawer => ShutterPlacement::HiddenUnderDrawer,
                ShutterVisibility::RaisedAboveCard => ShutterPlacement::RaisedAboveCard,
            },
            zoom: ZoomView::observe(state),
            safe_area: state.safe_area(),
            white: WhiteView::observe(state),
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
    /// Последний снимок для миниатюры (с картинкой, если она пришла).
    pub thumbnail: ThumbnailState,
    /// Замер «нажатие → картинка» последней показанной картинки (14g): момент нажатия и время кадра, на котором
    /// картинку впервые нарисовали; `None` — картинки с известным нажатием ещё не было.
    pub thumbnail_timing: Option<PictureTiming>,
    /// Наблюдаемое состояние экрана (панель, плашка, шаги EV, режим, затвор) на момент последнего кадра.
    pub state: ScreenStateSnapshot,
    /// Настройки съёмки, которые экран применит к СЛЕДУЮЩЕМУ снимку (RAW, формат кадра…): сценарий `capture`
    /// сверяет по ним, что значок RAW действительно переключился.
    pub capture_settings: camera_screen::settings::CaptureSettings,
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
    /// Сколько раз экрану отправлены изменившиеся факты о виде.
    pub look_facts_sent: u64,
    /// Сколько раз экрану отправлены изменившиеся упоры ручек теплоты и оттенка.
    pub look_white_stops_sent: u64,
    /// Сколько раз факты о виде пересобирались (тихие кадры без правок его не двигают).
    pub look_facts_rebuilds: u64,
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
    /// Источник фактов о виде для экрана калибровки; `None` — в сборке нет библиотеки пресетов (экран фактов не
    /// получает, ручки у него отключены).
    pub look_facts: Option<LookFactsSource>,
    /// Наблюдатель за упорами ручек теплоты и оттенка (решения 288, 293); `silent()` — упоров не будет.
    pub look_white_stops: LookWhiteStopsWatcher,
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
    look_facts: Option<LookFactsSource>,
    look_white_stops: LookWhiteStopsWatcher,
    gravity: GravityLink,
    executor: EffectExecutor,
    board: Arc<ScreenBoard>,
    facts_sent: u64,
    /// Сколько раз экрану отправлены изменившиеся факты о виде.
    look_facts_sent: u64,
    /// Сколько раз экрану отправлены изменившиеся упоры ручек теплоты и оттенка.
    look_white_stops_sent: u64,
    gravity_samples_sent: u64,
    report_min_interval: Duration,
    /// Экран ещё не прошёл первый кадр после установки: на нём исполнитель приводит показ вида к рецепту библиотеки
    /// (см. [`EffectExecutor::on_screen_installed`]).
    awaiting_first_frame: bool,
    /// Время кадра последней публикации отчёта; `None` — ещё не публиковали.
    last_published_at: Option<MonotonicNanos>,
    /// Снимок, чьё «миниатюра показана» уже записано в лог: строка идёт один раз на картинку.
    logged_picture_shot: Option<ShotRequestId>,
}

/// Что случилось в кадре: из этого решается, публиковать ли отчёт немедленно.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct FrameActivity {
    input_applied: bool,
    facts_changed: bool,
    effects_executed: bool,
    /// Картинка миниатюры впервые попала на кадр: отчёт с замером нужен сразу, а не по интервалу.
    picture_shown: bool,
}

impl FrameActivity {
    fn any(self) -> bool {
        self.input_applied || self.facts_changed || self.effects_executed || self.picture_shown
    }
}

impl CameraScreenSource {
    pub fn new(parts: ScreenSourceParts) -> Self {
        Self {
            screen: parts.screen,
            input: parts.input,
            facts: parts.facts,
            facts_watcher: CameraFactsWatcher::new(),
            look_facts: parts.look_facts,
            look_white_stops: parts.look_white_stops,
            gravity: parts.gravity,
            executor: parts.executor,
            board: parts.board,
            facts_sent: 0,
            look_facts_sent: 0,
            look_white_stops_sent: 0,
            gravity_samples_sent: 0,
            report_min_interval: parts.report_min_interval,
            awaiting_first_frame: true,
            last_published_at: None,
            logged_picture_shot: None,
        }
    }

    /// Состояние экрана для чтения (диагностика и сквозные тесты: «что экран показывает о виде»). Менять экран
    /// снаружи нельзя: он живёт на действиях из канала входа.
    pub fn screen_state(&self) -> &ScreenState {
        self.screen.state()
    }

    /// Где круг колеса цвета стоит на поверхности сейчас (`None`, если вид колёс не открыт или круг не помещается).
    /// Нужен сквозным тестам, которые ведут настоящий палец по кругу; ответ считает сам экран по той же раскладке,
    /// по которой он рисует круг, поэтому разойтись с нарисованным он не может.
    pub fn wheel_pad_placement(&self) -> Option<camera_screen::WheelPadPlacement> {
        self.screen.wheel_pad_placement()
    }

    /// Цели касаний белого (значок, карточка) по текущему состоянию экрана; `None` — раскладки ещё нет. Сквозные
    /// тесты берут точки у экрана, а не считают их сами: разойтись с нарисованным они не могут.
    pub fn white_touch_targets(&self) -> Option<camera_screen::WhiteTouchTargets> {
        self.screen.white_touch_targets()
    }

    /// Цели касаний раздела калибровки (линейки строк, «↻», точки полос, окно списка); `None` — раздела на экране
    /// нет. Сквозные тесты ведут настоящий палец по этим точкам: взять их из другого места они не могут.
    pub fn calibration_touch_targets(&self) -> Option<camera_screen::CalibrationTouchTargets> {
        self.screen.calibration_touch_targets()
    }

    /// Шаг 1: вход из канала. Забираем всё, что накопилось к этому кадру (очередь ограничена конфигом).
    fn apply_pending_input(&mut self) -> bool {
        let mut any = false;
        while let Some(input) = self.input.try_next() {
            any = true;
            match input {
                ScreenInput::Touch(touch) => self.screen.handle_touch(touch),
                ScreenInput::Action(action) => self.screen.handle_action(*action),
                ScreenInput::SavedWithPicture { request, picture } => {
                    self.screen.handle_saved_with_picture(request, picture);
                }
                ScreenInput::SystemGestureZones(zones) => self.screen.set_system_gesture_zones(zones),
            }
        }
        any
    }

    /// Шаг 2: факты камеры — только если изменились.
    fn sync_camera_facts(&mut self) -> bool {
        if let FactsChange::Changed(facts) = self.facts_watcher.observe(self.facts.current_facts()) {
            self.facts_sent += 1;
            self.screen.handle_action(ScreenAction::CameraFactsChanged(*facts));
            return true;
        }
        false
    }

    /// Шаг 2б: факты о виде — только если изменились (список пресетов, правки, основа, проблема).
    fn sync_look_facts(&mut self) -> bool {
        let Some(source) = &mut self.look_facts else {
            return false;
        };
        // Проблема запуска (файл пресетов сброшен / сохранения отключены): отдаётся один раз, плашку при входе в
        // раздел калибровки поднимает сам экран.
        let startup_problem = source.take_startup_problem();
        let startup_problem_sent = startup_problem.is_some();
        if let Some(kind) = startup_problem {
            self.screen.handle_action(ScreenAction::LookStartupProblem(kind));
        }
        let facts_changed = match source.observe(self.executor.latest_look_problem()) {
            LookFactsChange::Changed(facts) => {
                self.look_facts_sent += 1;
                self.screen.handle_action(ScreenAction::LookFactsChanged(facts));
                true
            }
            LookFactsChange::Unchanged => false,
        };
        facts_changed || startup_problem_sent
    }

    /// Шаг 2в: упоры ручек теплоты и оттенка — только если рендер сообщил новые.
    fn sync_look_white_stops(&mut self) -> bool {
        let Some(stops) = self.look_white_stops.changed() else {
            return false;
        };
        self.look_white_stops_sent += 1;
        self.screen.handle_action(ScreenAction::LookWhiteStopsChanged(stops));
        true
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
        self.executor.resend_pending_white();
        // Пауза слежения за белым — уровень режима: сверяется каждый кадр, а не только когда есть эффекты.
        self.executor
            .drive_white_tracking_hold(self.screen.state().white_tracking_hold());
        let effects = self.screen.drain_effects();
        if effects.is_empty() {
            return false;
        }
        for feedback in self.executor.execute(effects) {
            self.screen.handle_action(feedback);
        }
        true
    }

    /// Шаг 4б: картинка миниатюры впервые нарисована — одна строка лога «нажатие → картинка».
    /// Лог здесь, а не в экране: `camera-screen` без ввода-вывода. `true` — строка записана в этом кадре.
    fn log_picture_if_just_shown(&mut self) -> bool {
        let Some(timing) = self.screen.state().picture_timing() else {
            return false;
        };
        if timing.shown_at.is_none() || self.logged_picture_shot == Some(timing.shot) {
            return false;
        }
        self.logged_picture_shot = Some(timing.shot);
        tracing::info!(
            target: "fasticamera::camera_screen",
            shot = ?timing.shot,
            press_to_picture_ms = timing.press_to_picture_ms(),
            "миниатюра показана"
        );
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
            thumbnail_timing: self.screen.state().picture_timing(),
            state: ScreenStateSnapshot::observe(self.screen.state()),
            capture_settings: *self.screen.state().settings(),
            presence: self.screen.state().camera_presence(),
            icon_rotation_clockwise_degrees: self.screen.state().icon_rotation_clockwise_degrees(),
            effects: self.executor.counters(),
            input: self.input.stats(),
            facts_sent: self.facts_sent,
            look_facts_sent: self.look_facts_sent,
            look_white_stops_sent: self.look_white_stops_sent,
            look_facts_rebuilds: self.look_facts.as_ref().map_or(0, LookFactsSource::rebuilds),
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
        if self.awaiting_first_frame {
            // Новый экран: рендеру — действующий вид библиотеки (прежний экран мог уйти с зажатым «чистяком»).
            self.awaiting_first_frame = false;
            self.executor.on_screen_installed();
        }
        // Если рендер не принял вид (установка, отпускание «чистяка»), повторяем, пока не примет.
        self.executor.retry_look_render_if_unsure();
        let mut activity = FrameActivity {
            input_applied: self.apply_pending_input(),
            ..FrameActivity::default()
        };
        activity.facts_changed = self.sync_camera_facts();
        activity.facts_changed |= self.sync_look_facts();
        activity.facts_changed |= self.sync_look_white_stops();
        self.feed_gravity();
        let frame = self.screen.produce_frame(request);
        activity.picture_shown = self.log_picture_if_just_shown();
        // Эффекты исполняем и при отказе кадра: действие пользователя (например, тап по затвору) уже случилось.
        activity.effects_executed = self.run_effects();
        if activity.effects_executed {
            // Эффекты кадра могли изменить библиотеку или породить проблему: экран узнаёт об этом сразу.
            activity.facts_changed |= self.sync_look_facts();
        }
        if self.report_is_due(request.present_time, activity) {
            self.publish_report(request.present_time);
        }
        frame
    }
}

#[cfg(test)]
#[path = "camera_screen_source_tests.rs"]
mod tests;
