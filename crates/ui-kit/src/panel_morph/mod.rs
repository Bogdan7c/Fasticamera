//! K7 «Панель-морф» (`components.md`): меню Controls, подменю пункта, панель Exposure.
//!
//! Панель вырастает из рамки-источника (кнопки) в рамку-цель и сворачивается обратно. Вида здесь
//! нет: элемент отдаёт [`PanelSnapshot`] (рамка, радиус, материализация стекла, прозрачность и
//! сдвиг содержимого), рисует [`crate::view::panel_glass_group`].
//!
//! * Пружина по умолчанию — `camera.sheet` (`motion.md` 4.1, и открытие, и закрытие); морф пункта
//!   в подменю — `smooth` (`motion.md` 4.2): задаётся в [`PanelMorphSpec::spring`].
//! * Режим «на месте» ([`PanelMorphSpec::in_place`]): рамка и радиус постоянны, меняются только
//!   стекло и содержимое (шторка и карточка iOS 27).
//! * Прерывание (закрыли во время открытия) наследует положение и скорость: скачка нет.
//! * События [`PanelEvent`] приходят по одному на каждое **завершённое** движение: прерванное
//!   открытие события `Opened` не даёт.

mod element;
mod error;
mod spec;

pub use element::{PanelEvent, PanelMorph, PanelPhase, PanelSnapshot, TransitionOutcome};
pub use error::PanelError;
pub use spec::{ContentSlide, PanelMorphSpec, PanelMotionKind, concentric_radius_pt};
