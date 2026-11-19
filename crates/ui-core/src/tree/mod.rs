//! Дерево элементов экрана камеры: арена с поколениями, свойства, хит-тест.
//!
//! Владеет деревом экран (`camera-screen`), `ui-core` даёт механизм. Дерево хранит только то,
//! что нужно механике: иерархию, порядок рисования (позже в списке детей — сверху), визуальные
//! свойства, правила вёрстки и вычисленные рамки. Рисование и жесты сюда не входят: хит-тест
//! отдаёт путь [`UiTree::hit_path`], который идёт в арену жестов.
//!
//! # Инварианты
//! * Идентификатор [`crate::id::NodeId`] живёт ровно пока жив элемент; место удалённого
//!   переиспользуется с новым поколением, старый идентификатор даёт [`TreeError::UnknownNode`].
//! * Корень создаётся вместе с деревом и не удаляется.
//! * Визуальные свойства (масштаб, поворот) вёрстку не затрагивают — только хит-тест и
//!   [`UiTree::visual_bounds`].
//! * Рисование получает готовую геометрию из [`UiTree::draw_order`]: преобразование предков уже
//!   составлено той же композицией, что у хит-теста и `visual_bounds`.
//! * Рамки ([`UiTree::frame`]) обновляет только [`UiTree::layout`]; после изменения правил рамки
//!   устарели, пока вёрстка не пересчитана.

mod arena;
mod draw_order;
mod error;
mod hit_test;
mod layout_slots;
mod node_transform;
mod props;
mod transform;

pub use arena::UiTree;
pub use draw_order::NodeVisual;
pub use error::TreeError;
pub use hit_test::HitTestConfig;
pub use node_transform::NodeTransform;
pub use props::{HitTestPolicy, Visibility};
pub use transform::ElementScale;
