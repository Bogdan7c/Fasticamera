//! Материал Liquid Glass: параметры [`GlassStyle`] и эталонные формулы [`reference`](mod@reference).
//!
//! * `style` — настраиваемые числа в pt (serde, `validate()`), по `docs/ios-camera/glass.md`, раздел 6;
//! * `params` — те же числа, пересчитанные в пиксели под конкретную форму (`GlassStyle::resolve`);
//! * [`reference`](mod@reference) — формулы «в форме шейдера» на CPU; `ui-render-vulkan` сверяется с ними тестами паритета.
//!
//! Все числа — подгонка по открытым реимплементациям (`[reverse]`) и оценкам (`[estimate]`),
//! не данные Apple (`glass.md`, раздел 0).

mod params;
pub mod reference;
mod style;

pub use params::{DarkEdgeParamsPx, DimmingParams, GlassParamsPx, PressParamsPx, ShadowParamsPx, SpecularParamsPx};
pub use style::{
    DarkEdgeStyle, DimmingStyle, EdgeProfile, Extent, GlassStyle, GlassStyleError, PressStyle, ShadowStyle,
    SpecularStyle,
};
