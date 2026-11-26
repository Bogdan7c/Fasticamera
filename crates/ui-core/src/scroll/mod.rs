//! Прокрутка одной оси: «палец → бросок → прилипание → резинка» (`motion.md` раздел 2).
//!
//! Один движок [`ScrollAxis`] обслуживает карусель режимов и все шкалы. Состав:
//! * [`DecelerationRate`], [`DecelerationModel`] — замедление и проекция броска;
//! * [`RubberBand`] — «резинка» за краем и обратная функция;
//! * [`SnapSelection`] — политика выбора точки прилипания;
//! * [`ScrollConfig`] — параметры (serde + `validate()`).

mod axis;
mod config;
mod deceleration;
mod error;
mod rubber;
mod snap;

pub use axis::{
    AxisGeometry, ReleaseOutcome, ScrollAxis, ScrollMotionState, ScrollPhase, ScrollSample, ScrollTransition,
};
pub use config::ScrollConfig;
pub use deceleration::{DecelerationEnd, DecelerationModel, DecelerationRate, DecelerationSegment};
pub use error::ScrollError;
pub use rubber::RubberBand;
pub use snap::{SnapChoice, SnapSelection};
