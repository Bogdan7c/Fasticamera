//! «Мозг» экрана камеры (сессия 12): состояние, действия, эффекты-намерения и чистый редьюсер.
//!
//! Без UI-дерева и рисования (их строит следующий пакет поверх этого crate-а): здесь только то,
//! что решает, **что** на экране и **что** делать камере. Экран ничего не вызывает сам —
//! `fasticamera-app` исполняет [`Effect`] и присылает [`ScreenAction`].
//!
//! # Владельцы
//! * [`ScreenState`] — единственный владелец состояния экрана; меняется только через [`reduce`].
//! * [`ScreenLayout`] — единственный владелец геометрии кадра (решение 93): маска, перевод
//!   «тап ↔ сенсор» и команда рендеру [`FramingIntent`] берут прямоугольник отсюда.
//! * Факты камеры приходят нейтральным [`CameraFacts`]; зависимости от `camera-core` нет.
//!
//! # Карта модулей
//! * [`mode_behaviour`] — поведение режима как данные (затвор, громкость, раскладка, видоискатель, калибровка);
//!   [`calibration`] — раздел калибровки (выбор экрана и геометрия зон); [`look_view`] — факты о виде и плашка проблемы.
//! * [`settings`], [`tools`], [`modes`] — настройки съёмки, инструменты как данные (М3), реестр режимов (М1).
//! * [`shutter`] — машина состояний затвора (М2), запрос снимка; `saving` — набор снимков, ждущих сохранения.
//! * [`focus`], [`exposure`], [`level`], [`orientation`] — фокус и блокировка, EV, уровень, поворот значков.
//! * [`layout`], [`viewfinder_map`] — раскладка и перевод экран ↔ сенсор.
//! * [`zoom`], [`zoom_config`], [`zoom_labels`] — зум (сессия 13): владелец значения, числа, правило подписей.
//! * [`action`], [`effect`], [`outcome`], `reduce` — вход, выход и итог.
//! * [`screen`] — фасад [`CameraScreen`] (сцена `ui-core`, виджеты на элементах `ui-kit`, кадр `DrawList`);
//!   `widget` — по модулю на каждый виджет; [`widgets_config`] — числа их раскладки.

pub mod action;
pub mod calibration;
pub mod config;
pub mod effect;
pub mod exposure;
pub mod facts;
pub mod focus;
pub mod layout;
pub mod level;
pub mod look_view;
pub mod mode_behaviour;
pub mod modes;
pub mod orientation;
pub mod outcome;
mod reduce;
mod saving;
pub mod screen;
pub mod settings;
pub mod shutter;
mod state;
pub mod thumbnail;
mod time;
pub mod tools;
pub mod viewfinder_map;
pub mod white;
pub mod white_button_layout;
pub mod white_config;
pub mod white_display;
pub mod white_facts;
pub mod white_layout;
pub mod white_row;
mod widget;
pub mod widgets_config;
pub mod zoom;
pub mod zoom_config;
pub mod zoom_labels;

pub use action::{KeyPhase, ScreenAction};
pub use calibration::{
    CalibrationConfig, CalibrationConfigError, CalibrationLayout, CalibrationLayoutConfig, CalibrationLayoutError,
    CalibrationMenu, CalibrationMenuItem, CalibrationState, CalibrationZones, CleanViewState, ControlViewKind,
    DeleteConfirmation, Naming, PendingText, TextFlow, WheelPadPlacement, ZoneFractions,
};
pub use config::{ScreenConfig, ScreenConfigError, ToolsConfig};
pub use effect::{Effect, ScreenCameraIntent};
pub use facts::{
    ActiveLens, CameraFacts, CameraPresence, FlashAvailability, FocusPhase, LensFacts, LensSwitch, LensSwitchFailure,
    UnavailableReason,
};
pub use layout::{FramingIntent, LayoutConfig, LayoutError, NormalizedScreenRect, ScreenLayout};
pub use look_view::{LookView, ProblemBanner, ProblemKind};
pub use mode_behaviour::{
    CalibrationAvailability, CalibrationSurface, LayoutPolicy, ModeBehaviour, ShutterPresence, ViewfinderPlacement,
    VolumeKeyRole,
};
pub use outcome::{ActionOutcome, NoChangeReason, Rejection, RejectionCounters};
pub use reduce::{ScreenTransition, reduce};
pub use screen::{
    CameraScreen, GestureCounters, ScreenBuildError, ScreenDiagnostics, ScreenTouch, WhiteCardTargets,
    WhiteTouchTargets,
};
pub use state::{PanelState, ScreenState, ShutterVisibility, ViewfinderUnavailable};
pub use white::{WhiteAxis, WhiteScreenState};
pub use white_button_layout::{WhiteButtonLayout, WhiteButtonLayoutError};
pub use white_config::{
    WhiteButtonSide, WhiteConfigError, WhiteLight, WhiteLightIcon, WhiteLightIndex, WhiteScreenConfig,
};
pub use white_facts::{WhiteFacts, WhiteFollowFacts, WhiteRange, WhiteTakeFacts, WhiteTakeState};
pub use white_layout::{WhiteCardLayout, WhiteLayoutError};
pub use white_row::{SOURCE_BUTTON_COUNT, WhiteRowSlot};
pub use widgets_config::{WidgetConfigError, WidgetLayoutConfig};
pub use zoom::{FlipMark, ZoomState};
pub use zoom_config::{ZoomConfig, ZoomConfigError};

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
#[cfg(test)]
#[path = "layout_tests.rs"]
mod layout_tests;
#[cfg(test)]
#[path = "level_tests.rs"]
mod level_tests;
#[cfg(test)]
mod look_test_support;
#[cfg(test)]
#[path = "saving_unit_tests.rs"]
mod saving_unit_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
#[path = "viewfinder_map_tests.rs"]
mod viewfinder_map_tests;
#[cfg(test)]
#[path = "white_config_tests.rs"]
mod white_config_tests;
#[cfg(test)]
#[path = "white_display_tests.rs"]
mod white_display_tests;
