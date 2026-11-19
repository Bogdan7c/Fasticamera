//! K4 «Карусель с прилипанием» (`components.md`): переключатель режимов, стили, выбор фокусного.
//!
//! Элементы — список данных ([`CarouselEntry`]); карусель строится из **списка реализованных**
//! режимов (М1, решение 80): добавление режима — это новый элемент списка и
//! [`SnapCarousel::set_entries`], а не правка карусели. Механика — тот же движок прокрутки, что у
//! шкалы, политика `AdjacentOnly`: свайп листает на ±1 элемент, как бы сильно ни бросили.

mod element;
mod entry;
mod error;
mod events;

pub use element::SnapCarousel;
pub use entry::{CarouselEntry, CarouselSpec, ItemPlacement};
pub use error::CarouselError;
pub use events::{CarouselEvent, SelectionCause, TapOutcome};
