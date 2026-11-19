//! `ScreenState` — единственный владелец того, что на экране камеры.
//!
//! Менять состояние можно только через [`crate::reduce`]; снаружи — чтение запросами по смыслу
//! («виден ли затвор», «какие значки показать»), а не доступом к полям.

use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::surface::SurfaceSize;
use thiserror::Error;

use crate::config::{ScreenConfig, ScreenConfigError};
use crate::exposure::ExposureState;
use crate::facts::{CameraFacts, CameraPresence};
use crate::focus::FocusState;
use crate::layout::{FramingIntent, ScreenLayout};
use crate::level::LevelReading;
use crate::modes::{CaptureMode, ModeDescriptor, ModeRegistry};
use crate::orientation::icon_rotation_clockwise_degrees;
use crate::outcome::RejectionCounters;
use crate::settings::{CaptureSettings, TimerSetting};
use crate::shutter::{ShotRequestId, ShutterState, ThumbnailState};
use crate::tools::ToolId;
use crate::viewfinder_map::{MappingError, ViewfinderMapping};

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

/// Откуда открыто подменю: от этого зависит, куда вести после выбора варианта и по «Назад».
///
/// Подменю из меню Controls — часть шторки: после выбора возвращаемся в меню. Подменю, открытое
/// тапом по значку над кадром (таймер), никакого меню за собой не имеет: после выбора панель
/// закрывается, иначе пользователь увидел бы незапрошенное меню Controls, а затвор ушёл бы под шторку.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum SubmenuOrigin {
    /// Пункт меню Controls «морфнулся» в подменю.
    #[default]
    ControlsMenu,
    /// Тап по значку над кадром.
    StripIcon,
}

/// Виден ли затвор (решение 94).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutterVisibility {
    Visible,
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
    /// Откуда открыто текущее подменю; осмысленно только при `PanelState::Submenu`
    /// (выставляется при каждом входе в подменю).
    pub(crate) submenu_origin: SubmenuOrigin,
    pub(crate) settings: CaptureSettings,
    pub(crate) exposure: ExposureState,
    pub(crate) focus: FocusState,
    pub(crate) facts: CameraFacts,
    pub(crate) orientation: DeviceOrientation,
    pub(crate) level: LevelReading,
    pub(crate) shutter: ShutterState,
    pub(crate) thumbnail: ThumbnailState,
    pub(crate) next_request: u64,
    /// Срок, после которого ждать `CaptureFinished` бессмысленно (`CaptureConfig`); `Some` только
    /// пока затвор в `Capturing`.
    pub(crate) capture_deadline: Option<MonotonicNanos>,
    /// Сколько снимков экран объявил потерянными (камера пропала или итог не пришёл в срок).
    pub(crate) lost_captures: u32,
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
            submenu_origin: SubmenuOrigin::default(),
            settings: CaptureSettings::default(),
            exposure: ExposureState::new(&config.exposure),
            focus: FocusState::default(),
            facts: CameraFacts::STARTING,
            orientation: DeviceOrientation::Portrait,
            level: LevelReading::Unknown,
            shutter: ShutterState::Idle,
            thumbnail: ThumbnailState::Empty,
            next_request: 1,
            capture_deadline: None,
            lost_captures: 0,
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

    /// Сколько снимков потеряно: камера пропала во время съёмки или итог не пришёл в срок
    /// (`CaptureConfig::result_timeout_ms`). Потеря считается, а не глотается молча.
    pub fn lost_captures(&self) -> u32 {
        self.lost_captures
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
            .collect()
    }

    /// Пункты меню Controls текущего режима.
    pub fn menu_tools(&self) -> &[ToolId] {
        self.current_descriptor()
            .map_or(&[], |descriptor| descriptor.tools.menu())
    }

    /// Виден ли затвор (решение 94: под шторкой при открытом меню или подменю).
    pub fn shutter_visibility(&self) -> ShutterVisibility {
        match self.panel {
            PanelState::ControlsMenu | PanelState::Submenu(_) => ShutterVisibility::HiddenUnderDrawer,
            PanelState::Closed | PanelState::ExposurePanel => ShutterVisibility::Visible,
        }
    }

    /// Показывать ли текст «камера недоступна».
    pub fn is_camera_unavailable(&self) -> bool {
        matches!(self.facts.presence, CameraPresence::Unavailable { .. })
    }

    /// Целевой угол поворота значков, градусы по часовой (раскладка не меняется).
    pub fn icon_rotation_clockwise_degrees(&self) -> f32 {
        icon_rotation_clockwise_degrees(self.orientation)
    }

    /// Команда рендеру для текущего соотношения; `None`, пока нет раскладки.
    pub fn framing_intent(&self) -> Option<FramingIntent> {
        self.layout.map(|layout| layout.framing_intent(self.settings.aspect))
    }

    /// Перевод «экран ↔ сенсор» для текущего кадра. Единственное место, где он собирается.
    pub fn viewfinder_mapping(&self) -> Result<ViewfinderMapping, ViewfinderUnavailable> {
        let layout = self.layout.as_ref().ok_or(ViewfinderUnavailable::NoLayout)?;
        let stream = self
            .facts
            .stream
            .as_ref()
            .ok_or(ViewfinderUnavailable::NoStreamGeometry)?;
        let intent = layout.framing_intent(self.settings.aspect);
        let frame = layout.frame_rect(self.settings.aspect);
        ViewfinderMapping::new(frame, &intent.crop, stream).map_err(ViewfinderUnavailable::Mapping)
    }

    /// Выдать следующий номер запроса снимка.
    pub(crate) fn allocate_shot_request(&mut self) -> ShotRequestId {
        let request = ShotRequestId(self.next_request);
        self.next_request = self.next_request.saturating_add(1);
        request
    }
}
