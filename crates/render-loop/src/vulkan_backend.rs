//! Настоящий бэкенд показа поверх `gpu-vulkan`: `DisplayTarget` + тестовый паттерн.
//!
//! Архитектура (пакет C1, сессия 03):
//! * [`VulkanDisplayBackend`] реализует контракт [`DisplayBackend`]; поток рендера видит
//!   только этот трейт и нейтральные типы `backend`, а типы Vulkan переводятся здесь (`mapping`);
//! * `session::AttachedDisplay` владеет живым показом (цель, паттерн, окно ПК-стенда) и
//!   порядком уничтожения; бэкенд держит `Option` такого показа;
//! * `camera` — показ кадров камеры: видоискатель через ISP на GPU (сессия 07) или отладочная мозаика
//!   (сессия 05), импорт буферов, учёт лизингов и fence (`new_android_camera_preview` /
//!   `new_headless_camera_preview`); команды видоискателю и лента статистики AWB (`awb_statistics`);
//! * `config` — настройки; `present_history` — мост «id показа Vulkan -> кадр потока».
//!
//! Режимы показа: [`VulkanDisplayBackend::new_android`] — боевой (Android, окно оболочки);
//! `new_headless_presentation` — СТЕНД ТОЛЬКО ДЛЯ ПК: swapchain на `VK_EXT_headless_surface`.
//! Настоящее окно там не используется, но `NativeSurface` из вызова `attach` бэкенд удерживает
//! и возвращает в `detach`, чтобы протокол «окно отпущено -> квитанция» проверялся одинаково.

mod camera;
mod census;
mod config;
mod mapping;
mod present_history;
mod present_time;
mod session;

#[cfg(target_os = "android")]
pub use camera::AndroidCameraImporter;
pub use camera::{CameraFeed, CameraFrameImporter, CameraImportError, CameraImporterFactory, SubmissionSerial};
pub use config::VulkanBackendConfig;

use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::error::GpuInitError;
use gpu_vulkan::swapchain::PresentationSurface;
use shell_contract::surface::{NativeSurface, SurfaceSize};

use crate::backend::{
    AttachError, BackendDiagnostics, BackendError, DisplayBackend, FrameCpuTiming, FrameGpuTiming, FrameTiming,
    PastPresentation, PresentOutcome,
};
use crate::camera_diagnostics::CameraStreamDiagnostics;
use crate::still::{StillCommand, StillCommandOutcome};
use crate::ui_overlay::{UiOverlayCommand, UiOverlayOutcome};
use crate::viewfinder::{
    AutoExposureLinks, AwbStatisticsFeed, AwbStatisticsReader, ViewfinderCommand, ViewfinderCommandOutcome,
    ViewfinderDiagnostics, WhiteBalanceDiagnostics,
};
use camera::{
    AutoExposureParts, GpuMaker, SceneWhiteMemory, StillDiagnostics, StillEngine, SystemStillClock, UiOverlayState,
    ViewfinderSettings, ViewfinderStats, VulkanStillGpu, VulkanStillSettings, WhiteBalanceParts,
};
use camera_contract::three_a::{DirectiveSlot, WhiteBalanceDirective};
use census::census_by_kind;
use session::{AttachedDisplay, CameraAttach};

const LOG_TARGET: &str = "fasticamera::render::vulkan";

/// Изображение видоискателя, прочитанное обратно (тестовый шов, фича `test-support`).
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, PartialEq)]
pub struct ViewfinderReadback {
    /// Ширина, пиксели.
    pub width: u32,
    /// Высота, пиксели.
    pub height: u32,
    /// Пиксели `[r, g, b, a]` построчно сверху вниз (значения, закодированные кривой экрана).
    pub pixels: Vec<[f32; 4]>,
}

/// Как окно из `attach` превращается в поверхность показа.
#[derive(Debug, Clone, Copy)]
enum PresentationMode {
    /// Боевой режим: окно Android передаётся цели во владение.
    #[cfg(target_os = "android")]
    AndroidWindow,
    /// Стенд ПК: цель работает на headless-surface, а окно хранит сам бэкенд.
    #[cfg(not(target_os = "android"))]
    HeadlessBench,
}

impl PresentationMode {
    /// Возвращает поверхность для цели и окно, которое бэкенд должен удержать сам.
    fn wrap_window(self, window: NativeSurface) -> (PresentationSurface, Option<NativeSurface>) {
        match self {
            #[cfg(target_os = "android")]
            PresentationMode::AndroidWindow => (PresentationSurface::Android(window), None),
            #[cfg(not(target_os = "android"))]
            PresentationMode::HeadlessBench => (PresentationSurface::Headless, Some(window)),
        }
    }
}

/// Бэкенд показа на Vulkan. Все методы вызываются из потока рендера.
pub struct VulkanDisplayBackend {
    context: GpuContext,
    config: VulkanBackendConfig,
    mode: PresentationMode,
    /// `None` — surface не присоединена.
    attached: Option<AttachedDisplay>,
    /// Сколько раз `detach` завершился ошибкой (в диагностику).
    detach_failures: u64,
    /// Источник кадров камеры; `None` — показывается тестовый паттерн.
    camera_feed: Option<CameraFeed>,
    /// Счётчики показа камеры по уже закрытым surface (текущая добавляется при опросе диагностики).
    camera_finished: CameraStreamDiagnostics,
    /// Настройки видоискателя: из конфига, дальше — командами; переживают пересоздание surface.
    viewfinder_settings: ViewfinderSettings,
    /// Статистика видоискателя за жизнь бэкенда. `None`, пока её держит сессия показа камеры
    /// (возвращается при освобождении surface).
    viewfinder_stats: Option<ViewfinderStats>,
    /// Лента статистики AWB (сессия показа публикует, читатель — `awb_statistics()`).
    awb_feed: AwbStatisticsFeed,
    /// Указания AWB от движка камеры (читающая половина ящика; писатель — у composition root). Пока ничего
    /// не опубликовано, действует `WhiteBalanceDirective::DEFAULT`.
    white_balance_directives: DirectiveSlot<WhiteBalanceDirective>,
    /// Белая сцены переживает пересоздание surface и смену камеры (решение 65).
    scene_white: SceneWhiteMemory,
    /// Ящики нашего AE (указания от движка, предложения движку). `None` — AE на потоке рендера выключен:
    /// подтяжка ISP равна 1, предложений нет (вендорский AE в композиции).
    auto_exposure: Option<AutoExposureLinks>,
    /// Источник кадра UI и счётчики UI-прохода (сессия 11): переживают пересоздание surface. Рендер UI
    /// (объекты GPU) живёт в пайплайнах цепочки показа и создаётся только при установленном источнике.
    ui_overlay: UiOverlayState,
    /// Движок снимков (сессия 14): выбор кадра «что было на экране», очередь, проявление. `None` — снимков нет,
    /// команды снимка завершаются честным `NotSecured` (поток рендера сам).
    still: Option<StillEngine<VulkanStillGpu>>,
}

impl VulkanDisplayBackend {
    /// Создать рендер UI сразу при установке источника — МЕЖДУ кадрами, а не в первом кадре с UI:
    /// конвейеры и загрузка атласа (блокирующая) заняли бы время записи кадра и сорвали бы его vsync
    /// (ревью сессии 11). Цели показа ещё нет (surface не прикреплён) — рендер создастся лениво в
    /// первом кадре, как раньше; отказ здесь не фатален: запись кадра повторит попытку и учтёт ошибку.
    fn prepare_ui_renderer_between_frames(&mut self) {
        let Some(display) = self.attached.as_mut() else {
            return;
        };
        if let Err(error) = display.prepare_ui_overlay() {
            tracing::warn!(target: LOG_TARGET, %error, "рендер UI не создан при установке источника: повтор в кадре");
        }
    }

    /// Боевой конструктор для Android: профиль `AndroidDisplay` (swapchain, AHB, display timing).
    /// Ошибка — честный `GpuInitError` (нет Vulkan, расширения, слоя validation…).
    #[cfg(target_os = "android")]
    pub fn new_android(config: VulkanBackendConfig) -> Result<Self, GpuInitError> {
        Self::with_profile(config, DeviceProfile::AndroidDisplay, PresentationMode::AndroidWindow)
    }

    /// Конструктор СТЕНДА ПК: профиль `HeadlessPresentation` (swapchain на
    /// `VK_EXT_headless_surface`, без экрана и без display timing). Существует только для
    /// ПК-бенча и тестов потока рендера на настоящем драйвере; на телефоне не используется.
    #[cfg(not(target_os = "android"))]
    pub fn new_headless_presentation(config: VulkanBackendConfig) -> Result<Self, GpuInitError> {
        Self::with_profile(
            config,
            DeviceProfile::HeadlessPresentation,
            PresentationMode::HeadlessBench,
        )
    }

    /// Боевой конструктор Android с показом кадров камеры: предпросмотр RAW-мозаики вместо тестового
    /// паттерна. Буферы камеры импортируются в Vulkan без копий (`AndroidCameraImporter`).
    ///
    /// `white_balance_directives` — читающая половина «почтового ящика» указаний AWB: composition root
    /// создаёт ящик, писатель отдаёт `camera-core`, читатель — сюда (решение 64).
    #[cfg(target_os = "android")]
    pub fn new_android_camera_preview(
        config: VulkanBackendConfig,
        frames: raw_frame_stream::ViewfinderFrames,
        white_balance_directives: DirectiveSlot<WhiteBalanceDirective>,
    ) -> Result<Self, GpuInitError> {
        let cache_capacity = config.camera_import_cache_capacity;
        let feed = CameraFeed::new(
            frames,
            Box::new(move |context| Ok(Box::new(AndroidCameraImporter::new(context, cache_capacity)))),
        );
        let mut backend = Self::with_profile(config, DeviceProfile::AndroidDisplay, PresentationMode::AndroidWindow)?;
        backend.camera_feed = Some(feed);
        backend.white_balance_directives = white_balance_directives;
        Ok(backend)
    }

    /// Конструктор СТЕНДА ПК с показом кадров камеры: импорт буферов — через `make_importer` (на ПК нет
    /// `AHardwareBuffer`, тест подставляет синтетические источники). Остальное — боевой путь:
    /// опрос видоискателя, сабмит с ожиданием/экспортом fence, учёт лизингов. `white_balance_directives` —
    /// как у боевого конструктора (тест публикует указания через писателя ящика).
    #[cfg(not(target_os = "android"))]
    pub fn new_headless_camera_preview(
        config: VulkanBackendConfig,
        frames: raw_frame_stream::ViewfinderFrames,
        make_importer: CameraImporterFactory,
        white_balance_directives: DirectiveSlot<WhiteBalanceDirective>,
    ) -> Result<Self, GpuInitError> {
        let mut backend = Self::with_profile(
            config,
            DeviceProfile::HeadlessPresentation,
            PresentationMode::HeadlessBench,
        )?;
        backend.camera_feed = Some(CameraFeed::new(frames, make_importer));
        backend.white_balance_directives = white_balance_directives;
        Ok(backend)
    }

    fn with_profile(
        config: VulkanBackendConfig,
        profile: DeviceProfile,
        mode: PresentationMode,
    ) -> Result<Self, GpuInitError> {
        let context = GpuContext::new(&GpuContextConfig::new(profile, config.validation.clone()))?;
        let viewfinder_settings = ViewfinderSettings::from_config(&config.viewfinder).unwrap_or_else(|error| {
            // Конфиг проверяется `RenderLoopConfig::validate` до запуска; сюда попадает только конфиг,
            // собранный в обход проверки. Показ не ломаем: вся полезная область, ошибка — в журнал.
            tracing::error!(target: LOG_TARGET, %error, "секция viewfinder отвергнута: кадрирование по умолчанию");
            ViewfinderSettings {
                painter: config.viewfinder.painter,
                stages: isp_vulkan::viewfinder::IspStageSet::ALL,
                crop: crate::viewfinder::FramingCrop::Fixed(
                    camera_contract::viewfinder_crop::ViewfinderCrop::FULL_SENSOR,
                ),
                placement: config.viewfinder.placement,
                destination: crate::viewfinder::FramingDestination::Configured,
                headroom: crate::viewfinder::HeadroomBench::Disabled,
            }
        });
        let viewfinder_stats = Some(ViewfinderStats::new(config.viewfinder.gpu_time_window_frames));
        let ui_overlay = UiOverlayState::new(config.viewfinder.gpu_time_window_frames);
        Ok(Self {
            context,
            config,
            mode,
            attached: None,
            detach_failures: 0,
            camera_feed: None,
            camera_finished: CameraStreamDiagnostics::default(),
            viewfinder_settings,
            viewfinder_stats,
            awb_feed: AwbStatisticsFeed::new(),
            white_balance_directives: DirectiveSlot::new(),
            scene_white: SceneWhiteMemory::default(),
            auto_exposure: None,
            ui_overlay,
            still: None,
        })
    }

    /// Включает наш AE на потоке рендера: композиция передаёт ящики между рендером и движком камеры (писатель
    /// указаний и читатель предложений — у движка). Добавочный способ: конструкторы бэкенда не меняются, без
    /// этого вызова AE на потоке рендера выключен. Настройки берутся из `viewfinder.auto_exposure` конфига.
    #[must_use]
    pub fn with_auto_exposure(mut self, links: AutoExposureLinks) -> Self {
        self.auto_exposure = Some(links);
        self
    }

    /// Включает снимки без задержки затвора (сессия 14): композиция отдаёт потребителя кольца снимков того же
    /// потока, что и видоискатель. Добавочный способ, как `with_auto_exposure`: без вызова (или без источника
    /// кадров камеры) снимков нет. Настройки — `VulkanBackendConfig::still`.
    #[must_use]
    pub fn with_still_capture(mut self, ring: raw_frame_stream::SnapshotRingFrames) -> Self {
        let Some(feed) = self.camera_feed.as_ref() else {
            tracing::warn!(target: LOG_TARGET, "снимки включены без источника кадров камеры: игнорируются");
            return self;
        };
        let importers = feed.make_importer.clone();
        let context = self.context.clone();
        let settings = VulkanStillSettings::from_config(&self.config.still, self.config.teardown_timeout);
        let maker: GpuMaker<VulkanStillGpu> =
            Box::new(move |stream| VulkanStillGpu::new(&context, settings, &importers, stream));
        self.still = Some(StillEngine::new(
            self.config.still,
            self.config.viewfinder.tone,
            ring,
            maker,
            Box::new(SystemStillClock),
        ));
        self
    }

    /// Учёт снимков (`None` — снимков нет).
    pub fn still_diagnostics(&self) -> Option<StillDiagnostics> {
        self.still.as_ref().map(StillEngine::diagnostics)
    }

    /// Подаёт записи показа драйвера движку снимков, как если бы их вычерпал `drain_past_presentations`
    /// (тестовый шов, фича `test-support`: на ПК записей display timing нет).
    #[cfg(feature = "test-support")]
    pub fn inject_past_presentations(&mut self, presentations: &[PastPresentation]) {
        if let Some(still) = self.still.as_mut() {
            still.ingest_presentations(presentations);
        }
    }

    /// Читатель статистики AWB видоискателя (latest-wins, с кадром и меткой сенсора). Composition root
    /// берёт его до запуска потока рендера и отдаёт будущему владельцу AWB (сессия 08).
    pub fn awb_statistics(&self) -> AwbStatisticsReader {
        self.awb_feed.reader()
    }

    /// Читает изображение видоискателя присоединённой surface (тестовый шов, фича `test-support`):
    /// размер и пиксели `[r, g, b, a]` построчно сверху вниз. `Ok(None)` — нет surface, камеры или кадра.
    #[cfg(feature = "test-support")]
    pub fn read_viewfinder_image(&self) -> Result<Option<ViewfinderReadback>, gpu_vulkan::error::GpuError> {
        match self.attached.as_ref() {
            Some(display) => display.read_viewfinder_image(),
            None => Ok(None),
        }
    }

    /// Читает содержимое удержанного кадра присоединённой surface (тестовый шов, фича `test-support`).
    /// `Ok(None)` — нет surface, камеры или удержания.
    #[cfg(feature = "test-support")]
    pub fn read_held_image(&self) -> Result<Option<ViewfinderReadback>, gpu_vulkan::error::GpuError> {
        match self.attached.as_ref() {
            Some(display) => display.read_held_image(),
            None => Ok(None),
        }
    }

    /// Диагностика видоискателя (`None` — камеры нет): живой сессии, иначе накопленная бэкендом.
    fn viewfinder_diagnostics(&mut self) -> Option<ViewfinderDiagnostics> {
        self.camera_feed.as_ref()?;
        if let Some(mut live) = self.attached.as_mut().and_then(AttachedDisplay::viewfinder_diagnostics) {
            // UI-проход принадлежит бэкенду, а не сессии: его счётчики дописываются здесь.
            live.ui_overlay = self.ui_overlay.snapshot();
            return Some(live);
        }
        let settings = self.viewfinder_settings.clone();
        let stats = self
            .viewfinder_stats
            .get_or_insert_with(|| ViewfinderStats::new(self.config.viewfinder.gpu_time_window_frames));
        // Сессии показа нет — объектов стенда V4 тоже нет (освобождены вместе с сессией).
        let mut snapshot = stats.snapshot(
            &settings,
            camera::HeadroomLiveState::default(),
            WhiteBalanceDiagnostics::idle(self.config.viewfinder.white_balance.statistics_every_n_frames),
        );
        snapshot.ui_overlay = self.ui_overlay.snapshot();
        Some(snapshot)
    }

    /// Взводит одноразовый тестовый шов отказа в присоединённой цели (feature `fault-injection`):
    /// например, изобразить зависший GPU, чтобы пройти путь «detach не удался» детерминированно.
    #[cfg(feature = "fault-injection")]
    pub fn inject_fault(&mut self, fault: gpu_vulkan::swapchain::InjectedFault) -> Result<(), BackendError> {
        match self.attached.as_mut() {
            Some(display) => display.inject_fault(fault),
            None => Err(BackendError {
                operation: "inject_fault".to_owned(),
                reason: "surface не присоединена".to_owned(),
            }),
        }
    }

    /// Сколько показов текущей цепочки ждут записи display timing (feature `fault-injection`,
    /// тестовый шов); `None` — surface не присоединена.
    #[cfg(feature = "fault-injection")]
    pub fn present_history_len(&self) -> Option<usize> {
        self.attached.as_ref().map(AttachedDisplay::present_history_len)
    }

    /// Контекст GPU бэкенда (дёшево клонируется). Нужен снаружи, чтобы до запуска потока
    /// снять базовую линию живых объектов и после — проверить, что они вернулись.
    pub fn context(&self) -> &GpuContext {
        &self.context
    }
}

impl DisplayBackend for VulkanDisplayBackend {
    fn attach(&mut self, surface: NativeSurface, size: SurfaceSize) -> Result<(), AttachError> {
        if self.attached.is_some() {
            // Поток обязан сначала отсоединить прежнюю surface; новая уничтожается здесь.
            return Err(AttachError::SurfaceRejected(BackendError {
                operation: "attach".to_owned(),
                reason: "бэкенд уже держит surface: сначала detach".to_owned(),
            }));
        }
        let (presentation, held_surface) = self.mode.wrap_window(surface);
        let camera = self.camera_feed.as_ref().map(|feed| CameraAttach {
            feed,
            settings: self.viewfinder_settings.clone(),
            stats: &mut self.viewfinder_stats,
            gpu_time_window_frames: self.config.viewfinder.gpu_time_window_frames,
            awb_feed: &self.awb_feed,
            white_balance: WhiteBalanceParts {
                config: self.config.viewfinder.white_balance,
                directives: self.white_balance_directives.clone(),
                memory: self.scene_white.clone(),
            },
            auto_exposure: self.auto_exposure.as_ref().map(|links| AutoExposureParts {
                config: self.config.viewfinder.auto_exposure,
                directives: links.directives.clone(),
                proposals: links.proposals.clone(),
            }),
        });
        let display = AttachedDisplay::attach(&self.context, &self.config, presentation, held_surface, size, camera)?;
        self.attached = Some(display);
        Ok(())
    }

    fn resize(&mut self, size: SurfaceSize) -> Result<(), BackendError> {
        match self.attached.as_mut() {
            Some(display) => display.resize(size),
            None => Err(BackendError {
                operation: "resize".to_owned(),
                reason: "surface не присоединена".to_owned(),
            }),
        }
    }

    fn render_and_present(&mut self, timing: FrameTiming) -> PresentOutcome {
        match self.attached.as_mut() {
            Some(display) => display.render_and_present(timing, &mut self.ui_overlay),
            None => PresentOutcome::Failed(BackendError {
                operation: "render_and_present".to_owned(),
                reason: "surface не присоединена".to_owned(),
            }),
        }
    }

    fn detach(&mut self) -> Option<NativeSurface> {
        // Surface уходит: каждый незаконченный снимок получает итог, объекты GPU снимков освобождаются до цепочки.
        if let Some(still) = self.still.as_mut() {
            still.on_detach();
        }
        let display = self.attached.take()?;
        let report = display.release();
        if let Some(camera) = &report.camera {
            self.camera_finished = self
                .camera_finished
                .combined_with(camera, self.config.camera_present_history_capacity);
        }
        if let Some(viewfinder) = report.viewfinder {
            self.viewfinder_stats = Some(viewfinder);
        }
        if let Some(error) = &report.error {
            // Окно возвращаем всё равно (иначе оболочка ждала бы таймаут), но ошибку не прячем.
            self.detach_failures += 1;
            tracing::error!(
                target: LOG_TARGET,
                %error,
                "detach завершился ошибкой: окно возвращено оболочке и квитанция будет подтверждена, но VkSurfaceKHR/swapchain/кольцо кадров/пайплайн утекли; утёкшая VkSurfaceKHR может удерживать подключение производителя к окну"
            );
        }
        report.surface
    }

    fn diagnostics(&mut self) -> BackendDiagnostics {
        let validation = self.context.validation_report();
        let snapshot = self.context.live_objects();
        let live_objects = u64::try_from(snapshot.total()).unwrap_or(u64::MAX);
        let live_objects_by_kind = census_by_kind(&snapshot);
        let display = self.attached.as_ref().and_then(AttachedDisplay::display_info);
        let camera = self.camera_feed.as_ref().map(|_| {
            let live = self.attached.as_ref().and_then(AttachedDisplay::camera_diagnostics);
            match live {
                Some(live) => self
                    .camera_finished
                    .combined_with(&live, self.config.camera_present_history_capacity),
                None => self.camera_finished.clone(),
            }
        });
        let viewfinder = self.viewfinder_diagnostics();
        let still = self.still_diagnostics();
        BackendDiagnostics {
            camera,
            viewfinder,
            still,
            live_objects,
            live_objects_by_kind,
            validation_errors: validation.errors,
            validation_warnings: validation.warnings,
            detach_failures: self.detach_failures,
            display,
            known_deviations: validation.known_deviations,
        }
    }

    fn drain_past_presentations(&mut self) -> Vec<PastPresentation> {
        let drained = match self.attached.as_mut() {
            Some(display) => display.drain_past_presentations(),
            None => Vec::new(),
        };
        // Единственный потребитель записей драйвера — этот метод; снимкам они нужны для доказательства
        // «какой кадр был на экране в момент нажатия», поэтому копия уходит и им.
        if let Some(still) = self.still.as_mut() {
            still.ingest_presentations(&drained);
        }
        drained
    }

    fn take_frame_cpu_timing(&mut self) -> Option<FrameCpuTiming> {
        self.attached.as_mut()?.take_frame_cpu_timing()
    }

    fn drain_frame_gpu_timings(&mut self) -> Vec<FrameGpuTiming> {
        self.attached
            .as_mut()
            .map(AttachedDisplay::drain_frame_gpu_timings)
            .unwrap_or_default()
    }

    fn apply_viewfinder_command(&mut self, command: ViewfinderCommand) -> ViewfinderCommandOutcome {
        if self.camera_feed.is_none() {
            return ViewfinderCommandOutcome::Unsupported;
        }
        let outcome = self.viewfinder_settings.apply(command);
        if outcome == ViewfinderCommandOutcome::Applied
            && let Some(display) = self.attached.as_mut()
        {
            display.apply_viewfinder_settings(self.viewfinder_settings.clone());
        }
        outcome
    }

    fn apply_still_command(&mut self, command: StillCommand) -> StillCommandOutcome {
        let attached = self.attached.is_some();
        match self.still.as_mut() {
            // Снимку без surface некуда деваться: очередь двигается только на показанных кадрах, и `Begin` ждал бы
            // до следующего attach. Итог — сразу (`RendererStopped`), а не после.
            Some(still) => match command {
                StillCommand::Begin(begin) if !attached => {
                    still.refuse_begin_without_surface(begin);
                    StillCommandOutcome::Accepted
                }
                command => still.apply_command(command),
            },
            None => StillCommandOutcome::Unsupported(command),
        }
    }

    fn advance_stills(&mut self) {
        let Some(still) = self.still.as_mut() else {
            return;
        };
        if let Some(feed) = self.attached.as_mut().and_then(AttachedDisplay::take_still_feed) {
            still.ingest_feed(feed);
        }
        still.advance();
    }

    fn apply_ui_overlay_command(&mut self, command: UiOverlayCommand) -> UiOverlayOutcome {
        // UI рисуется только поверх видоискателя камеры (фон стекла — его изображение): без камеры команда
        // не поддержана, а источник из `Install` уничтожается здесь же.
        if self.camera_feed.is_none() {
            return UiOverlayOutcome::Unsupported;
        }
        let installs_source = matches!(command, UiOverlayCommand::Install(_));
        let outcome = self.ui_overlay.apply(command);
        if installs_source && outcome == UiOverlayOutcome::Applied {
            self.prepare_ui_renderer_between_frames();
        }
        outcome
    }
}
