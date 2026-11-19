//! Ошибка сборки/кадра виджетов: причина сохраняется текстом с именем элемента, не глотается.

use std::fmt::Display;

use thiserror::Error;
use ui_core::scene::SceneError;
use ui_kit::view::ViewError;

/// Что пошло не так в виджете.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum WidgetError {
    /// Сцена `ui-core` (дерево, вёрстка, жесты, анимации).
    #[error("сцена: {0}")]
    Scene(#[from] SceneError),
    /// Сборка вида (`ui_kit::view`): стекло, подписи, прозрачность.
    #[error("вид: {0}")]
    View(#[from] ViewError),
    /// Элемент `ui-kit` отверг вызов.
    #[error("элемент {element}: {reason}")]
    Element { element: &'static str, reason: String },
    /// Раскладка экрана ещё не построена (не было размера surface).
    #[error("раскладка экрана ещё не построена")]
    NoLayout,
    /// Данные не сошлись (индекс вне списка и т. п.): ошибка сборки, а не пользователя.
    #[error("{0}")]
    Inconsistent(String),
}

/// Адаптер `map_err(element("shutter"))`: ошибка элемента → [`WidgetError::Element`].
pub(crate) fn element<E: Display>(name: &'static str) -> impl FnOnce(E) -> WidgetError {
    move |error| WidgetError::Element {
        element: name,
        reason: error.to_string(),
    }
}
