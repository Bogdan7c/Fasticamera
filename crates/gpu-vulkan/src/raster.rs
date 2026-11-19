//! Публичный «растровый набор» (сессия 11): безопасные RAII-обёртки, чтобы другой crate
//! (`ui-render-vulkan`) рисовал UI и материал Liquid Glass, не получая в руки `ash::Device`
//! (решение 52 владельца).
//!
//! # Что внутри
//! * [`RasterPipeline`] — графический конвейер из SPIR-V вызывающего (вершинный + фрагментный, точка
//!   входа `main`): без вершинных буферов (вершины генерирует шейдер из `gl_VertexIndex` /
//!   `gl_InstanceIndex`, данные экземпляров лежат в storage-буфере), `TRIANGLE_LIST`, без отсечения,
//!   динамические viewport и scissor, динамический рендеринг в один цвет без глубины. Раскладка набора
//!   дескрипторов и кольцо наборов — те же, что у compute ([`crate::compute::BindingKind`],
//!   [`crate::compute::BindingWrite`]), но видимость привязок — `VERTEX | FRAGMENT`;
//! * [`BlendMode`] — `Opaque` или `PremultipliedAlpha` (`src = ONE`, `dst = ONE_MINUS_SRC_ALPHA` и для
//!   цвета, и для альфы);
//! * [`RasterPass`] — один проход динамического рендеринга в [`ColorAttachment`] (кадр swapchain или
//!   [`crate::color_target::ColorTarget`]): `begin` → `draw`… → `end`;
//! * [`record_between_color_passes`] — барьер между двумя проходами в одну цель (композитор → UI).
//!
//! # Инварианты
//! * все проверки — ДО записи команд: формат конвейера равен формату цели, длина push-констант равна
//!   объявленной, все привязки слота записаны, слот в кольце, scissor внутри цели; при ошибке
//!   командный буфер остаётся нетронутым, а проход — открытым и пригодным для следующего `draw`;
//! * проход, брошенный без `end()`, закрывает рендеринг в `Drop` (и пишет предупреждение): иначе
//!   командный буфер остался бы внутри рендеринга и невалидным;
//! * набор слота переписывается только когда слот свободен от GPU — контракт, общий с compute
//!   (`bind` / `release_slot_after_fence`);
//! * барьеры ресурсов (цель, текстуры, буфер экземпляров) записывают их владельцы вне прохода.
//!
//! Модуль не знает про UI и стекло: шейдеры, смысл привязок и констант — у вызывающего.

mod attachment;
mod create;
mod pass;
mod pipeline;
mod sync;

pub use self::attachment::{AttachmentLoad, ColorAttachment};
pub use self::pass::{DrawCall, RasterPass};
pub use self::pipeline::{BlendMode, RasterPipeline, RasterPipelineDesc};
pub use self::sync::record_between_color_passes;
