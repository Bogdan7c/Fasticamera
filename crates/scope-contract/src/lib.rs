//! Нейтральный словарь цветовых приборов Fasticamera (решения 183, 188–190).
//!
//! Здесь только типы и проверка значений, никакой математики: правила попадания и показа живут
//! в `scope-model` (эталон на CPU) и `isp-vulkan` (GPU), экран только шлёт [`ScopeIntent`].
//!
//! Приборы меряют картинку ПОСЛЕ вида — ровно то, что на превью (решение 189), и считаются на каждом
//! новом кадре камеры вместе с показанным кадром (решение 190).
//!
//! Принцип: непроверенное значение не существует. Размер картинки прибора — newtype с `try_new`.
//! Crate — лист графа: не зависит ни от одного crate-а проекта.

mod intent;
mod kind;
mod level_lines;
mod scan;
mod size;

pub use intent::{ScopeIntent, ScopeRequest};
pub use kind::{ScopeKind, ScopePictureShape, VectorscopeAxes};
pub use level_lines::{LevelLine, WAVEFORM_LEVEL_LINES};
pub use scan::ScopeScan;
pub use size::{MAX_SCOPE_IMAGE_SIDE_PX, ScopeImageSize, ScopeImageSizeError};
