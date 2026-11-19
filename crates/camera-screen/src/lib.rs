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
//! * [`settings`], [`tools`], [`modes`] — настройки съёмки, инструменты как данные (М3), реестр режимов (М1).
//! * [`shutter`] — машина состояний затвора (М2), запрос снимка.
//! * [`focus`], [`exposure`], [`level`], [`orientation`] — фокус и блокировка, EV, уровень, поворот значков.
//! * [`layout`], [`viewfinder_map`] — раскладка и перевод экран ↔ сенсор.
//! * [`action`], [`effect`], [`outcome`], `reduce` — вход, выход и итог.
//! * [`screen`] — фасад [`CameraScreen`] (сцена `ui-core`, виджеты на элементах `ui-kit`, кадр `DrawList`);
//!   `widget` — по модулю на каждый виджет; [`widgets_config`] — числа их раскладки.

pub mod action;
pub mod config;
pub mod effect;
pub mod exposure;
pub mod facts;
pub mod focus;
pub mod layout;
pub mod level;
pub mod modes;
pub mod orientation;
pub mod outcome;
mod reduce;
pub mod screen;
pub mod settings;
pub mod shutter;
mod state;
mod time;
pub mod tools;
pub mod viewfinder_map;
mod widget;
pub mod widgets_config;

pub use action::{KeyPhase, ScreenAction};
pub use config::{ScreenConfig, ScreenConfigError, ToolsConfig};
pub use effect::{Effect, ScreenCameraIntent};
pub use facts::{CameraFacts, CameraPresence, FocusPhase, UnavailableReason};
pub use layout::{FramingIntent, LayoutConfig, LayoutError, NormalizedScreenRect, ScreenLayout};
pub use outcome::{ActionOutcome, NoChangeReason, Rejection, RejectionCounters};
pub use reduce::{ScreenTransition, reduce};
pub use screen::{CameraScreen, GestureCounters, ScreenBuildError, ScreenDiagnostics, ScreenTouch};
pub use state::{PanelState, ScreenState, ShutterVisibility, ViewfinderUnavailable};
pub use widgets_config::{WidgetConfigError, WidgetLayoutConfig};

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
mod test_support;
#[cfg(test)]
#[path = "viewfinder_map_tests.rs"]
mod viewfinder_map_tests;
