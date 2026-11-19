//! `ui-render-vulkan` — GPU-рендер UI и материала Liquid Glass поверх живого фона (решение 86, сессия 11).
//!
//! Пара с `ui-draw` как `isp-vulkan` с `isp-model`: там словарь «что рисовать» и эталонные формулы на CPU,
//! здесь — их исполнение шейдерами. Шейдеры повторяют формулы эталона выражение в выражение (f32); паритет
//! GPU ↔ эталон закреплён тестами с явным допуском. Vulkan — только через растровый набор `gpu-vulkan` (решение 52).
//!
//! # Состав
//! * [`config`] — пределы кадра и пирамида размытия (serde + `validate`);
//! * [`frame`] — вход кадра [`UiFrame`] (список рисования, размер, поворот цели, фон) и отчёт;
//! * [`UiRenderer`] — `record_frame` / `release_slot_after_fence` / `counters`;
//! * [`counters`], [`error`] — диагностика и ошибки с различением причин.
//!
//! # Как устроен кадр
//! Пирамида размытия фона (dual Kawase, фрагментные проходы) строится только при смене ключа фона; затем один
//! проход UI с `Preserve` поверх композитора: фигуры, глифы и иконки (SDF-атлас), стекло — каждый элемент в
//! своих границах, без полноэкранных проходов.

pub mod config;
pub mod counters;
pub mod error;
pub mod frame;
mod pack;
mod pipelines;
mod plan;
mod pyramid;
mod renderer;
mod slots;
#[cfg(feature = "test-support")]
pub mod testing;

pub use config::{
    BackdropPyramidConfig, ConfigError, MAX_PYRAMID_LEVELS, PyramidFormat, UiRenderLimits, UiRendererConfig,
};
pub use counters::UiRenderCounters;
pub use error::{LimitKind, UiRenderError};
pub use frame::{BackdropInput, BackdropPlacement, FrameMarker, PyramidOutcome, UiFrame, UiFrameReport};
pub use renderer::UiRenderer;

/// Вшитые SPIR-V шейдеры (генерирует `build.rs`).
pub(crate) mod shaders {
    // Сборка кладёт сюда ВСЕ шейдеры каталога, включая тестовый `test_backdrop_fill.comp`
    // (синтетический фон, нужен только `testing` под фичей `test-support`).
    #[cfg_attr(
        not(feature = "test-support"),
        expect(
            dead_code,
            reason = "тестовый шейдер синтетического фона используется только с test-support"
        )
    )]
    pub(crate) mod blobs {
        include!(concat!(env!("OUT_DIR"), "/shader_blobs.rs"));
    }
}
