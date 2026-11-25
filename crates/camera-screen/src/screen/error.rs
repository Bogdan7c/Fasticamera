//! Ошибки фасада экрана.

use thiserror::Error;
use ui_kit::tokens::TokenError;

use crate::config::ScreenConfigError;
use crate::widget::error::WidgetError;

/// Экран не удалось создать.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ScreenBuildError {
    #[error("конфиг экрана: {0}")]
    Config(#[from] ScreenConfigError),
    #[error("токены ui-kit: {0}")]
    Tokens(#[from] TokenError),
    #[error("кнопка белого: {0}")]
    WhiteButton(#[from] crate::white_button_layout::WhiteButtonLayoutError),
}

/// Кадр построить не удалось. Фасад превращает её в `UiFrameError::SourceFailed`.
#[derive(Debug, Clone, PartialEq, Error)]
pub(crate) enum FrameBuildError {
    #[error("пересборка сцены отложена после ошибки ещё на {remaining_ms:.0} мс")]
    RebuildBackoff { remaining_ms: f64 },
    #[error("размер surface {width_px} × {height_px} px непригоден")]
    UnusableSurface { width_px: f32, height_px: f32 },
    #[error("масштаб рендера {requested} px/pt не совпадает с масштабом сцены {scene}")]
    ScaleMismatch { requested: f32, scene: f64 },
    #[error("редьюсер отказал в размере surface: {0:?}")]
    SurfaceRejected(crate::outcome::Rejection),
    #[error("виджет: {0}")]
    Widget(#[from] WidgetError),
    #[error("масштаб pt: {0}")]
    Scale(#[from] ui_kit::view::ViewError),
    #[error("сцена: {0}")]
    Scene(#[from] ui_core::scene::SceneError),
    #[error("раскладка: {0}")]
    Layout(#[from] crate::layout::LayoutError),
}
