//! Фигуры списка рисования и их знаковое расстояние (SDF) — эталон формул шейдера.
//!
//! Всё в `f32`, без итераций: те же выражения потом записываются в GLSL, а GPU сверяется с этим
//! модулем тестами паритета.
//!
//! * [`ShapeGeometry`] — скруглённый прямоугольник, капсула, круг; [`CornerCurve`] — дуга окружности
//!   или «непрерывное» скругление iOS (суперэллипс);
//! * [`signed_distance`], [`distance_gradient`], [`corrected_distance`] — расстояние, его градиент и
//!   приближение к евклидову расстоянию первого порядка;
//! * [`coverage`], [`smooth_union`] — сглаживание края и слияние форм «как капли».

mod blend;
mod sdf;
mod shadow;
mod types;

pub use blend::{SmoothUnion, coverage, over, smooth_union, smooth_union_with_weights};
pub use sdf::{CorrectedDistance, corrected_distance, distance_gradient, signed_distance};
pub use shadow::{shadow_falloff, shadow_probe_point};
pub use types::{CornerCurve, CornerSpec, DEFAULT_CONTINUOUS_EXPONENT, SdfParameters, ShapeError, ShapeGeometry};
