//! Общий стенд «палец → экран → движок → рендер → пиксели» (план white-balance, сессия 02, пакет D0).
//!
//! Стенд ДОКЛАДЫВАЕТ экран камеры к уже готовому стенду движка и рендера (`engine_stand::Stand`: настоящий движок на
//! fake-бэкенде + настоящий `VulkanDisplayBackend`):
//! - настоящий `CameraScreenSource` (экран, канал входа, исполнитель эффектов);
//! - порт к движку — `HandleCameraPort` на ручку ТОГО ЖЕ движка, что кормит рендер; факты экрана — `EngineFactsSource`;
//! - палец — те же `TouchSample`, что шлёт оболочка, через `ScreenInputSender`; время экрана ведёт стенд (кадр = 16 мс).
//!
//! Два независимых «такта»: кадр UI ([`PanelStand::ui_frames`]) и кадр камеры ([`PanelStand::run_camera_frames`], он же
//! прогон `Stand::run_frames`). Их чередование — [`PanelStand::alternate`]. UI на кадр камеры НЕ накладывается:
//! эталон — чистый кадр видоискателя; экран исполняет эффекты и рисует свой список, который тест читает.

use std::sync::Arc;
use std::time::Duration;

use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::white_reading::WhiteReadingBoard;
use camera_contract::zoom::LensIntentChannel;
use camera_core::CameraHandle;
use camera_core::status::CameraStatus;
use camera_screen::{CameraScreen, ScreenAction, ScreenConfig, ScreenTouch};
use fasticamera::camera_screen_effects::{EffectCounters, EffectExecutor, EffectTargets, HandleCameraPort};
use fasticamera::camera_screen_facts::EngineFactsSource;
use fasticamera::camera_screen_input::{InputDelivery, ScreenInputSender, screen_input_channel};
use fasticamera::camera_screen_service::SilentShutterSound;
use fasticamera::camera_screen_source::{CameraScreenSource, GravityLink, ScreenBoard, ScreenSourceParts};
use shell_contract::clock::MonotonicNanos;
use shell_contract::haptics::HapticPattern;
use shell_contract::surface::SurfaceSize;
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_core::geometry::Point;
use ui_draw::draw_list::{DrawItem, DrawList, IconItem, TextItem};
use ui_draw::frame_source::{BackdropContinuity, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;
use ui_draw::icons::IconId;
use ui_draw::light::LightDirection;

use super::engine_stand::{Stand, StandOptions};
use super::lagging_facts::{FactsFreeze, FreezableFacts};
use super::screen_parts::{NoCapture, QuietCommands, ScreenSideRecords};
use super::white_rig::FrameImage;

/// Размер экрана телефона в пикселях (как в остальных сквозных тестах экрана).
const SURFACE: SurfaceSize = SurfaceSize {
    width_px: 1080,
    height_px: 2412,
};
/// Ширина экрана в pt эталона (раскладка iOS): 1 pt = ширина / 440 px.
const SURFACE_WIDTH_PT: f32 = 440.0;
/// Шаг времени кадров UI, мс (60 Гц).
const UI_FRAME_MS: i64 = 16;
/// Сколько кадров UI длится короткий тап (палец лежит ≈ 50 мс, как в тестах экрана).
const TAP_HOLD_FRAMES: usize = 3;
/// Сколько касаний канал входа держит в очереди.
const INPUT_QUEUE_CAPACITY: usize = 64;

/// Куда экран шлёт намерения камере.
pub enum CameraPort {
    /// Настоящий порт на ручку движка этого стенда.
    LiveEngine,
    /// Движка «нет»: каждое намерение камере — `NoEngine` (ловушка теста: без порта швы не работают).
    Absent,
}

pub struct PanelStand {
    /// Движок + рендер + эталонные прогоны кадров камеры.
    pub engine: Stand,
    /// Рубильник «экран видит прежние факты» (гонка факты ↔ палец).
    pub facts_freeze: FactsFreeze,
    source: CameraScreenSource,
    sender: ScreenInputSender,
    board: Arc<ScreenBoard>,
    side: ScreenSideRecords,
    last_ui: DrawList,
    ui_frame_index: i64,
}

impl PanelStand {
    /// `None` — GPU недоступен и тест осознанно пропущен (как у `Stand::start`). Доска показаний своя и пустая.
    pub fn start(options: StandOptions, port: CameraPort) -> Option<Self> {
        Self::start_with_readings(options, port, WhiteReadingBoard::new())
    }

    /// Стенд с доской показаний источников белого, которую тест держит у себя (как корень композиции держит её у
    /// `CameraServices`): экран читает с неё показание «≈ N» для кнопки «Phone».
    pub fn start_with_readings(options: StandOptions, port: CameraPort, readings: WhiteReadingBoard) -> Option<Self> {
        Self::start_with_screen_config(options, port, readings, ScreenConfig::default())
    }

    /// То же со своим конфигом экрана (например, кнопка белого слева от затвора).
    pub fn start_with_screen_config(
        options: StandOptions,
        port: CameraPort,
        readings: WhiteReadingBoard,
        screen_config: ScreenConfig,
    ) -> Option<Self> {
        let engine = Stand::start(options)?;
        let side = ScreenSideRecords::new();
        let facts_freeze = FactsFreeze::default();
        let (sender, receiver) = screen_input_channel(INPUT_QUEUE_CAPACITY);
        let board = Arc::new(ScreenBoard::default());
        let camera: Box<dyn fasticamera::camera_screen_effects::CameraIntentPort> = match port {
            CameraPort::LiveEngine => Box::new(HandleCameraPort::new(engine.handle())),
            CameraPort::Absent => Box::new(fasticamera::camera_screen_effects::AbsentCameraPort),
        };
        let executor = EffectExecutor::new(EffectTargets {
            camera,
            haptics: Arc::clone(&side.haptics) as _,
            commands: Arc::new(QuietCommands),
            render: Arc::clone(&side.render) as _,
            capture: Arc::new(NoCapture),
            shutter_sound: Arc::new(SilentShutterSound::default()),
            lens: LensIntentChannel::new(),
            look: None,
        });
        let screen = CameraScreen::new(
            screen_config,
            ui_kit::tokens::UiKitTokens::default(),
            ui_core::time::MotionPreference::Full,
        )
        .expect("экран собирается из конфига теста");
        let source = CameraScreenSource::new(ScreenSourceParts {
            look_white_stops: fasticamera::look_white_stops::LookWhiteStopsWatcher::silent(),
            scope_labels: fasticamera::scope_labels::ScopeLabelsWatcher::silent(),
            screen,
            input: receiver,
            facts: Box::new(FreezableFacts::new(
                EngineFactsSource::with_white_readings(engine.handle(), readings),
                facts_freeze.clone(),
            )),
            look_facts: None,
            gravity: GravityLink::Unavailable {
                reason: "стенд панели белого".to_owned(),
            },
            executor,
            board: Arc::clone(&board),
            report_min_interval: Duration::ZERO,
        });
        sender.send_action(ScreenAction::SurfaceResized(SURFACE));
        let mut stand = Self {
            engine,
            facts_freeze,
            source,
            sender,
            board,
            side,
            last_ui: DrawList::new(LightDirection::from_angle_degrees(45.0)),
            ui_frame_index: 0,
        };
        // Первый кадр: экран разбирает размер поверхности и строит раскладку (до этого цели касаний не посчитать).
        stand.ui_frames(1);
        Some(stand)
    }

    /// Ручка движка этого стенда (для команд «в обход экрана»).
    pub fn handle(&self) -> CameraHandle {
        self.engine.handle()
    }

    pub fn status(&self) -> CameraStatus {
        self.engine.status()
    }

    /// Ждёт состояния движка по условию (`Stand::wait_status`).
    pub fn wait_status(&self, what: &str, matches: impl Fn(&CameraStatus) -> bool) -> CameraStatus {
        self.engine.wait_status(what, matches)
    }

    /// Возобновление приложения: движок открывает камеру, рендер получает поток; экран «видит живую камеру» с
    /// ближайших кадров UI.
    pub fn resume(&mut self) {
        self.engine.resume();
        self.ui_frames(2);
    }

    /// Уход приложения в фон: движок закрывает камеру, поток рендера останавливается.
    pub fn pause(&mut self) {
        self.engine.pause();
        self.ui_frames(2);
    }

    // ---- палец ----

    /// Время СЛЕДУЮЩЕГО кадра UI: касание получает его метку, поэтому оно разбирается на ближайшем кадре и не
    /// «из будущего».
    fn next_frame_time(&self) -> MonotonicNanos {
        MonotonicNanos::from_nanos((self.ui_frame_index + 1) * UI_FRAME_MS * 1_000_000)
    }

    fn touch(&mut self, phase: TouchPhase, point_pt: Point) -> InputDelivery {
        let px_per_pt = f64::from(SURFACE.width_px) / f64::from(SURFACE_WIDTH_PT);
        let time = self.next_frame_time();
        self.sender.send_touch(ScreenTouch::Sample(TouchSample {
            pointer: PointerId::new(0),
            phase,
            position: SurfacePoint {
                x_px: (point_pt.x * px_per_pt) as f32,
                y_px: (point_pt.y * px_per_pt) as f32,
            },
            time,
        }))
    }

    /// Палец лёг в точку (pt экрана).
    pub fn touch_down(&mut self, point_pt: Point) {
        assert_eq!(self.touch(TouchPhase::Began, point_pt), InputDelivery::Queued);
    }

    /// Палец сдвинулся в точку (pt экрана).
    pub fn touch_move(&mut self, point_pt: Point) {
        assert_eq!(self.touch(TouchPhase::Moved, point_pt), InputDelivery::Queued);
    }

    /// Палец оторван в точке (pt экрана).
    pub fn touch_up(&mut self, point_pt: Point) {
        assert_eq!(self.touch(TouchPhase::Ended, point_pt), InputDelivery::Queued);
    }

    /// Короткий тап: палец лежит ≈ 50 мс времени экрана, потом отрыв и кадр, на котором экран его разбирает.
    pub fn tap(&mut self, point_pt: Point) {
        self.hold_for_frames(point_pt, TAP_HOLD_FRAMES);
        self.ui_frames(1);
    }

    /// Палец лежит `hold_ms` мс ВРЕМЕНИ ЭКРАНА (кадры идут по 16 мс, реального ожидания нет), потом отрыв.
    pub fn hold(&mut self, point_pt: Point, hold_ms: i64) {
        let frames = usize::try_from((hold_ms + UI_FRAME_MS - 1) / UI_FRAME_MS).expect("время неотрицательно");
        self.hold_for_frames(point_pt, frames);
        self.ui_frames(1);
    }

    fn hold_for_frames(&mut self, point_pt: Point, frames: usize) {
        self.touch_down(point_pt);
        self.ui_frames(frames);
        self.touch_up(point_pt);
    }

    /// Протяжка пальца из `from` в `to` за `duration_ms` мс времени экрана: шаги по кадру, потом отрыв.
    pub fn drag(&mut self, from: Point, to: Point, duration_ms: i64) {
        self.touch_down(from);
        let steps = (duration_ms / UI_FRAME_MS).max(1);
        for step in 1..=steps {
            let progress = step as f64 / steps as f64;
            self.touch_move(Point::new(
                from.x + (to.x - from.x) * progress,
                from.y + (to.y - from.y) * progress,
            ));
            self.ui_frames(1);
        }
        self.touch_up(to);
        self.ui_frames(1);
    }

    // ---- кадры ----

    /// `count` кадров UI подряд: экран разбирает вход, факты движка и исполняет эффекты (намерения камере уходят в
    /// движок на этих кадрах).
    pub fn ui_frames(&mut self, count: usize) {
        for _ in 0..count {
            self.ui_frame_index += 1;
            let request = UiFrameRequest {
                present_time: MonotonicNanos::from_nanos(self.ui_frame_index * UI_FRAME_MS * 1_000_000),
                logical_size: PxSize::new(SURFACE.width_px as f32, SURFACE.height_px as f32),
                px_per_pt: SURFACE.width_px as f32 / SURFACE_WIDTH_PT,
                backdrop: BackdropContinuity::Live,
            };
            self.last_ui = self.source.produce_frame(&request).expect("экран строит кадр");
        }
    }

    /// Кадры камеры: кадр в рендер, его результат в движок (`Stand::run_frames`). Возвращает пиксели показанных кадров.
    pub fn run_camera_frames(&mut self, count: i64, hints: &dyn Fn(i64) -> CaptureColorHints) -> Vec<FrameImage> {
        self.engine.run_frames(count, hints)
    }

    /// Чередование «кадр UI, кадр камеры» `count` раз: экран и камера живут вместе, как на телефоне.
    pub fn alternate(&mut self, count: i64, hints: &dyn Fn(i64) -> CaptureColorHints) -> Vec<FrameImage> {
        let mut images = Vec::new();
        for _ in 0..count {
            self.ui_frames(1);
            images.extend(self.engine.run_frames(1, hints));
        }
        images
    }

    // ---- что видно ----

    /// Список рисования UI последнего кадра.
    pub fn ui_list(&self) -> &DrawList {
        &self.last_ui
    }

    /// Все тексты последнего кадра UI (прогон, цвет, позиция в `transform`); смысл строки определяют сравнением
    /// прогона с раскладкой нужной строки ролью экрана (как `texts_of` в тестах `camera-screen`).
    pub fn ui_texts(&self) -> Vec<&TextItem> {
        self.last_ui
            .leaf_items()
            .filter_map(|item| match item {
                DrawItem::Text(text) => Some(text),
                _ => None,
            })
            .collect()
    }

    /// Видимые (непрозрачность заметна) значки `icon` последнего кадра UI.
    pub fn ui_icons(&self, icon: IconId) -> Vec<IconItem> {
        self.last_ui
            .leaf_items()
            .filter_map(|item| match item {
                DrawItem::Icon(drawn) if drawn.icon == icon && drawn.opacity.get() > 0.05 => Some(*drawn),
                _ => None,
            })
            .collect()
    }

    /// Цели касаний белого от НАСТОЯЩЕГО экрана (значок, карточка): стенд ничего не считает сам.
    pub fn white_targets(&self) -> camera_screen::WhiteTouchTargets {
        self.source.white_touch_targets().expect("раскладка экрана построена")
    }

    /// Цели касаний раздела калибровки от НАСТОЯЩЕГО экрана (капсула белого, карточка белого в разделе).
    pub fn calibration_targets(&self) -> camera_screen::CalibrationTouchTargets {
        self.source
            .calibration_touch_targets()
            .expect("раздел калибровки построен")
    }

    /// Действие экрана «в обход пальца» (поворот телефона и т. п.); применится на ближайшем кадре UI.
    pub fn send_action(&self, action: ScreenAction) {
        self.sender.send_action(action);
    }

    /// Зоны системных жестов (пиксели поверхности): касание, начатое в зоне, принадлежит системе.
    pub fn set_system_gesture_zones(&self, zones: shell_contract::system_gesture::SystemGestureZones) {
        assert_eq!(self.sender.send_system_gesture_zones(zones), InputDelivery::Queued);
    }

    /// Состояние экрана для чтения (панель, шаги EV, раскладка).
    pub fn screen(&self) -> &camera_screen::ScreenState {
        self.source.screen_state()
    }

    /// Счётчики исполнителя эффектов по последнему отчёту экрана (намерения камере: отправлено / нет движка).
    pub fn effect_counters(&self) -> EffectCounters {
        self.board.latest().expect("экран опубликовал отчёт").effects
    }

    /// Факты о камере, как их видит экран (по последнему отчёту).
    pub fn camera_presence(&self) -> camera_screen::CameraPresence {
        self.board.latest().expect("экран опубликовал отчёт").presence
    }

    /// Какие вибрации экран сыграл, по порядку.
    pub fn haptics_played(&self) -> Vec<HapticPattern> {
        self.side.haptics.played()
    }

    /// Сколько регулировок экран попросил у рендера (они только записаны, к бэкенду стенда не применяются).
    pub fn render_adjustments_requested(&self) -> usize {
        self.side.render.adjustments_requested()
    }

    /// Завершение: поток рендера закрыт, validation без ошибок, движок остановлен.
    pub fn finish(self) {
        let Self { engine, .. } = self;
        engine.finish();
    }
}
