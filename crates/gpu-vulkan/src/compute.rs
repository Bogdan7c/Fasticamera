//! Публичный «compute-набор» (сессия 07): безопасные RAII-обёртки, чтобы другой crate (`isp-vulkan`)
//! запускал свои compute-шейдеры, не получая в руки `ash::Device`.
//!
//! # Что внутри
//! * [`ComputePipeline`] — compute-пайплайн из SPIR-V вызывающего (точка входа `main`), раскладка из
//!   одного набора дескрипторов ([`BindingKind`] по номерам привязок), push-константы заданного
//!   размера и specialization-константы ([`SpecializationConstant`]: выключатели стадий ISP, размер
//!   рабочей группы через `local_size_*_id`);
//! * кольцо наборов дескрипторов — по одному набору на слот кадра; переписывается типизированно
//!   ([`BindingWrite`] + [`DescriptorResource`]) с проверкой «вид ресурса подходит к привязке»;
//! * [`ComputePipeline::record_dispatch`] + [`WorkgroupCount::covering`] — запуск, покрывающий W×H.
//!
//! # Инварианты
//! * каждый объект Vulkan — `Owned*` и виден в `live_objects()`; сырых `destroy_*` у вызывающего нет;
//! * все проверки идут ДО записи команд: при ошибке командный буфер остаётся нетронутым;
//! * набор слота переписывается только когда слот свободен от GPU (fence слота сигнален) — это
//!   контракт вызывающего, тот же, что у `RawMosaicPreview` и кольца кадров;
//! * dispatch запрещён, пока хоть одна привязка набора слота ни разу не записана (`InvalidState`):
//!   иначе шейдер читал бы неинициализированный дескриптор (ошибка validation, на Adreno — мусор).
//!
//! Модуль не знает про камеру и ISP: шейдеры, смысл привязок и констант — у вызывающего.

mod binding;
pub(crate) mod descriptors;
mod dispatch;
mod pipeline;

pub use self::binding::{BindingKind, BindingWrite, DescriptorRangeLimits, DescriptorResource};
pub use self::dispatch::{LocalSize, WorkgroupCount, WorkgroupSizeLimits};
pub use self::pipeline::{ComputePipeline, ComputePipelineDesc, SpecializationConstant};
