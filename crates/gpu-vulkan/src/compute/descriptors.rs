//! Кольцо наборов дескрипторов: одна раскладка, по набору на слот кадра, учёт записанных привязок.
//!
//! Общий для [`super::ComputePipeline`] и компоновщика картинок (`crate::compositor`): отличаются
//! только стадия шейдера и список привязок.
//!
//! Инварианты:
//! * набор слота переписывается только когда слот свободен от GPU (контракт вызывающего);
//! * все записи проверяются ДО `vkUpdateDescriptorSets`: при ошибке набор не меняется;
//! * набор выдаётся для записи команд, только если КАЖДАЯ его привязка хоть раз записана;
//! * набор, привязанный в командный буфер ([`DescriptorRing::bind_for_recording`]), нельзя ИЗМЕНИТЬ,
//!   пока владелец не подтвердит, что fence слота сигнален ([`DescriptorRing::release_after_fence`]).
//!
//! # Почему последний пункт — ошибка, а не «контракт на словах»
//! `vkCmdBindDescriptorSets` разрешает драйверу прочитать содержимое набора в любой момент между
//! записью команды и концом её исполнения. Обновление набора, уже привязанного в командном буфере
//! (в состоянии записи или ожидания), без `UPDATE_AFTER_BIND` делает этот буфер недействительным
//! (`VUID-vkUpdateDescriptorSets-None-03047`; спецификация `vkCmdBindDescriptorSets`: «contents must not
//! be altered … until the command completes»). Типичная ошибка — дважды записать один слот в один
//! командный буфер. Поэтому повторная запись ИНЫХ ресурсов в «занятый» слот — `InvalidState`, а запись
//! тех же самых ресурсов — честная пустая операция (обновления нет, нарушать нечего).

use std::sync::atomic::{AtomicBool, Ordering};

use ash::vk;

use super::binding::{BindingKind, BindingWrite, DescriptorResource, validate_write};
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::linear_sampler::create_linear_clamp_sampler;
use crate::live_objects::{OwnedDescriptorPool, OwnedDescriptorSetLayout, OwnedSampler};

/// Больше привязок в одном наборе не бывает: маска «записано» — `u64`.
const MAX_BINDINGS: usize = 64;

/// Кольцо наборов. Порядок полей = порядок уничтожения: пул (с наборами) → раскладка → сэмплер
/// (неизменяемый сэмплер раскладки уничтожается последним).
pub(crate) struct DescriptorRing {
    _pool: OwnedDescriptorPool,
    set_layout: OwnedDescriptorSetLayout,
    _sampler: Option<OwnedSampler>,
    sets: Vec<vk::DescriptorSet>,
    /// По слоту: битовая маска привязок, которые хоть раз записаны.
    written: Vec<u64>,
    /// По слоту и привязке: что сейчас записано в набор (для «та же запись — не обновление»).
    current: Vec<Vec<Option<DescriptorResource>>>,
    /// По слоту: набор привязан в командный буфер, а fence слота ещё не подтверждён владельцем.
    /// Атомарные флаги — потому что привязка пишется из `&self` (`ComputePipeline::record_dispatch`).
    bound_since_release: Vec<AtomicBool>,
    /// По слоту: владелец подтвердил fence (`release_after_fence`), и запомненное в `current` больше нельзя
    /// считать записанным: ресурсы за это время могли быть уничтожены, а их хэндлы Vulkan — выданы заново
    /// (драйвер переиспользует значения хэндлов). Сбрасывается в `write`, которому нужен `&mut self`.
    cache_stale: Vec<AtomicBool>,
    kinds: Vec<BindingKind>,
    context: GpuContext,
}

impl DescriptorRing {
    /// Создаёт раскладку, пул и `slot_count` наборов. `label` попадает в имена шагов ошибок.
    pub(crate) fn new(
        context: &GpuContext,
        kinds: &[BindingKind],
        stage: vk::ShaderStageFlags,
        slot_count: usize,
        label: &'static str,
    ) -> Result<Self, GpuError> {
        let invalid = |what: String| GpuError::InvalidArgument { what };
        if kinds.is_empty() || kinds.len() > MAX_BINDINGS {
            return Err(invalid(format!(
                "{label}: привязок {}, допустимо 1..={MAX_BINDINGS}",
                kinds.len()
            )));
        }
        let slots = u32::try_from(slot_count)
            .ok()
            .filter(|count| *count > 0)
            .ok_or_else(|| invalid(format!("{label}: число слотов {slot_count} вне 1..=u32::MAX")))?;

        let needs_sampler = kinds.contains(&BindingKind::CombinedImageSampler);
        let sampler = needs_sampler
            .then(|| create_linear_clamp_sampler(context, "vkCreateSampler(набор дескрипторов)"))
            .transpose()?;
        let immutable_samplers = [sampler.as_ref().map_or(vk::Sampler::null(), OwnedSampler::handle)];

        let bindings: Vec<vk::DescriptorSetLayoutBinding<'_>> = (0_u32..)
            .zip(kinds)
            .map(|(binding, kind)| {
                let layout_binding = vk::DescriptorSetLayoutBinding::default()
                    .binding(binding)
                    .descriptor_type(kind.descriptor_type())
                    .descriptor_count(1)
                    .stage_flags(stage);
                if *kind == BindingKind::CombinedImageSampler {
                    layout_binding.immutable_samplers(&immutable_samplers)
                } else {
                    layout_binding
                }
            })
            .collect();
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: устройство валидно; bindings и immutable_samplers живут до конца вызова.
        let set_layout = OwnedDescriptorSetLayout::from_raw(
            context,
            run_step(
                unsafe { context.device().create_descriptor_set_layout(&layout_info, None) },
                label,
            )?,
        );

        let pool_sizes = pool_sizes(kinds, slots);
        let pool_info = vk::DescriptorPoolCreateInfo::default()
            .max_sets(slots)
            .pool_sizes(&pool_sizes);
        // SAFETY: устройство валидно; структуры локальные.
        let pool = OwnedDescriptorPool::from_raw(
            context,
            run_step(
                unsafe { context.device().create_descriptor_pool(&pool_info, None) },
                label,
            )?,
        );
        let layouts = vec![set_layout.handle(); slot_count];
        let allocate_info = vk::DescriptorSetAllocateInfo::default()
            .descriptor_pool(pool.handle())
            .set_layouts(&layouts);
        // SAFETY: пул и раскладка принадлежат этому устройству; наборы освобождаются вместе с пулом.
        let sets = run_step(
            unsafe { context.device().allocate_descriptor_sets(&allocate_info) },
            label,
        )?;
        Ok(Self {
            _pool: pool,
            set_layout,
            _sampler: sampler,
            sets,
            written: vec![0; slot_count],
            current: vec![vec![None; kinds.len()]; slot_count],
            bound_since_release: (0..slot_count).map(|_| AtomicBool::new(false)).collect(),
            cache_stale: (0..slot_count).map(|_| AtomicBool::new(false)).collect(),
            kinds: kinds.to_vec(),
            context: context.clone(),
        })
    }

    /// Раскладка набора (для `VkPipelineLayout`).
    pub(crate) fn set_layout(&self) -> vk::DescriptorSetLayout {
        self.set_layout.handle()
    }

    /// Число слотов.
    pub(crate) fn slot_count(&self) -> usize {
        self.sets.len()
    }

    /// Виды привязок по номерам.
    pub(crate) fn kinds(&self) -> &[BindingKind] {
        &self.kinds
    }

    /// Переписывает привязки набора слота. Всё проверяется до вызова Vulkan.
    ///
    /// Контракт: слот свободен от GPU (fence последнего сабмита, читавшего набор слота, сигнален).
    /// Если набор привязан в командный буфер после последнего [`Self::release_after_fence`], запись
    /// других ресурсов — `InvalidState` (см. модуль), запись тех же — пустая операция.
    pub(crate) fn write(&mut self, slot: usize, writes: &[BindingWrite]) -> Result<(), GpuError> {
        let set = self.set_at(slot)?;
        let limits = self.context.descriptor_range_limits();
        for write in writes {
            validate_write(&self.kinds, write, limits)?;
        }
        // После `release_after_fence` «что записано» неизвестно: хэндл нового ресурса мог совпасть со значением
        // хэндла уничтоженного (RADV: хэндл — адрес объекта драйвера), а набор хранит АДРЕС старой памяти.
        // Пропустить запись «того же» хэндла значило бы читать освобождённую память (page fault, DEVICE_LOST).
        if self.cache_stale[slot].swap(false, Ordering::AcqRel) {
            self.current[slot].fill(None);
        }
        let unchanged = writes
            .iter()
            .all(|write| self.current[slot][write.binding as usize] == Some(write.resource));
        if unchanged {
            return Ok(());
        }
        if self.bound_since_release[slot].load(Ordering::Acquire) {
            return Err(GpuError::InvalidState {
                what: format!(
                    "набор слота {slot} уже привязан в командный буфер и ещё не освобождён fence-ом: \
                     менять его нельзя (повторная запись слота в один кадр?)"
                ),
            });
        }
        // Описания ресурсов живут в векторах до конца вызова `update_descriptor_sets`.
        let image_infos: Vec<[vk::DescriptorImageInfo; 1]> = writes
            .iter()
            .map(|write| {
                let (view, layout) = write
                    .resource
                    .image_view_and_layout()
                    .unwrap_or((vk::ImageView::null(), vk::ImageLayout::UNDEFINED));
                [vk::DescriptorImageInfo::default().image_view(view).image_layout(layout)]
            })
            .collect();
        let buffer_infos: Vec<[vk::DescriptorBufferInfo; 1]> = writes
            .iter()
            .map(|write| {
                let (buffer, range) = write.resource.buffer_range().unwrap_or((vk::Buffer::null(), 0));
                [vk::DescriptorBufferInfo::default()
                    .buffer(buffer)
                    .offset(0)
                    .range(range)]
            })
            .collect();
        let vk_writes: Vec<vk::WriteDescriptorSet<'_>> = writes
            .iter()
            .enumerate()
            .map(|(index, write)| {
                let kind = self.kinds[write.binding as usize];
                let base = vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(write.binding)
                    .descriptor_type(kind.descriptor_type());
                if write.resource.buffer_range().is_some() {
                    base.buffer_info(&buffer_infos[index])
                } else {
                    base.image_info(&image_infos[index])
                }
            })
            .collect();
        // SAFETY: набор принадлежит слоту, свободному от GPU (контракт метода); все структуры живы до
        // конца вызова; виды ресурсов проверены против раскладки выше.
        unsafe { self.context.device().update_descriptor_sets(&vk_writes, &[]) };
        let slot_mask = &mut self.written[slot];
        for write in writes {
            *slot_mask |= 1_u64 << write.binding;
            self.current[slot][write.binding as usize] = Some(write.resource);
        }
        Ok(())
    }

    /// Набор слота для `vkCmdBindDescriptorSets`: как [`Self::ready_set`], и слот помечается «привязан»
    /// до [`Self::release_after_fence`].
    pub(crate) fn bind_for_recording(&self, slot: usize) -> Result<vk::DescriptorSet, GpuError> {
        let set = self.ready_set(slot)?;
        self.bound_since_release[slot].store(true, Ordering::Release);
        Ok(set)
    }

    /// Владелец подтверждает: fence последнего сабмита, привязавшего набор слота, сигнален — набор
    /// снова можно менять. Слот вне кольца — `InvalidArgument`.
    pub(crate) fn release_after_fence(&self, slot: usize) -> Result<(), GpuError> {
        self.set_at(slot)?;
        self.bound_since_release[slot].store(false, Ordering::Release);
        self.cache_stale[slot].store(true, Ordering::Release);
        Ok(())
    }

    /// Набор слота для записи команд; ошибка, если какая-то привязка ни разу не записана.
    pub(crate) fn ready_set(&self, slot: usize) -> Result<vk::DescriptorSet, GpuError> {
        let set = self.set_at(slot)?;
        let all_bindings = if self.kinds.len() == MAX_BINDINGS {
            u64::MAX
        } else {
            (1_u64 << self.kinds.len()) - 1
        };
        let missing = all_bindings & !self.written[slot];
        if missing != 0 {
            let missing_bindings: Vec<u32> = (0..self.kinds.len() as u32)
                .filter(|binding| missing & (1_u64 << binding) != 0)
                .collect();
            return Err(GpuError::InvalidState {
                what: format!("в наборе слота {slot} не записаны привязки {missing_bindings:?}"),
            });
        }
        Ok(set)
    }

    fn set_at(&self, slot: usize) -> Result<vk::DescriptorSet, GpuError> {
        self.sets.get(slot).copied().ok_or_else(|| GpuError::InvalidArgument {
            what: format!("слот набора {slot} вне 0..{}", self.sets.len()),
        })
    }
}

/// Размеры пула: по каждому типу дескриптора — число привязок этого типа × число слотов.
fn pool_sizes(kinds: &[BindingKind], slots: u32) -> Vec<vk::DescriptorPoolSize> {
    let mut sizes: Vec<vk::DescriptorPoolSize> = Vec::new();
    for kind in kinds {
        let descriptor_type = kind.descriptor_type();
        match sizes.iter_mut().find(|size| size.ty == descriptor_type) {
            Some(size) => size.descriptor_count += slots,
            None => sizes.push(
                vk::DescriptorPoolSize::default()
                    .ty(descriptor_type)
                    .descriptor_count(slots),
            ),
        }
    }
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_support::gpu_context_or_skip;
    use crate::live_objects::LiveObjectKind;

    #[test]
    fn pool_sizes_merge_same_descriptor_types() {
        let kinds = [
            BindingKind::ReadOnlyStorageBuffer,
            BindingKind::StorageImageWrite,
            BindingKind::ReadWriteStorageBuffer,
        ];
        let sizes = pool_sizes(&kinds, 3);
        assert_eq!(sizes.len(), 2);
        assert_eq!(sizes[0].ty, vk::DescriptorType::STORAGE_BUFFER);
        assert_eq!(sizes[0].descriptor_count, 6);
        assert_eq!(sizes[1].ty, vk::DescriptorType::STORAGE_IMAGE);
        assert_eq!(sizes[1].descriptor_count, 3);
    }

    #[test]
    fn empty_layout_and_zero_slots_are_rejected_without_creating_objects() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        let stage = vk::ShaderStageFlags::COMPUTE;
        assert!(DescriptorRing::new(&context, &[], stage, 2, "тест").is_err());
        assert!(DescriptorRing::new(&context, &[BindingKind::UniformBuffer], stage, 0, "тест").is_err());
        assert_eq!(context.live_objects(), baseline);
    }

    #[test]
    fn sampler_exists_only_for_combined_image_sampler_layouts_and_is_freed() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        {
            let ring = DescriptorRing::new(
                &context,
                &[BindingKind::CombinedImageSampler],
                vk::ShaderStageFlags::FRAGMENT,
                2,
                "тест",
            )
            .unwrap();
            assert_eq!(ring.slot_count(), 2);
            assert_eq!(context.live_objects().count(LiveObjectKind::DescriptorPool), 1);
            assert_eq!(context.live_objects().count(LiveObjectKind::Sampler), 1);
            let plain = DescriptorRing::new(
                &context,
                &[BindingKind::UniformBuffer],
                vk::ShaderStageFlags::COMPUTE,
                2,
                "тест",
            )
            .unwrap();
            assert_eq!(context.live_objects().count(LiveObjectKind::Sampler), 1);
            assert_eq!(plain.slot_count(), 2);
        }
        assert_eq!(context.live_objects(), baseline);
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }

    #[test]
    fn release_after_fence_forgets_cached_writes_so_a_recreated_resource_with_the_same_handle_is_rewritten() {
        let Some(context) = gpu_context_or_skip() else { return };
        let mut ring = DescriptorRing::new(
            &context,
            &[BindingKind::ReadOnlyStorageBuffer],
            vk::ShaderStageFlags::COMPUTE,
            1,
            "тест",
        )
        .unwrap();
        let buffer = crate::device_buffer::DeviceBuffer::new(&context, 64).unwrap();
        let write = [BindingWrite {
            binding: 0,
            resource: buffer.read_only_binding(),
        }];
        ring.write(0, &write).unwrap();
        assert_eq!(ring.current[0][0], Some(buffer.read_only_binding()), "запись запомнена");
        // Тот же ресурс без освобождения — пустая операция (кэш в силе).
        ring.write(0, &write).unwrap();
        assert!(!ring.cache_stale[0].load(Ordering::Acquire));
        // Владелец подтвердил fence: «что записано» больше неизвестно — ресурс мог быть уничтожен, а его
        // хэндл выдан новому буферу с другой памятью (так падал снимок: page fault на RADV).
        ring.release_after_fence(0).unwrap();
        assert!(ring.cache_stale[0].load(Ordering::Acquire));
        // Следующая запись обязана дойти до Vulkan даже для «того же» хэндла: после неё кэш снова в силе.
        ring.write(0, &write).unwrap();
        assert!(!ring.cache_stale[0].load(Ordering::Acquire), "кэш сброшен записью");
        assert_eq!(ring.current[0][0], Some(buffer.read_only_binding()));
    }
}
