//! `ScreenState` — единственный владелец того, что на экране камеры.
//!
//! Менять состояние можно только через [`crate::reduce`]; снаружи — чтение запросами по смыслу
//! («виден ли затвор», «какие значки показать»), а не доступом к полям.

use std::sync::Arc;

use camera_contract::ids::LensFacing;
use camera_contract::viewfinder_crop::{ViewfinderCrop, ZoomFactor};
use camera_contract::zoom::{FramedCrop, LensGroup, PreviewMirror, ZoomCatalog, ZoomFit, ZoomFraming};
use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::SurfaceSize;
use thiserror::Error;

use crate::config::{ScreenConfig, ScreenConfigError};
use crate::exposure::ExposureState;
use crate::facts::{CameraFacts, CameraPresence, FlashAvailability};
use crate::focus::FocusState;
use crate::layout::{FramingIntent, ScreenLayout};
use crate::level::LevelReading;
use crate::modes::{CaptureMode, ModeDescriptor, ModeRegistry};
use crate::orientation::icon_rotation_clockwise_degrees;
use crate::outcome::RejectionCounters;
use crate::saving::SavingShots;
use crate::settings::{CaptureSettings, FlashMode, TimerSetting};
use crate::shutter::{ShotRequestId, ShutterState, ThumbnailState};
use crate::tools::ToolId;
use crate::viewfinder_map::{MappingError, MirrorMode, StreamGeometry, ViewfinderMapping};
use crate::zoom::ZoomState;

/// Открытая панель. Одновременно открыта не больше одной.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelState {
    #[default]
    Closed,
    /// Меню Controls (сетка инструментов).
    ControlsMenu,
    /// Подменю инструмента (пункт меню «морфится» в него).
    Submenu(ToolId),
    /// Панель Exposure (линейка EV).
    ExposurePanel,
}

/// Откуда открыта карточка инструмента (подменю или панель Exposure, решение 103).
///
/// От этого зависит, куда ведёт левая кнопка карточки («‹» / «×»): карточка из меню Controls — часть
/// шторки, «‹» возвращает в меню; карточка, открытая тапом по значку над кадром, меню за собой не
/// имеет, «×» закрывает панель (иначе пользователь увидел бы незапрошенное меню). Выбор варианта
/// карточку не закрывает ни при каком происхождении (Apple 27: «Tap Aspect, then tap a ratio»).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum CardOrigin {
    /// Пункт меню Controls «морфнулся» в карточку.
    #[default]
    ControlsMenu,
    /// Тап по значку над кадром.
    StripIcon,
}

/// Виден ли затвор и где (решения 94, 103).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutterVisibility {
    /// Панелей нет: затвор на обычном месте.
    Visible,
    /// Открыта карточка инструмента: ряд затвора поднят над карточкой, затвор виден и снимает.
    RaisedAboveCard,
    /// Меню Controls открыто: затвор уходит под шторку и не нажимается.
    HiddenUnderDrawer,
}

/// Почему перевод координат сейчас недоступен.
#[derive(Debug, Clone, Copy, PartialEq, Error)]
pub enum ViewfinderUnavailable {
    #[error("раскладка ещё не построена (нет размера surface)")]
    NoLayout,
    #[error("нет геометрии потока камеры")]
    NoStreamGeometry,
    #[error("перевод координат невозможен: {0}")]
    Mapping(MappingError),
}

/// Состояние экрана камеры.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenState {
    pub(crate) registry: ModeRegistry,
    pub(crate) mode: CaptureMode,
    pub(crate) panel: PanelState,
    /// Откуда открыта текущая карточка; осмысленно только при `Submenu`/`ExposurePanel`
    /// (выставляется при каждом входе в карточку).
    pub(crate) card_origin: CardOrigin,
    pub(crate) settings: CaptureSettings,
    pub(crate) exposure: ExposureState,
    pub(crate) focus: FocusState,
    /// Зум: единственный владелец значения, которое видит пользователь (сессия 13).
    pub(crate) zoom: ZoomState,
    pub(crate) facts: CameraFacts,
    pub(crate) orientation: DeviceOrientation,
    pub(crate) level: LevelReading,
    pub(crate) shutter: ShutterState,
    pub(crate) thumbnail: ThumbnailState,
    pub(crate) next_request: u64,
    /// Срок, после которого ждать `CaptureSecured` бессмысленно (`CaptureConfig::capture_timeout_ms`); `Some`
    /// только пока затвор в `Capturing`.
    pub(crate) capture_deadline: Option<MonotonicNanos>,
    /// Захваченные снимки, ждущие сохранения (решение 140); у каждого свой срок `saving_timeout_ms`.
    pub(crate) saving: SavingShots,
    /// Сколько снимков экран объявил потерянными (камера пропала до захвата, захват или сохранение не
    /// уложились в срок).
    pub(crate) lost_captures: u32,
    /// Сколько захваченных снимков не удалось взять на учёт из-за переполнения набора. Недостижимо при
    /// исправной работе (нажатие при полном наборе отвергается заранее), счётчик — страховка от тихой потери.
    pub(crate) saving_overflows: u32,
    /// Последний известный момент времени (метки действий не убывают). Нужен, когда индикатор
    /// создаётся по факту камеры, а не по жесту: у факта своей метки времени нет.
    pub(crate) last_time: MonotonicNanos,
    pub(crate) surface: Option<SurfaceSize>,
    pub(crate) layout: Option<ScreenLayout>,
    pub(crate) rejections: RejectionCounters,
}

impl ScreenState {
    /// Начальное состояние. Проверяет конфиг: невалидный конфиг не даёт создать экран.
    pub fn new(config: &ScreenConfig) -> Result<Self, ScreenConfigError> {
        config.validate()?;
        let registry = ModeRegistry::implemented(config.tools.tool_set()?);
        Ok(Self {
            mode: registry.default_mode(),
            registry,
            panel: PanelState::Closed,
            card_origin: CardOrigin::default(),
            settings: CaptureSettings::default(),
            exposure: ExposureState::new(&config.exposure),
            focus: FocusState::default(),
            zoom: ZoomState::new(),
            facts: CameraFacts::STARTING,
            orientation: DeviceOrientation::Portrait,
            level: LevelReading::Unknown,
            shutter: ShutterState::Idle,
            thumbnail: ThumbnailState::Empty,
            next_request: 1,
            capture_deadline: None,
            saving: SavingShots::new(config.capture.max_saving_shots),
            lost_captures: 0,
            saving_overflows: 0,
            last_time: MonotonicNanos::from_nanos(0),
            surface: None,
            layout: None,
            rejections: RejectionCounters::default(),
        })
    }

    pub fn mode(&self) -> CaptureMode {
        self.mode
    }

    pub fn panel(&self) -> PanelState {
        self.panel
    }

    pub fn settings(&self) -> &CaptureSettings {
        &self.settings
    }

    pub fn exposure(&self) -> &ExposureState {
        &self.exposure
    }

    pub fn focus(&self) -> &FocusState {
        &self.focus
    }

    /// Зум экрана: направление, коэффициент, колесо, щипок.
    pub fn zoom(&self) -> &ZoomState {
        &self.zoom
    }

    /// Шкала зума текущего направления; `None` — каталога нет или в нём нет этого направления
    /// (тогда ленты зума нет, а действия зума получают отказ `ZoomUnavailable`).
    pub fn zoom_group(&self) -> Option<&LensGroup> {
        self.facts.lens.catalog.as_ref()?.group(self.zoom.facing())
    }

    /// Куда перевернётся камера по кнопке: с задней — на фронтальную, с любой другой — на заднюю.
    pub fn flip_target_facing(&self) -> LensFacing {
        match self.zoom.facing() {
            LensFacing::Back => LensFacing::Front,
            LensFacing::Front | LensFacing::External => LensFacing::Back,
        }
    }

    /// Шкала зума направления, на которое переворачивает кнопка; `None` — такой камеры в каталоге нет.
    pub fn flip_target_group(&self) -> Option<&LensGroup> {
        self.facts.lens.catalog.as_ref()?.group(self.flip_target_facing())
    }

    /// Есть ли куда переворачивать. Каталог опубликован и в нём нет второго направления (например, у
    /// устройства только задние камеры) — кнопка не рисуется и не ловит касания: «не поддерживается»
    /// показываем честно (инвариант 6), а не мёртвой кнопкой. Пока каталога нет (запуск), кнопка видна как
    /// раньше: тап получит отказ `ZoomUnavailable`, решать «нет камеры» ещё не по чему.
    pub fn can_flip_camera(&self) -> bool {
        match self.facts.lens.catalog.as_ref() {
            Some(catalog) => catalog.group(self.flip_target_facing()).is_some(),
            None => true,
        }
    }

    /// Каталог зума для кадрирования: пустой, пока движок его не опубликовал (кадр целиком по центру).
    fn zoom_catalog(&self) -> Arc<ZoomCatalog> {
        self.facts
            .lens
            .catalog
            .clone()
            .unwrap_or_else(|| Arc::new(ZoomCatalog::default()))
    }

    /// Последние факты камеры, которые учёл экран.
    pub fn camera_facts(&self) -> &CameraFacts {
        &self.facts
    }

    pub fn camera_presence(&self) -> CameraPresence {
        self.facts.presence
    }

    pub fn shutter(&self) -> ShutterState {
        self.shutter
    }

    pub fn thumbnail(&self) -> ThumbnailState {
        self.thumbnail
    }

    pub fn level(&self) -> LevelReading {
        self.level
    }

    /// Сколько снимков потеряно: камера пропала во время захвата, кадр не захвачен или снимок не сохранён в срок
    /// (`CaptureConfig::capture_timeout_ms` / `saving_timeout_ms`). Потеря считается, а не глотается молча.
    pub fn lost_captures(&self) -> u32 {
        self.lost_captures
    }

    /// Сколько захваченных снимков сейчас сохраняется (затвор при этом свободен).
    pub fn saving_shots(&self) -> usize {
        self.saving.len()
    }

    /// Сколько захваченных снимков не взято на учёт из-за переполнения набора (см. поле).
    pub fn saving_overflows(&self) -> u32 {
        self.saving_overflows
    }

    pub fn rejections(&self) -> &RejectionCounters {
        &self.rejections
    }

    /// Раскладка кадров; `None`, пока не пришёл размер surface.
    /// Последний принятый размер surface (аддитивный геттер P5b: фасаду нужно знать, пересчитывать ли раскладку).
    pub fn surface_size(&self) -> Option<SurfaceSize> {
        self.surface
    }

    pub fn layout(&self) -> Option<&ScreenLayout> {
        self.layout.as_ref()
    }

    /// Режимы карусели (из реестра реализованных).
    pub fn mode_carousel(&self) -> &[ModeDescriptor] {
        self.registry.descriptors()
    }

    fn current_descriptor(&self) -> Option<&ModeDescriptor> {
        self.registry.descriptor(self.mode)
    }

    /// Значки над кадром, которые надо показать сейчас: набор режима, где таймер виден только
    /// когда он включён (`behavior.md` 2.2).
    pub fn visible_strip_tools(&self) -> Vec<ToolId> {
        let Some(descriptor) = self.current_descriptor() else {
            return Vec::new();
        };
        descriptor
            .tools
            .strip()
            .iter()
            .copied()
            .filter(|tool| *tool != ToolId::Timer || self.settings.timer != TimerSetting::Off)
            .filter(|tool| self.is_tool_available(*tool))
            .collect()
    }

    /// Пункты меню Controls текущего режима, которые можно показать сейчас (без инструментов, которых у камеры
    /// нет: вспышка, пока `FlashAvailability::Unavailable`). Остальные пункты идут подряд в порядке меню.
    pub fn menu_tools(&self) -> Vec<ToolId> {
        let Some(descriptor) = self.current_descriptor() else {
            return Vec::new();
        };
        descriptor
            .tools
            .menu()
            .iter()
            .copied()
            .filter(|tool| self.is_tool_available(*tool))
            .collect()
    }

    /// Есть ли у камеры то, чем инструмент пользуется (инвариант 6: нет возможности — нет значка и пункта).
    pub(crate) fn is_tool_available(&self, tool: ToolId) -> bool {
        match tool {
            ToolId::Flash => self.facts.flash == FlashAvailability::Available,
            ToolId::Raw
            | ToolId::Timer
            | ToolId::Exposure
            | ToolId::Aspect
            | ToolId::Grid
            | ToolId::Level
            | ToolId::Live
            | ToolId::Styles
            | ToolId::Night => true,
        }
    }

    /// Вспышка, которая действительно уйдёт в снимок: выбор пользователя, но только если камера вспышку
    /// поддерживает. Выбранная `On`, когда вспышка пропала, в снимок не попадает (`Off`).
    pub fn effective_flash(&self) -> FlashMode {
        match self.facts.flash {
            FlashAvailability::Available => self.settings.flash,
            FlashAvailability::Unavailable => FlashMode::Off,
        }
    }

    /// Где затвор (решения 94, 103): под шторкой при меню, поднят при карточке, иначе на месте.
    pub fn shutter_visibility(&self) -> ShutterVisibility {
        match self.panel {
            PanelState::ControlsMenu => ShutterVisibility::HiddenUnderDrawer,
            PanelState::Submenu(_) | PanelState::ExposurePanel => ShutterVisibility::RaisedAboveCard,
            PanelState::Closed => ShutterVisibility::Visible,
        }
    }

    /// Открыта ли карточка инструмента (подменю или Exposure).
    pub fn is_tool_card_open(&self) -> bool {
        matches!(self.panel, PanelState::Submenu(_) | PanelState::ExposurePanel)
    }

    /// Лежит ли под открытой карточкой меню Controls: карточка открыта из меню, левая кнопка — «‹» назад в
    /// меню, а сама шторка остаётся под карточкой. У карточки, открытой значком над кадром, меню нет (`false`),
    /// левая кнопка — «×». Виджеты читают намерение, а не поле `card_origin`.
    pub(crate) fn card_returns_to_menu(&self) -> bool {
        self.is_tool_card_open() && self.card_origin == CardOrigin::ControlsMenu
    }

    /// Скрыт ли нижний ряд (миниатюра, капсула режимов, кнопка меню): растворяется, пока открыта
    /// карточка (решение 103). Под меню ряд закрывает сама шторка, это не «скрытие» ряда.
    pub fn is_bottom_row_hidden(&self) -> bool {
        self.is_tool_card_open()
    }

    /// Идёт ли переключение объектива по фактам движка (сессия 13). Картинку перехода рисует `BackdropContinuity`
    /// рендера; это факт для логики (например, диагностики и тестов), а не источник размытия.
    pub fn is_lens_switching(&self) -> bool {
        matches!(self.facts.lens.switch, crate::facts::LensSwitch::Switching { .. })
    }

    /// Показывать ли текст «камера недоступна».
    pub fn is_camera_unavailable(&self) -> bool {
        matches!(self.facts.presence, CameraPresence::Unavailable { .. })
    }

    /// Целевой угол поворота значков, градусы по часовой (раскладка не меняется).
    pub fn icon_rotation_clockwise_degrees(&self) -> f32 {
        icon_rotation_clockwise_degrees(self.orientation)
    }

    /// Входы формулы кадрирования на сейчас: соотношение, направление, зум, шкала камер. Те же значения, что
    /// экран отдаёт рендеру (`framing_intent`), поэтому снимок кадрируется как видоискатель.
    pub(crate) fn zoom_framing(&self) -> ZoomFraming {
        ZoomFraming {
            aspect: self.settings.aspect.crop_aspect(),
            facing: self.zoom.facing(),
            ratio: self.zoom.ratio(),
            catalog: self.zoom_catalog(),
        }
    }

    /// Команда рендеру для текущего соотношения и зума; `None`, пока нет раскладки.
    pub fn framing_intent(&self) -> Option<FramingIntent> {
        self.layout.map(|layout| {
            layout.framing_intent(
                self.settings.aspect,
                self.zoom.facing(),
                self.zoom.ratio(),
                self.zoom_catalog(),
            )
        })
    }

    /// Кадрирование кадра **активной камеры** по общей с рендером формуле. Нет активной камеры (потока нет) —
    /// кадр целиком по центру без зеркала, как посчитает и рендер для камеры вне каталога.
    fn framed_crop(&self, framing: &ZoomFraming) -> FramedCrop {
        match &self.facts.lens.active {
            Some(active) => framing.crop_for_camera(&active.camera),
            None => FramedCrop {
                crop: ViewfinderCrop::centered(framing.aspect, ZoomFactor::NONE),
                mirror: PreviewMirror::None,
                shown_ratio: framing.ratio,
                fit: ZoomFit::CameraNotInCatalog,
            },
        }
    }

    /// Перевод «экран ↔ сенсор» для текущего кадра. Единственное место, где он собирается.
    pub fn viewfinder_mapping(&self) -> Result<ViewfinderMapping, ViewfinderUnavailable> {
        let layout = self.layout.as_ref().ok_or(ViewfinderUnavailable::NoLayout)?;
        let stream = self
            .facts
            .stream
            .as_ref()
            .ok_or(ViewfinderUnavailable::NoStreamGeometry)?;
        let intent = layout.framing_intent(
            self.settings.aspect,
            self.zoom.facing(),
            self.zoom.ratio(),
            self.zoom_catalog(),
        );
        let frame = layout.frame_rect(self.settings.aspect);
        // Кадрирование и зеркало берутся из той же формулы, что считает рендер для кадра этой камеры
        // (решение 93). `StreamGeometry::mirror` от приложения при этом ИГНОРИРУЕТСЯ: два источника зеркала
        // дали бы двойное отражение (тап слева на фронталке ушёл бы не туда).
        let framed = self.framed_crop(&intent.zoom);
        let stream = StreamGeometry {
            mirror: match framed.mirror {
                PreviewMirror::None => MirrorMode::None,
                PreviewMirror::Horizontal => MirrorMode::Horizontal,
            },
            ..*stream
        };
        ViewfinderMapping::new(frame, &framed.crop, &stream).map_err(ViewfinderUnavailable::Mapping)
    }

    /// Выдать следующий номер запроса снимка.
    pub(crate) fn allocate_shot_request(&mut self) -> ShotRequestId {
        let request = ShotRequestId(self.next_request);
        self.next_request = self.next_request.saturating_add(1);
        request
    }
}
