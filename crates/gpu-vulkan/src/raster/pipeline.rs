//! [`RasterPipeline`]: графический конвейер из чужого SPIR-V с кольцом наборов дескрипторов.

use ash::vk;

use super::create::{RASTER_STAGES, create_graphics_pipeline, create_pipeline_layout};
use crate::compute::descriptors::DescriptorRing;
use crate::compute::{BindingKind, BindingWrite, SpecializationConstant};
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::live_objects::{OwnedPipeline, OwnedPipelineLayout};

/// Как результат фрагментного шейдера смешивается с тем, что уже в цели.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    /// Заменяет содержимое (смешивание выключено).
    Opaque,
    /// Premultiplied alpha: шейдер отдаёт цвет, уже умноженный на альфу; результат = `src + dst·(1 − src.a)`
    /// для цвета и альфы (`src = ONE`, `dst = ONE_MINUS_SRC_ALPHA`).
    PremultipliedAlpha,
}

/// Описание растрового конвейера.
#[derive(Debug, Clone, Copy)]
pub struct RasterPipelineDesc<'a> {
    /// Короткое имя для ошибок и логов (например, `"ui-rects"`).
    pub label: &'static str,
    /// SPIR-V вершинного шейдера. Точка входа — `main`.
    pub vertex_spirv: &'a [u8],
    /// SPIR-V фрагментного шейдера. Точка входа — `main`.
    pub fragment_spirv: &'a [u8],
    /// Привязки единственного набора (`set = 0`), видимые стадиям `VERTEX | FRAGMENT`; номер привязки =
    /// позиция в срезе. Пусто — набора нет (шейдеры без ресурсов).
    pub bindings: &'a [BindingKind],
    /// Размер push-констант, байт: диапазон `VERTEX | FRAGMENT`, offset 0 (кратен 4; 0 — без них).
    pub push_constant_bytes: u32,
    /// Specialization-константы, применяются к обеим стадиям (идентификаторы не повторяются).
    pub specialization: &'a [SpecializationConstant],
    /// Формат цветовой цели; проходы с целью иного формата отклоняются.
    pub color_format: vk::Format,
    /// Смешивание.
    pub blend: BlendMode,
    /// Сколько наборов дескрипторов держать: обычно = числу кадров в полёте (> 0).
    pub frame_slots: usize,
}

/// Конвейер + раскладка + кольцо наборов. Порядок полей = порядок уничтожения: конвейер → его layout →
/// наборы (раскладка набора нужна layout-у только при создании).
pub struct RasterPipeline {
    pipeline: OwnedPipeline,
    pipeline_layout: OwnedPipelineLayout,
    /// `None` — привязок нет (описание с пустым `bindings`).
    descriptors: Option<DescriptorRing>,
    frame_slots: usize,
    bindings: Vec<BindingKind>,
    push_constant_bytes: u32,
    color_format: vk::Format,
    label: &'static str,
    context: GpuContext,
}

impl std::fmt::Debug for RasterPipeline {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "RasterPipeline({}, {:?})", self.label, self.pipeline)
    }
}

impl RasterPipeline {
    /// Создаёт конвейер. Все проверки описания — до создания объектов Vulkan.
    pub fn new(context: &GpuContext, desc: &RasterPipelineDesc<'_>) -> Result<Self, GpuError> {
        validate_desc(context, desc)?;
        let descriptors = if desc.bindings.is_empty() {
            None
        } else {
            Some(DescriptorRing::new(
                context,
                desc.bindings,
                RASTER_STAGES,
                desc.frame_slots,
                "vkCreateDescriptorSetLayout/Pool(raster)",
            )?)
        };
        let pipeline_layout = create_pipeline_layout(context, descriptors.as_ref(), desc.push_constant_bytes)?;
        let pipeline = create_graphics_pipeline(context, desc, &pipeline_layout)?;
        Ok(Self {
            pipeline,
            pipeline_layout,
            descriptors,
            frame_slots: desc.frame_slots,
            bindings: desc.bindings.to_vec(),
            push_constant_bytes: desc.push_constant_bytes,
            color_format: desc.color_format,
            label: desc.label,
            context: context.clone(),
        })
    }

    /// Число слотов наборов дескрипторов.
    pub fn frame_slots(&self) -> usize {
        self.frame_slots
    }

    /// Виды привязок по номерам (как в описании).
    pub fn bindings(&self) -> &[BindingKind] {
        &self.bindings
    }

    /// Формат цветовой цели, под который создан конвейер.
    pub fn color_format(&self) -> vk::Format {
        self.color_format
    }

    /// Переписывает привязки набора слота `frame_slot`. Проверки (номер, вид ресурса, диапазон и его
    /// лимит устройства) — до вызова Vulkan: при ошибке набор не меняется. Контракт — как у
    /// `ComputePipeline::bind`: GPU не читает набор этого слота; набор, привязанный проходом и ещё не
    /// освобождённый ([`Self::release_slot_after_fence`]), нельзя менять другими ресурсами (`InvalidState`).
    ///
    /// Конвейер без привязок принимает только пустой `writes` (иначе `InvalidArgument`: привязки вне 0..0).
    pub fn bind(&mut self, frame_slot: usize, writes: &[BindingWrite]) -> Result<(), GpuError> {
        match self.descriptors.as_mut() {
            Some(ring) => ring.write(frame_slot, writes),
            None => {
                self.check_slot_without_bindings(frame_slot)?;
                match writes.first() {
                    None => Ok(()),
                    Some(write) => Err(GpuError::InvalidArgument {
                        what: format!(
                            "{}: у конвейера нет привязок, а записана привязка {}",
                            self.label, write.binding
                        ),
                    }),
                }
            }
        }
    }

    /// Владелец подтверждает: fence последнего сабмита, в который записан `draw` со слотом `frame_slot`,
    /// сигнален — набор слота снова можно переписывать. Слот вне кольца — `InvalidArgument`.
    pub fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), GpuError> {
        match self.descriptors.as_ref() {
            Some(ring) => ring.release_after_fence(frame_slot),
            None => self.check_slot_without_bindings(frame_slot),
        }
    }

    /// Проверяет рисование БЕЗ записи команд (те же проверки, что в `RasterPass::draw`, кроме scissor):
    /// формат цели, размер push-констант, слот в кольце, все привязки набора записаны.
    pub(super) fn check_draw(
        &self,
        target_format: vk::Format,
        frame_slot: usize,
        push_constant_bytes: usize,
    ) -> Result<(), GpuError> {
        if target_format != self.color_format {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "{}: конвейер создан под формат {:?}, а цель прохода {target_format:?}",
                    self.label, self.color_format
                ),
            });
        }
        if push_constant_bytes != self.push_constant_bytes as usize {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "{}: push-константы {push_constant_bytes} байт, объявлено {}",
                    self.label, self.push_constant_bytes
                ),
            });
        }
        match self.descriptors.as_ref() {
            Some(ring) => ring.ready_set(frame_slot).map(|_| ()),
            None => self.check_slot_without_bindings(frame_slot),
        }
    }

    /// Записывает привязку конвейера, набора слота и push-констант (проходы `draw` затем рисуют).
    /// Вызывать только после успешного [`Self::check_draw`]: набор слота помечается «привязан».
    pub(super) fn record_bind(
        &self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        push_constants: &[u8],
    ) -> Result<(), GpuError> {
        let set = self
            .descriptors
            .as_ref()
            .map(|ring| ring.bind_for_recording(frame_slot))
            .transpose()?;
        let device = self.context.device();
        // SAFETY: командный буфер в состоянии записи внутри рендеринга (контракт `RasterPass`); конвейер,
        // layout и набор принадлежат self; размер push-констант равен объявленному диапазону
        // VERTEX|FRAGMENT (проверено в `check_draw`).
        unsafe {
            device.cmd_bind_pipeline(command_buffer, vk::PipelineBindPoint::GRAPHICS, self.pipeline.handle());
            if let Some(set) = set {
                device.cmd_bind_descriptor_sets(
                    command_buffer,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout.handle(),
                    0,
                    &[set],
                    &[],
                );
            }
            if !push_constants.is_empty() {
                device.cmd_push_constants(
                    command_buffer,
                    self.pipeline_layout.handle(),
                    RASTER_STAGES,
                    0,
                    push_constants,
                );
            }
        }
        Ok(())
    }

    /// Слот без кольца дескрипторов: достаточно, что он в пределах `frame_slots`.
    fn check_slot_without_bindings(&self, frame_slot: usize) -> Result<(), GpuError> {
        if frame_slot < self.frame_slots {
            Ok(())
        } else {
            Err(GpuError::InvalidArgument {
                what: format!("{}: слот {frame_slot} вне 0..{}", self.label, self.frame_slots),
            })
        }
    }
}

/// Проверяет описание: SPIR-V не пустые, слоты > 0, push-константы кратны 4 и в лимите, id констант
/// уникальны, формат цели умеет рисование (и смешивание для `PremultipliedAlpha`).
fn validate_desc(context: &GpuContext, desc: &RasterPipelineDesc<'_>) -> Result<(), GpuError> {
    let invalid = |what: String| GpuError::InvalidArgument { what };
    if desc.vertex_spirv.is_empty() || desc.fragment_spirv.is_empty() {
        return Err(invalid(format!(
            "{}: пустой SPIR-V вершинного или фрагментного шейдера",
            desc.label
        )));
    }
    if desc.frame_slots == 0 {
        return Err(invalid(format!("{}: число слотов 0", desc.label)));
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
    let mut required = vk::FormatFeatureFlags::COLOR_ATTACHMENT;
    if desc.blend == BlendMode::PremultipliedAlpha {
        required |= vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND;
    }
    let supported = context.optimal_tiling_features(desc.color_format);
    if !supported.contains(required) {
        return Err(GpuError::Unsupported {
            what: format!(
                "{}: формат цели {:?} не поддерживает {:?} (есть {supported:?})",
                desc.label,
                desc.color_format,
                required & !supported
            ),
        });
    }
    Ok(())
}
