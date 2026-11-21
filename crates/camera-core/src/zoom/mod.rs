//! Модель зума движка камеры (сессия 13, механизм М6 «политики зума»).
//!
//! Чистые функции без ввода-вывода:
//! * [`build_zoom_catalog`] — из каталога камер строит шкалу коэффициентов каждого направления
//!   (какая камера с какого коэффициента, где кнопки, какой предел);
//! * [`select_lens`] — по шкале, запрошенному коэффициенту и текущей камере выбирает камеру с
//!   гистерезисом, чтобы граница между объективами не мигала.
//!
//! Словарь (`ZoomRatio`, `ZoomCatalog`, `LensIntent`…) живёт в `camera-contract`: его читают и
//! экран, и рендер. Здесь — только политика движка. Все пороги — параметры [`ZoomConfig`].

mod catalog;
mod config;
mod selector;
mod standby;

pub use catalog::{ZoomCatalogBuild, ZoomDiagnostic, build_zoom_catalog, build_zoom_catalog_reporting};
pub use config::{ZoomConfig, ZoomConfigError};
pub use selector::{SwitchCost, select_initial_lens, select_lens};
pub use standby::standby_candidates;

#[cfg(test)]
mod catalog_tests;
#[cfg(test)]
mod config_tests;
#[cfg(test)]
mod selector_tests;
#[cfg(test)]
mod standby_tests;
