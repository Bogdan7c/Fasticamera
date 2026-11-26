//! K3 «Шкала с упорами» (`components.md`): одна механика для зума, EV, ISO, выдержки, фокуса, баланса.
//!
//! Шкала не знает, что она «зум»: смысл задаёт модель значений ([`crate::value`]) и привязка на
//! экране. Вида нет: шкала отдаёт состояние ([`ScaleSnapshot`], риски [`ScaleMark`]) и события
//! ([`ScaleEvent`]); рисует `ui-render-vulkan`.
//!
//! * «Ведение за пальцем» — смещение контента равно смещению пальца 1:1 внутри границ, за краем —
//!   «резинка» (движок `ui-core`); начало отсчёта — [`crate::drive::DragOrigin`].
//! * Прилипание — [`SnapPolicy`]: к каждому шагу или плавно с магнитом на опорных значениях.
//! * Хаптика — из таблицы токенов: тик → `SelectionTick`, опорное значение/ноль → `RigidImpact`.

mod detents;
mod element;
mod error;
mod events;
mod spec;
mod zero_magnet;

pub use element::SnapScale;
pub use error::ScaleError;
pub use events::{ScaleEvent, ScaleMark, ScaleSnapshot, SetValueOutcome, TravelDirection};
pub use spec::{ScaleSpec, SnapPolicy, TickVisibility};
pub use zero_magnet::ZeroMagnet;
