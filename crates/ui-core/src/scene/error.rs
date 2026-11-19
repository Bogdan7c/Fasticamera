//! Ошибки сцены: каждая подсистема сохраняет свою причину.

use thiserror::Error;

use crate::animator::AnimatorError;
use crate::gesture::GestureError;
use crate::input::PointScaleError;
use crate::layout::LayoutError;
use crate::tree::TreeError;

/// Ошибка операции [`super::UiScene`].
#[derive(Debug, Clone, PartialEq, Error)]
pub enum SceneError {
    #[error("дерево: {0}")]
    Tree(#[from] TreeError),
    #[error("вёрстка: {0}")]
    Layout(#[from] LayoutError),
    #[error("жесты: {0}")]
    Gesture(#[from] GestureError),
    #[error("анимации: {0}")]
    Animator(#[from] AnimatorError),
    #[error("масштаб pt: {0}")]
    PointScale(#[from] PointScaleError),
}
