//! [`ComputePipeline`]: compute-пайплайн из чужого SPIR-V с кольцом наборов дескрипторов.

use ash::vk;

use super::binding::{BindingKind, BindingWrite};
use super::descriptors::DescriptorRing;
use super::dispatch::WorkgroupCount;
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedPipeline, OwnedPipelineLayout};

/// Одна specialization-константа: `layout(constant_id = N) const uint ...` в шейдере.
///
/// Только `u32`: выключатели стадий (0/1), размер рабочей группы, коды режимов. Булевы константы
/// GLSL (`const bool`) тоже передаются 32-битным значением (0 = false), так что тип покрывает и их.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SpecializationConstant {
    /// `constant_id` в шейдере.
    pub constant_id: u32,
    /// Значение.
    pub value: u32,
}

/// Описание compute-пайплайна.
#[derive(Debug, Clone, Copy)]
pub struct ComputePipelineDesc<'a> {
    /// Короткое имя для ошибок и логов (например, `"isp-viewfinder"`).
    pub label: &'static str,
    /// SPIR-V (например, `include_bytes!` из другого crate). Точка входа — `main`.
    pub spirv: &'a [u8],
    /// Привязки единственного набора (`set = 0`): номер привязки = позиция в срезе.
    pub bindings: &'a [BindingKind],
    /// Размер push-констант, байт (кратен 4; 0 — без push-констант).
    pub push_constant_bytes: u32,
    /// Specialization-константы (идентификаторы не повторяются).
    pub specialization: &'a [SpecializationConstant],
    /// Сколько наборов дескрипторов держать: обычно = числу кадров в полёте.
    pub frame_slots: usize,
}

/// Compute-пайплайн + раскладка + кольцо наборов дескрипторов.
///
/// Порядок полей = порядок уничтожения: пайплайн → его layout → наборы (раскладка набора нужна
/// layout-у пайплайна только при создании).
pub struct ComputePipeline {
    pipeline: OwnedPipeline,
    pipeline_layout: OwnedPipelineLayout,
    descriptors: DescriptorRing,
    push_constant_bytes: u32,
    label: &'static str,
    context: GpuContext,
}

impl std::fmt::Debug for ComputePipeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "ComputePipeline({}, {:?})", self.label, self.pipeline)
    }
}

impl ComputePipeline {
    /// Создаёт пайплайн. Все проверки описания — до создания объектов Vulkan.
    pub fn new(context: &GpuContext, desc: &ComputePipelineDesc<'_>) -> Result<Self, GpuError> {
        validate_desc(context, desc)?;
        let descriptors = DescriptorRing::new(
            context,
            desc.bindings,
            vk::ShaderStageFlags::COMPUTE,
            desc.frame_slots,
            "vkCreateDescriptorSetLayout/Pool(compute)",
        )?;
        let pipeline_layout = create_pipeline_layout(context, &descriptors, desc.push_constant_bytes)?;
        let pipeline = create_pipeline(context, desc, &pipeline_layout)?;
        Ok(Self {
            pipeline,
            pipeline_layout,
            descriptors,
            push_constant_bytes: desc.push_constant_bytes,
            label: desc.label,
            context: context.clone(),
        })
    }

    /// Число слотов наборов дескрипторов.
    pub fn frame_slots(&self) -> usize {
        self.descriptors.slot_count()
    }

    /// Виды привязок по номерам (как в описании).
    pub fn bindings(&self) -> &[BindingKind] {
        self.descriptors.kinds()
    }

    /// Переписывает привязки набора слота `frame_slot`. Проверки (номер, вид ресурса, диапазон и его
    /// лимит устройства) — до вызова Vulkan: при ошибке набор не меняется. Другие слоты не затрагиваются.
    ///
    /// Контракт: GPU не читает набор этого слота (fence последнего сабмита слота сигнален).
    /// Если слот уже привязан dispatch-ем ([`Self::record_dispatch`]) и владелец ещё не подтвердил его
    /// fence ([`Self::release_slot_after_fence`]), запись ДРУГИХ ресурсов — `InvalidState` (обновление
    /// набора, привязанного в командном буфере, делает буфер недействительным); тех же — пустая операция.
    pub fn bind(&mut self, frame_slot: usize, writes: &[BindingWrite]) -> Result<(), GpuError> {
        self.descriptors.write(frame_slot, writes)
    }

    /// Владелец подтверждает: fence последнего сабмита, в который записан dispatch слота `frame_slot`,
    /// сигнален — набор слота снова можно переписывать ([`Self::bind`]).
    pub fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), GpuError> {
        self.descriptors.release_after_fence(frame_slot)
    }

    /// Проверяет dispatch БЕЗ записи команд — те же проверки, что у [`Self::record_dispatch`]: слот в
    /// кольце, все привязки набора записаны, размер push-констант `push_constant_bytes`, число групп в
    /// лимите устройства. Звать до первой команды кадра, чтобы ошибка не оставила полузаписанный буфер.
    pub fn check_dispatch(
        &self,
        frame_slot: usize,
        push_constant_bytes: usize,
        groups: WorkgroupCount,
    ) -> Result<(), GpuError> {
        self.descriptors.ready_set(frame_slot)?;
        self.check_push_and_groups(push_constant_bytes, groups)
    }

    /// Как [`Self::check_dispatch`], но без проверки набора дескрипторов: для прохода, чьи привязки
    /// станут известны только позже (цель приёмника кадра выдаётся первой командой кадра). Проверяет
    /// размер push-констант и лимит групп устройства.
    pub fn check_dispatch_shape(&self, push_constant_bytes: usize, groups: WorkgroupCount) -> Result<(), GpuError> {
        self.check_push_and_groups(push_constant_bytes, groups)
    }

    /// Записывает в `command_buffer`: привязку пайплайна и набора слота, push-константы и dispatch.
    ///
    /// Ошибки до записи команд — как у [`Self::check_dispatch`]. Барьеры для ресурсов (картинки,
    /// буферы, кадр камеры) записывает вызывающий через intent-методы их владельцев.
    pub fn record_dispatch(
        &self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        push_constants: &[u8],
        groups: WorkgroupCount,
    ) -> Result<(), GpuError> {
        self.descriptors.ready_set(frame_slot)?;
        self.check_push_and_groups(push_constants.len(), groups)?;
        // Последняя проверка пройдена: дальше только команды, слот помечается «привязан».
        let set = self.descriptors.bind_for_recording(frame_slot)?;
        let device = self.context.device();
        // SAFETY: командный буфер в состоянии записи (контракт вызывающего); пайплайн, layout и набор
        // принадлежат self; размер push-констант совпадает с объявленным диапазоном COMPUTE.
        unsafe {
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::COMPUTE, self.pipeline.handle());
            device.cmd_bind_descriptor_sets(
                command_buffer,
                vk::PipelineBindPoint::COMPUTE,
                self.pipeline_layout.handle(),
                0,
                &[set],
                &[],
            );
            if !push_constants.is_empty() {
                device.cmd_push_constants(
                    command_buffer,
                    self.pipeline_layout.handle(),
                    vk::ShaderStageFlags::COMPUTE,
                    0,
                    push_constants,
                );
            }
            device.cmd_dispatch(command_buffer, groups.x, groups.y, groups.z);
        }
        Ok(())
    }

    /// Размер push-констант равен объявленному, число групп — в лимите устройства.
    fn check_push_and_groups(&self, push_constant_bytes: usize, groups: WorkgroupCount) -> Result<(), GpuError> {
        if push_constant_bytes != self.push_constant_bytes as usize {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "{}: push-константы {push_constant_bytes} байт, объявлено {}",
                    self.label, self.push_constant_bytes
                ),
            });
        }
        groups.validate_against(self.context.max_compute_work_group_count())
    }
}

/// Проверяет описание: SPIR-V не пустой, push-константы кратны 4 и в лимите, id констант уникальны.
fn validate_desc(context: &GpuContext, desc: &ComputePipelineDesc<'_>) -> Result<(), GpuError> {
    let invalid = |what: String| GpuError::InvalidArgument { what };
    if desc.spirv.is_empty() {
        return Err(invalid(format!("{}: пустой SPIR-V", desc.label)));
    }
    let push_limit = context.max_push_constants_size();
    if !desc.push_constant_bytes.is_multiple_of(4) || desc.push_constant_bytes > push_limit {
        return Err(invalid(format!(
            "{}: push-константы {} байт (нужно кратно 4 и ≤ {push_limit})",
            desc.label, desc.push_constant_bytes
        )));
    }
    for (index, constant) in desc.specialization.iter().enumerate() {
        if desc.specialization[..index]
            .iter()
            .any(|earlier| earlier.constant_id == constant.constant_id)
        {
            return Err(invalid(format!(
                "{}: specialization-константа {} задана дважды",
                desc.label, constant.constant_id
            )));
        }
    }
    Ok(())
}

fn create_pipeline_layout(
    context: &GpuContext,
    descriptors: &DescriptorRing,
    push_constant_bytes: u32,
) -> Result<OwnedPipelineLayout, GpuError> {
    let set_layouts = [descriptors.set_layout()];
    let push_ranges = [vk::PushConstantRange::default()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(push_constant_bytes)];
    let used_ranges: &[vk::PushConstantRange] = if push_constant_bytes == 0 { &[] } else { &push_ranges };
    let layout_info = vk::PipelineLayoutCreateInfo::default()
        .set_layouts(&set_layouts)
        .push_constant_ranges(used_ranges);
    // SAFETY: устройство валидно; массивы живут до конца вызова.
    let layout = run_step(
        unsafe { context.device().create_pipeline_layout(&layout_info, None) },
        "vkCreatePipelineLayout(compute)",
    )?;
    Ok(OwnedPipelineLayout::from_raw(context, layout))
}

fn create_pipeline(
    context: &GpuContext,
    desc: &ComputePipelineDesc<'_>,
    layout: &OwnedPipelineLayout,
) -> Result<OwnedPipeline, GpuError> {
    let module = context.create_shader_module(desc.spirv)?;
    let constant_bytes: Vec<u8> = desc
        .specialization
        .iter()
        .flat_map(|constant| constant.value.to_ne_bytes())
        .collect();
    let map_entries: Vec<vk::SpecializationMapEntry> = (0_u32..)
        .zip(desc.specialization)
        .map(|(index, constant)| {
            vk::SpecializationMapEntry::default()
                .constant_id(constant.constant_id)
                .offset(index * 4)
                .size(std::mem::size_of::<u32>())
        })
        .collect();
    let specialization_info = vk::SpecializationInfo::default()
        .map_entries(&map_entries)
        .data(&constant_bytes);
    let stage = vk::PipelineShaderStageCreateInfo::default()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module.handle())
        .name(c"main")
        .specialization_info(&specialization_info);
    let pipeline_info = vk::ComputePipelineCreateInfo::default()
        .stage(stage)
        .layout(layout.handle());
    // SAFETY: модуль, layout и данные констант живут до конца вызова; кэша пайплайнов нет.
    let pipelines = unsafe {
        context
            .device()
            .create_compute_pipelines(vk::PipelineCache::null(), &[pipeline_info], None)
    }
    .map_err(|(_, result)| GpuError::Vulkan {
        step: "vkCreateComputePipelines",
        result,
    })?;
    let handle = pipelines.first().copied().ok_or(GpuError::InvalidState {
        what: "vkCreateComputePipelines не вернул пайплайн".to_owned(),
    })?;
    Ok(OwnedPipeline::from_raw(context, handle))
}
