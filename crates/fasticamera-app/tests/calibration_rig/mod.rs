//! Стенд сквозных тестов раздела калибровки (14b): НАСТОЯЩИЙ источник UI экрана камеры
//! (`CameraScreenSource`) с настоящим исполнителем эффектов, библиотекой пресетов, фактами о виде и брокером
//! диалога ввода — поверх настоящего бэкенда показа (RADV, headless, validation, подставная камера).
//!
//! Подставное здесь только то, что в продукте живёт за границей процесса: платформа диалога ввода, хранилище
//! пресетов (его сбой нужен тесту), камера и оболочка. «Ящик» рендера — запись команд: стенд забирает их из ящика и
//! применяет к бэкенду тем же `apply_viewfinder_command`, каким пользуется поток рендера, — до показа следующего кадра.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use camera_contract::zoom::LensIntentChannel;
use camera_screen::shutter::ShotRequest;
use camera_screen::{CameraFacts, CameraPresence, CameraScreen, ScreenAction, ScreenConfig, ScreenTouch};
use fasticamera::camera_screen_capture::{CaptureService, SubmitOutcome};
use fasticamera::camera_screen_effects::{AbsentCameraPort, EffectExecutor, EffectTargets};
use fasticamera::camera_screen_facts::{CameraFactsSource, FactsDiagnostics};
use fasticamera::camera_screen_input::{InputDelivery, ScreenInputSender, screen_input_channel};
use fasticamera::camera_screen_service::SilentShutterSound;
use fasticamera::camera_screen_source::{CameraScreenSource, GravityLink, ScreenBoard, ScreenSourceParts};
use fasticamera::look_facts::{BandSwatchStyle, LookFactsCatalog, LookFactsSource};
use fasticamera::look_service::{LookHandle, LookPersistence, LookService, LookStartup};
use fasticamera::look_startup::catalog_environment;
use fasticamera::look_white_stops::LookWhiteStopsWatcher;
use fasticamera::text_effects::TextResultToScreen;
use fasticamera::text_input_broker::TextInputBroker;
use fasticamera::viewfinder_control::{ControlDelivery, RenderAdjustment, RenderControl};
use look_contract::{LookRecipe, LookSettings, LookWhiteStopsBoard};
use look_presets::{LibrarySnapshot, PresetLibrary, PresetStoreConfig, SaveOutcome, SaveRequestOutcome};
use render_loop::backend::DisplayBackend;
use render_loop::config::{DisplayColorConfig, RenderLoopConfig, ToneBaseChoice, ViewfinderToneConfig};
use render_loop::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use render_loop::viewfinder::{ViewfinderCommand, ViewfinderCommandOutcome};
use scope_contract::{ScopeIntent, ScopeRequest};
use scope_model::{HorizontalOrder, ResolvedScope, ScopeCounters, ScopeRule, ShownCodes, accumulate};
use shell_contract::clock::MonotonicNanos;
use shell_contract::commands::{CommandOutcome, FrameRate, FrameRateOutcome, ScreenWake, ShellCommands};
use shell_contract::edge_insets::RawEdgeInsetsPx;
use shell_contract::haptics::{HapticOutcome, HapticPattern, HapticsService};
use shell_contract::safe_area::SafeAreaInsets;
use shell_contract::surface::SurfaceSize;
use shell_contract::system_gesture::SystemGestureZones;
use shell_contract::text_input::{TextInputService, TextRequest};
use shell_contract::touch::{PointerId, SurfacePoint, TouchPhase, TouchSample};
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::frame_source::{BackdropContinuity, UiFrameError, UiFrameRequest, UiFrameSource};
use ui_draw::geometry::PxSize;
use ui_draw::light::LightDirection;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};
use ui_kit::view::{LabelPainter, PxPerPt};

use crate::viewfinder_rig::{CellValues, ViewfinderBench, assert_image_color, reference_color_with_look};

/// Размер экрана телефона в пикселях (как в остальных тестах экрана).
const SURFACE: SurfaceSize = SurfaceSize {
    width_px: 1080,
    height_px: 2412,
};
/// Сколько кадров камеры готовит стенд: хватает на самый длинный тест, лишние не мешают.
///
/// Длинные сценарии «палец ведёт линейку» (несколько протяжек по ~40 кадров, листание списка, сброс) берут сотни
/// кадров; запас дешёвый (кадр камеры 64 × 48, сюжет — одна запись в таблице импортёра).
pub const CAMERA_FRAMES: usize = 1500;
/// Шаг времени кадров UI, мс.
const FRAME_STEP_MS: i64 = 16;

/// Ящик рендера теста: запоминает команды, как поток рендера запоминает их до применения.
#[derive(Default)]
pub struct RecordingInbox {
    adjustments: Mutex<Vec<RenderAdjustment>>,
}

impl RenderControl for RecordingInbox {
    fn adjust(&self, adjustment: RenderAdjustment) -> ControlDelivery {
        self.adjustments.lock().expect("замок").push(adjustment);
        ControlDelivery::Delivered
    }

    fn ui_overlay(&self, _command: UiOverlayCommand) -> ControlDelivery {
        ControlDelivery::Delivered
    }
}

/// Платформа диалога: помнит показанные запросы (ответ тест подаёт брокеру сам, как это делает маршрутизатор).
#[derive(Default)]
pub struct FakeDialogs {
    shown: Mutex<Vec<TextRequest>>,
}

impl FakeDialogs {
    pub fn shown(&self) -> Vec<TextRequest> {
        self.shown.lock().expect("замок").clone()
    }
}

impl TextInputService for FakeDialogs {
    fn request_text(&self, request: &TextRequest) -> CommandOutcome {
        self.shown.lock().expect("замок").push(request.clone());
        CommandOutcome::Queued
    }
}

/// Хранилище пресетов, которое просьбу принимает, а запись заканчивает сбоем (диск полон): итог приходит на опросе.
pub struct FailingPersistence {
    failure: look_contract::StorageFailure,
    outcomes: Mutex<Vec<SaveOutcome>>,
}

impl FailingPersistence {
    pub fn disk_full() -> Self {
        Self {
            failure: look_contract::StorageFailure::WriteFailed("диск полон".to_owned()),
            outcomes: Mutex::default(),
        }
    }
}

impl LookPersistence for FailingPersistence {
    fn request_save(&self, _snapshot: LibrarySnapshot) -> SaveRequestOutcome {
        self.outcomes
            .lock()
            .expect("замок")
            .push(SaveOutcome::Failed(look_contract::PresetError::Storage(
                self.failure.clone(),
            )));
        SaveRequestOutcome::Queued
    }

    fn drain_outcomes(&self) -> Vec<SaveOutcome> {
        std::mem::take(&mut *self.outcomes.lock().expect("замок"))
    }
}

/// Хранилище, которое принимает всё и ничего не пишет (запись не предмет этих тестов).
pub struct AcceptingPersistence;

impl LookPersistence for AcceptingPersistence {
    fn request_save(&self, _snapshot: LibrarySnapshot) -> SaveRequestOutcome {
        SaveRequestOutcome::Queued
    }

    fn drain_outcomes(&self) -> Vec<SaveOutcome> {
        Vec::new()
    }
}

struct LiveCamera;

impl CameraFactsSource for LiveCamera {
    fn current_facts(&mut self) -> CameraFacts {
        CameraFacts {
            presence: CameraPresence::Live,
            ..CameraFacts::STARTING
        }
    }

    fn diagnostics(&self) -> FactsDiagnostics {
        FactsDiagnostics::default()
    }
}

struct QuietHaptics;

impl HapticsService for QuietHaptics {
    fn play(&self, _pattern: HapticPattern) -> HapticOutcome {
        HapticOutcome::Unsupported
    }
}

struct QuietCommands;

impl ShellCommands for QuietCommands {
    fn request_camera_permission(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn set_preferred_frame_rate(&self, _rate: FrameRate) -> FrameRateOutcome {
        FrameRateOutcome::Applied
    }

    fn open_system_gallery(&self) -> CommandOutcome {
        CommandOutcome::Queued
    }

    fn keep_screen_on(&self, _wake: ScreenWake) -> CommandOutcome {
        CommandOutcome::Queued
    }
}

struct NoCapture;

impl CaptureService for NoCapture {
    fn submit(&self, _request: ShotRequest, _look: LookRecipe, _now: MonotonicNanos) -> SubmitOutcome {
        SubmitOutcome::Accepted
    }
}

/// Источник UI, который ничего не рисует: нужен, чтобы ПЕРЕУСТАНОВКА источника на рендере отозвала запрос прибора.
pub struct EmptyUiSource;

impl UiFrameSource for EmptyUiSource {
    fn name(&self) -> &'static str {
        "calibration-e2e-empty"
    }

    fn produce_frame(&mut self, _request: &UiFrameRequest) -> Result<DrawList, UiFrameError> {
        Ok(DrawList::new(LightDirection::from_angle_degrees(45.0)))
    }
}

/// Конфиг рендера стенда: sRGB без хитростей охвата, остальное по умолчанию.
pub fn render_config() -> RenderLoopConfig {
    RenderLoopConfig {
        display_color: DisplayColorConfig::SrgbOnly,
        ..RenderLoopConfig::default()
    }
}

/// Библиотека пресетов со встроенными пресетами из конфига и заданным хранилищем.
pub fn library_over(persistence: Box<dyn LookPersistence>) -> LookHandle {
    let config = PresetStoreConfig::default();
    let catalog = config
        .builtin_catalog(&catalog_environment(&render_config()))
        .expect("конфиг по умолчанию верен");
    LookHandle::new(LookService::new(
        PresetLibrary::new(catalog),
        persistence,
        LookStartup::InMemoryOnly,
    ))
}

/// Всё, что тест видит и трогает: бэкенд, источник экрана, канал входа, брокер и библиотека.
pub struct CalibrationRig {
    pub bench: ViewfinderBench,
    pub source: CameraScreenSource,
    pub board: Arc<ScreenBoard>,
    pub sender: ScreenInputSender,
    pub inbox: Arc<RecordingInbox>,
    pub library: LookHandle,
    /// Доска упоров ручек теплоты и оттенка, которую читает источник экрана.
    pub look_white_stops: LookWhiteStopsBoard,
    pub broker: TextInputBroker,
    pub dialogs: Arc<FakeDialogs>,
    /// Намерения по прибору, которые рендер получил, по порядку (стенд записывает их перед применением).
    pub scope_intents: Vec<ScopeIntent>,
    /// Исходы применения всех команд видоискателю (ничего не должно быть отвергнуто).
    pub outcomes: Vec<ViewfinderCommandOutcome>,
    /// Сколько команд «место кадра» стенд пропустил (см. `deliver_inbox_to_the_backend`).
    pub framing_commands_skipped: u64,
    scene: CellValues,
    frame_index: i64,
    /// Рисунок экрана, который источник отдал на последнем кадре (подписи значений читает `drawn_text_count`).
    last_ui: Option<DrawList>,
}

impl CalibrationRig {
    /// Стенд с библиотекой `library` и сюжетом камеры `scene`. `None` — GPU осознанно пропущен (тест тихо заканчивается).
    pub fn start(library: LookHandle, scene: CellValues) -> Option<Self> {
        let mut bench = ViewfinderBench::start(&render_config(), scene, CAMERA_FRAMES)?;
        // Порядок решения 14h: источник UI ставится ПЕРВЫМ (его установка отзывает запросы приборов), затем `Show`.
        // Настоящий источник экрана камеры стенд ведёт сам (`produce_frame`), поэтому на рендере стоит пустой.
        assert_eq!(
            bench
                .backend
                .apply_ui_overlay_command(UiOverlayCommand::Install(Box::new(EmptyUiSource))),
            UiOverlayOutcome::Applied
        );
        let dialogs = Arc::new(FakeDialogs::default());
        let broker = TextInputBroker::new(Arc::clone(&dialogs) as Arc<dyn TextInputService>);
        let inbox = Arc::new(RecordingInbox::default());
        // Доска упоров теплоты и оттенка: в продукте в неё пишет поток рендера, здесь — тест (`look_white_stops`).
        let look_white_stops = LookWhiteStopsBoard::new();
        let (source, sender, board) = Self::new_screen(&library, &broker, &inbox, &look_white_stops);
        let rig = Self {
            look_white_stops,
            bench,
            source,
            board,
            sender,
            inbox,
            library,
            broker,
            dialogs,
            scope_intents: Vec::new(),
            outcomes: Vec::new(),
            framing_commands_skipped: 0,
            scene,
            frame_index: 0,
            last_ui: None,
        };
        rig.sender.send_action(ScreenAction::SurfaceResized(SURFACE));
        Some(rig)
    }

    /// Собирает НОВЫЙ экран так, как это делает сервис экрана при установке: исполнитель с брокером, источник
    /// фактов о виде, приёмник ответов диалога, указывающий на канал входа этого экрана.
    fn new_screen(
        library: &LookHandle,
        broker: &TextInputBroker,
        inbox: &Arc<RecordingInbox>,
        look_white_stops: &LookWhiteStopsBoard,
    ) -> (CameraScreenSource, ScreenInputSender, Arc<ScreenBoard>) {
        let (sender, receiver) = screen_input_channel(64);
        let board = Arc::new(ScreenBoard::default());
        let executor = EffectExecutor::new(EffectTargets {
            camera: Box::new(AbsentCameraPort),
            haptics: Arc::new(QuietHaptics),
            commands: Arc::new(QuietCommands),
            render: Arc::clone(inbox) as Arc<dyn RenderControl>,
            capture: Arc::new(NoCapture),
            shutter_sound: Arc::new(SilentShutterSound::default()),
            lens: LensIntentChannel::new(),
            look: Some(library.clone()),
        })
        .with_text_input(broker.clone());
        broker.set_sink(Arc::new(TextResultToScreen::new(sender.injector())));
        let screen = CameraScreen::new(
            ScreenConfig::default(),
            ui_kit::tokens::UiKitTokens::default(),
            ui_core::time::MotionPreference::Full,
        )
        .expect("экран");
        let source = CameraScreenSource::new(ScreenSourceParts {
            look_white_stops: LookWhiteStopsWatcher::new(look_white_stops.clone()),
            screen,
            input: receiver,
            facts: Box::new(LiveCamera),
            look_facts: Some(LookFactsSource::new(
                library.clone(),
                LookFactsCatalog::from_render_config(&render_config(), &BandSwatchStyle::default()),
            )),
            gravity: GravityLink::Unavailable {
                reason: "тест".to_owned(),
            },
            executor,
            board: Arc::clone(&board),
            report_min_interval: Duration::ZERO,
        });
        (source, sender, board)
    }

    /// Рендер снял источник UI и поставил другой, а экран камеры собран ЗАНОВО (как делает сервис экрана при
    /// переустановке): новый источник, новый канал входа, новый приёмник ответов.
    pub fn replace_screen_with_a_fresh_one(&mut self) {
        assert_eq!(
            self.bench
                .backend
                .apply_ui_overlay_command(UiOverlayCommand::Install(Box::new(EmptyUiSource))),
            UiOverlayOutcome::Applied
        );
        let (source, sender, board) =
            Self::new_screen(&self.library, &self.broker, &self.inbox, &self.look_white_stops);
        self.source = source;
        self.sender = sender;
        self.board = board;
        self.sender.send_action(ScreenAction::SurfaceResized(SURFACE));
    }

    /// Действие экрана (как от маршрутизатора или платформы): ляжет в канал входа и разберётся на ближайшем кадре.
    pub fn send(&mut self, action: ScreenAction) {
        self.sender.send_action(action);
    }

    /// Края экрана, как их сообщает телефон: полосы системных жестов (фильтр касаний) и безопасная зона (раскладка).
    pub fn report_screen_edges(&mut self, insets: RawEdgeInsetsPx, safe_area: SafeAreaInsets) {
        let zones = SystemGestureZones::try_from_insets(insets).expect("зоны");
        assert_eq!(self.sender.send_system_gesture_zones(zones), InputDelivery::Queued);
        self.sender.send_action(ScreenAction::SafeAreaChanged(safe_area));
    }

    /// Касание ОДНОГО пальца (номер 0) в пикселях поверхности, с меткой времени СЛЕДУЮЩЕГО кадра: экран разберёт его
    /// на этом кадре, а время касаний идёт вровень со временем кадров (жест не видит «будущего» и не отстаёт).
    /// Это настоящий вход экрана — тот же канал и тот же образец, что шлёт оболочка, а не вызов виджета.
    pub fn touch(&mut self, phase: TouchPhase, x_px: f32, y_px: f32) -> InputDelivery {
        let time = MonotonicNanos::from_nanos((self.frame_index + 1) * FRAME_STEP_MS * 1_000_000);
        self.sender.send_touch(ScreenTouch::Sample(TouchSample {
            pointer: PointerId::new(0),
            phase,
            position: SurfacePoint { x_px, y_px },
            time,
        }))
    }

    /// Один кадр: источник UI разбирает вход и исполняет эффекты, команды из ящика применяются к бэкенду, затем
    /// бэкенд показывает следующий кадр камеры (его пиксели читает `assert_shows`).
    pub fn frame(&mut self) {
        self.frame_index += 1;
        let request = UiFrameRequest {
            present_time: MonotonicNanos::from_nanos(self.frame_index * FRAME_STEP_MS * 1_000_000),
            logical_size: PxSize::new(1080.0, 2412.0),
            px_per_pt: 1080.0 / 440.0,
            backdrop: BackdropContinuity::Live,
        };
        self.last_ui = Some(self.source.produce_frame(&request).expect("кадр экрана"));
        self.deliver_inbox_to_the_backend();
        self.bench.show_next_frame();
    }

    /// Несколько кадров подряд: ответы и факты доходят на следующих кадрах (вход → эффект → факты).
    pub fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame();
        }
    }

    fn deliver_inbox_to_the_backend(&mut self) {
        let delivered: Vec<RenderAdjustment> = self.inbox.adjustments.lock().expect("замок").drain(..).collect();
        for adjustment in delivered {
            let RenderAdjustment::Viewfinder(command) = adjustment else {
                panic!("ожидалась команда видоискателю: {adjustment:?}");
            };
            // «Место кадра» (кроп по раскладке экрана) к виду отношения не имеет, а кадр стенда 64×48 слишком мал для
            // кропа: сетка статистики AWB стала бы крупнее ячеек. Команду считаем и не применяем.
            if matches!(command, ViewfinderCommand::SetFraming(_)) {
                self.framing_commands_skipped += 1;
                continue;
            }
            if let ViewfinderCommand::Scopes(intent) = &command {
                self.scope_intents.push(*intent);
            }
            self.outcomes.push(self.bench.backend.apply_viewfinder_command(command));
        }
    }

    /// Сколько риск-упоров нарисовано на последнем кадре экрана (решения 288, 293): фигуры-полоски толщиной и длиной
    /// из конфига вида упоров, у обычных рисок размеры другие.
    pub fn drawn_stop_tick_count(&self) -> usize {
        let look = ScreenConfig::default().calibration.widgets.knob_stops;
        let px_per_pt = 1080.0 / 440.0;
        let (width, height) = (
            (look.tick_thickness_pt * px_per_pt) as f32,
            (look.tick_length_pt * px_per_pt) as f32,
        );
        self.last_ui.as_ref().map_or(0, |list| {
            list.leaf_items()
                .filter(|item| {
                    matches!(
                        item,
                        DrawItem::Shape(shape) if matches!(
                            shape.geometry,
                            ui_draw::shape::ShapeGeometry::RoundedRect { size, .. }
                                if (size.width - width).abs() < 0.05 && (size.height - height).abs() < 0.05
                        )
                    )
                })
                .count()
        })
    }

    /// Яркость (альфа цвета) подписи значения шкалы `text` на последнем кадре; `None` — такой подписи нет.
    pub fn drawn_scale_value_alpha(&self, text: &str) -> Option<f32> {
        let tokens = UiKitTokens::default();
        let expected = LabelPainter::new(&tokens.typography, PxPerPt::new(1080.0 / 440.0).expect("масштаб"))
            .layout(text, TextRoleKind::ScaleValue)
            .expect("раскладка подписи");
        self.last_ui.as_ref().and_then(|list| {
            list.leaf_items().find_map(|item| match item {
                DrawItem::Text(drawn) if drawn.run == expected && drawn.opacity.get() > 0.05 => Some(drawn.color.a),
                _ => None,
            })
        })
    }

    /// Сколько раз на последнем кадре экрана нарисована подпись `text` ролью значения шкалы (`+30`, `−7`, `0`).
    /// Раскладка текста та же, что у экрана: масштаб кадра стенда и токены по умолчанию.
    pub fn drawn_scale_value_count(&self, text: &str) -> usize {
        let tokens = UiKitTokens::default();
        let expected = LabelPainter::new(&tokens.typography, PxPerPt::new(1080.0 / 440.0).expect("масштаб"))
            .layout(text, TextRoleKind::ScaleValue)
            .expect("раскладка подписи");
        self.last_ui.as_ref().map_or(0, |list| {
            list.leaf_items()
                .filter(
                    |item| matches!(item, DrawItem::Text(drawn) if drawn.run == expected && drawn.opacity.get() > 0.05),
                )
                .count()
        })
    }

    /// Пиксели показанного видоискателя совпадают с эталоном `isp-model` для вида `look` на основе `base`.
    pub fn assert_shows(&self, base: ToneBaseChoice, look: &LookSettings, what: &str) {
        let tone = ViewfinderToneConfig {
            base,
            ..render_config().viewfinder.tone
        };
        let reference = reference_color_with_look(&self.bench.passport, self.scene, &tone, look);
        assert_image_color(&self.bench.backend, reference, what);
    }

    /// Эталон не должен совпадать с другим эталоном, иначе тест ничего не докажет: расхождение в единицах кода.
    pub fn reference_gap(
        &self,
        first: (ToneBaseChoice, &LookSettings),
        second: (ToneBaseChoice, &LookSettings),
    ) -> f32 {
        let reference = |(base, look): (ToneBaseChoice, &LookSettings)| {
            let tone = ViewfinderToneConfig {
                base,
                ..render_config().viewfinder.tone
            };
            reference_color_with_look(&self.bench.passport, self.scene, &tone, look)
        };
        let (first, second) = (reference(first), reference(second));
        (0..3)
            .map(|channel| (first[channel] - second[channel]).abs())
            .fold(0.0, f32::max)
    }

    /// Какой прибор сейчас заказан у рендера (его собственная диагностика, а не запись команд стенда).
    pub fn scope_requested_at_the_render(&mut self) -> Option<scope_contract::ScopeRequest> {
        self.bench
            .backend
            .diagnostics()
            .viewfinder
            .expect("диагностика видоискателя")
            .scopes
            .requested
    }

    /// Состояние экрана для чтения.
    pub fn screen(&self) -> &camera_screen::ScreenState {
        self.source.screen_state()
    }

    /// Все команды применены без отказа и стенд завершается чисто (без утечек и ошибок validation).
    pub fn finish(self) {
        assert!(
            self.outcomes.iter().all(|outcome| matches!(
                outcome,
                ViewfinderCommandOutcome::Applied | ViewfinderCommandOutcome::AlreadyInEffect
            )),
            "рендер отверг команду: {:?}",
            self.outcomes
        );
        self.bench.finish();
    }
}

/// Счётчики, которые насчитал GPU в последнем проходе прибора.
pub fn gpu_scope_counters(rig: &CalibrationRig) -> ScopeCounters {
    rig.bench.backend.context().wait_idle().expect("wait_idle");
    rig.bench
        .backend
        .read_scope_counters()
        .expect("чтение счётчиков приборов")
        .expect("набор приборов есть и проход записан")
}

/// Эталон `scope_model::accumulate` по картинке видоискателя, прочитанной с GPU: то, что показано в ЭТОМ кадре,
/// с видом, размером и осями ровно из запроса, который отправил экран.
pub fn cpu_scope_counters(rig: &CalibrationRig, request: &ScopeRequest) -> ScopeCounters {
    rig.bench.backend.context().wait_idle().expect("wait_idle");
    let image = rig
        .bench
        .backend
        .read_viewfinder_image()
        .expect("чтение изображения видоискателя")
        .expect("изображение видоискателя есть");
    let shown = ShownCodes::from_display_rgba(image.width, image.height, &image.pixels, HorizontalOrder::AsStored)
        .expect("коды");
    let model = RenderLoopConfig::default().viewfinder.scopes.model;
    let rule = ResolvedScope::resolve(request, &model, isp_model::image::DisplayGamut::Srgb).expect("правило");
    // Шаги — по плану прибора (у вектороскопа свой, решение 328; у следов реже поперёк взгляда, 329), как у GPU.
    let steps = rule.planned_samples(shown.width(), shown.height(), &model).steps();
    accumulate(&shown, steps, &rule as &dyn ScopeRule).expect("эталон")
}

/// Запрос прибора, который рендер держит прямо сейчас (его собственная диагностика), а не запись команд стенда.
pub fn live_scope_request(rig: &mut CalibrationRig) -> ScopeRequest {
    rig.scope_requested_at_the_render()
        .expect("рендер держит запрос прибора")
}
