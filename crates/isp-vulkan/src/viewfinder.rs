//! Видоискатель: ОДИН слитый compute-проход RAW → [`ViewfinderImage`] (сессия 07, пакет P5).
//!
//! # Конвейер (паритет с `isp_model::pipeline::run_reference`, демозаик Preview)
//! уровни → коррекция виньетирования картой усилений (`shaders/lens_shading.glsl`; «пропуск» в параметрах
//! кадра вырезает стадию из прохода, карта больше буфера слота — `ViewfinderError::LensShadingMap`) →
//! баланс белого + обрезка → демозаик 2×2 → камера→XYZ D50 → XYZ→экран → сжатие охвата →
//! тон с сохранением оттенка → OETF sRGB; плюс статистика AWB по плиткам в том же проходе (после виньетирования, до баланса белого)
//! (группа в одной плитке — `subgroupAdd`, на стыке плиток — shared-окно; затем `atomicAdd` в буфер
//! слота, раскладка — `isp_model::AwbTile`; способ — [`AwbReduction`]).
//! Шейдер — `shaders/viewfinder_isp.glsl`; f32 везде (решение по fp16 — в отчёте сессии 07).
//!
//! # Владение
//! * [`ViewfinderIsp`] владеет изображением выхода, вариантами пайплайна, uniform-буферами,
//!   буферами статистики и метками времени (всё по слотам кадров);
//! * кадр камеры ([`gpu_vulkan::raw_mosaic_preview::MosaicSource`]) ISP только читает: приём у камеры
//!   (`record_acquire_for(.., ConsumerStage::Compute)`) и возврат — работа вызывающего (render-loop);
//! * выход в прямой ориентации с закодированными значениями: компоновщику остаётся только положить
//!   его на экран с учётом поворота дисплея; стекло читает его как [`gpu_vulkan::backdrop::BackdropSource`].
//!
//! # Цена стадий
//! Каждая стадия выключается specialization-константой ([`IspStageSet`]); A/B-замер «с/без» даёт её
//! цену в мс GPU (метки [`ViewfinderSlotReport::isp_gpu_time`]).

mod awb_group_layout;
mod awb_reduction;
mod config;
mod error;
mod geometry;
mod image;
mod isp;
pub(crate) mod retirement;
mod stages;
mod statistics;
mod uniforms;
mod variant_family;
mod variant_stats;
mod variants;

pub use self::awb_group_layout::{AwbGroupLayout, DispatchShape};
pub use self::awb_reduction::{AwbReduction, AwbReductionPolicy, SubgroupArithmeticSupport};
pub use self::config::{
    DEFAULT_PIPELINE_VARIANT_CAPACITY, LargestMosaicConfig, MAX_FRAME_SLOTS, MAX_WORKGROUP_INVOCATIONS, VariantPrewarm,
    ViewfinderIspConfig, ViewfinderStorageFormat, WorkgroupSizeConfig,
};
pub use self::error::ViewfinderError;
pub use self::geometry::{OutputAllocation, ViewfinderGeometry};
pub use self::image::ViewfinderImage;
pub use self::isp::{RecordedViewfinderFrame, ViewfinderFrame, ViewfinderIsp, ViewfinderSlotReport};
pub use self::stages::{IspStage, IspStageSet};
pub use self::uniforms::{VIEWFINDER_UNIFORM_BYTES, gamut_for_display};
pub use self::variant_stats::VariantCacheStats;
