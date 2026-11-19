//! Жесты и арбитраж по модели UIKit (`docs/ios-camera/behavior.md` раздел 1).
//!
//! * [`RecognizerSpec`] — закрытый набор видов (тап, долгий тап, пан, щипок, свайп, нажатие) и их
//!   пороги; никаких трейт-объектов и замыканий;
//! * [`GestureArena`] — добавление распознавателей к элементам, отношения «требует провала» и
//!   «одновременно», вход касаний и кадров, выход событий [`GestureEvent`];
//! * [`GestureState`], [`GestureData`] — состояния и данные жестов.
//!
//! Арена не знает ни дерева элементов, ни экрана: она получает [`crate::id::NodeId`] и путь хит-теста
//! (срез от самого глубокого элемента к корню), а возвращает события с намерением экрана `I`.

mod arena;
mod error;
mod kinds;
mod relations;
mod spec;
mod state;
mod touches;

#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests_arena_api;
#[cfg(test)]
mod tests_scenarios;
#[cfg(test)]
mod tests_scenarios_more;

pub use arena::{GestureArena, GestureConfig, PointerDisposition};
pub use error::GestureError;
pub use relations::RelationError;
pub use spec::{
    ExtraFingers, LongPressSpec, PanDirection, PanSpec, PinchSpec, PressSpec, RecognizerSpec, SpecError, SwipeSpec,
    TapCount, TapSpec,
};
pub use state::{GestureData, GestureEvent, GestureState, PanTranslation, RecognizerId, SwipeDirection};
