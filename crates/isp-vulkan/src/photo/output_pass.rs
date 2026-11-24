//! Выходной проход снимка: вариант пайплайна на модель основы тона (сессия 16a).
//!
//! # Зачем
//! Общий выходной шейдер читает номер модели основы тона из uniform-блока на лету и несёт код всех четырёх моделей.
//! На Adreno 740 исполнение дорого, а присутствие неисполняемых веток тоже стоит (`photo_tone_model.glsl`).
//! Модель снимка известна при `begin`, поэтому для каждой модели заведён свой пайплайн: номер — константа
//! специализации, ветки остальных моделей драйвер выбрасывает. Формулы не меняются (решение 213: фото бит-в-бит).
//!
//! # Владение
//! [`OutputPasses`] владеет пайплайнами и их наборами дескрипторов. Какой вариант нужен, решает вызывающий
//! ([`ToneBaseKind`] проявки), а не хранилище: проход вызывает `bind_for`/`record_for`/`check_for` с этой моделью.
//!
//! # Прежний путь
//! [`ToneModelSpecialization::Runtime`] строит ОДИН пайплайн из замороженной копии прежнего шейдера
//! (`photo_output_rgba8_reference.comp`): эталон тестов точного равенства и A/B на устройстве.

use ash::vk;
use gpu_vulkan::compute::{
    BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;

use super::config::ToneModelSpecialization;
use crate::quality::pass::{PassPushConstants, QualityPass};
use crate::quality::stages::OUTPUT_BINDINGS;
use crate::shaders::blobs;
use crate::tone_base::ToneBaseKind;

/// Единственный слот наборов дескрипторов: проявление одно за раз.
const SLOT: usize = 0;
/// `constant_id` размера группы по X и Y (общий для всех проходов) и номера модели основы тона (`photo_tone_model.glsl`).
const LOCAL_SIZE_X_ID: u32 = 0;
const LOCAL_SIZE_Y_ID: u32 = 1;
const TONE_MODEL_ID: u32 = 2;
/// Размер push-констант прохода, байт (раскладка `PlaneGeometry`: 4 × u32 + vec4).
const PUSH_CONSTANT_BYTES: u32 = 32;

/// Пайплайн выходного прохода под ОДНУ модель основы тона.
struct SpecializedOutput {
    pipeline: ComputePipeline,
    local: LocalSize,
}

impl SpecializedOutput {
    fn new(context: &GpuContext, local: LocalSize, tone: ToneBaseKind) -> Result<Self, GpuError> {
        let pipeline = ComputePipeline::new(
            context,
            &ComputePipelineDesc {
                label: "isp-photo-output-rgba8",
                spirv: blobs::PHOTO_OUTPUT_RGBA8_COMP,
                bindings: &OUTPUT_BINDINGS,
                push_constant_bytes: PUSH_CONSTANT_BYTES,
                specialization: &[
                    SpecializationConstant {
                        constant_id: LOCAL_SIZE_X_ID,
                        value: local.x.get(),
                    },
                    SpecializationConstant {
                        constant_id: LOCAL_SIZE_Y_ID,
                        value: local.y.get(),
                    },
                    SpecializationConstant {
                        constant_id: TONE_MODEL_ID,
                        value: tone.model_id(),
                    },
                ],
                frame_slots: 1,
            },
        )?;
        Ok(Self { pipeline, local })
    }

    fn groups(&self, push: &PassPushConstants) -> WorkgroupCount {
        WorkgroupCount::covering(push.extent.width, push.extent.height, self.local)
    }
}

/// Раскладка push-констант `PlaneGeometry` (зеркало `PassPushConstants::to_bytes` стенда V4: те же 32 байта).
fn push_bytes(push: &PassPushConstants) -> Vec<u8> {
    [
        push.extent.width,
        push.extent.height,
        push.cfa.shader_value(),
        push.pass_mode,
    ]
    .iter()
    .flat_map(|word| word.to_ne_bytes())
    .chain(push.pass_parameters.iter().flat_map(|value| value.to_ne_bytes()))
    .collect()
}

/// Что стоит за выходным проходом проявителя.
enum Variants {
    /// Вариант на каждую модель основы (порядок — [`ToneBaseKind::ALL`]).
    PerModel(Vec<(ToneBaseKind, SpecializedOutput)>),
    /// Прежний путь: один пайплайн, модель читается из блока на лету.
    Runtime(Box<QualityPass>),
}

/// Выходной проход снимка (все варианты).
pub(crate) struct OutputPasses {
    variants: Variants,
}

impl OutputPasses {
    /// Создаёт пайплайны согласно режиму. `PerModel` создаёт все четыре сразу: пайплайн на первом снимке посреди
    /// съёмки был бы заметной задержкой.
    pub(crate) fn new(context: &GpuContext, local: LocalSize, mode: ToneModelSpecialization) -> Result<Self, GpuError> {
        let variants = match mode {
            ToneModelSpecialization::PerModel => Variants::PerModel(
                ToneBaseKind::ALL
                    .into_iter()
                    .map(|tone| SpecializedOutput::new(context, local, tone).map(|output| (tone, output)))
                    .collect::<Result<Vec<_>, GpuError>>()?,
            ),
            ToneModelSpecialization::Runtime => Variants::Runtime(Box::new(QualityPass::new(
                context,
                "isp-photo-output-rgba8-runtime-model",
                blobs::PHOTO_OUTPUT_RGBA8_REFERENCE_COMP,
                &OUTPUT_BINDINGS,
                local,
                1,
            )?)),
        };
        Ok(Self { variants })
    }

    fn specialized(&self, tone: ToneBaseKind) -> Result<&SpecializedOutput, GpuError> {
        match &self.variants {
            Variants::PerModel(outputs) => outputs
                .iter()
                .find(|(kind, _)| *kind == tone)
                .map(|(_, output)| output)
                .ok_or_else(|| GpuError::InvalidState {
                    what: format!("нет варианта выходного прохода для модели {tone:?}"),
                }),
            Variants::Runtime(_) => Err(GpuError::InvalidState {
                what: "режим Runtime не имеет вариантов по модели".to_owned(),
            }),
        }
    }

    fn specialized_mut(&mut self, tone: ToneBaseKind) -> Result<&mut SpecializedOutput, GpuError> {
        match &mut self.variants {
            Variants::PerModel(outputs) => outputs
                .iter_mut()
                .find(|(kind, _)| *kind == tone)
                .map(|(_, output)| output)
                .ok_or_else(|| GpuError::InvalidState {
                    what: format!("нет варианта выходного прохода для модели {tone:?}"),
                }),
            Variants::Runtime(_) => Err(GpuError::InvalidState {
                what: "режим Runtime не имеет вариантов по модели".to_owned(),
            }),
        }
    }

    /// Привязывает ресурсы проявки к варианту под `tone` (остальные варианты не трогаются).
    pub(crate) fn bind_for(&mut self, tone: ToneBaseKind, writes: &[BindingWrite]) -> Result<(), GpuError> {
        match &mut self.variants {
            Variants::Runtime(pass) => pass.bind(SLOT, writes),
            Variants::PerModel(_) => self.specialized_mut(tone)?.pipeline.bind(SLOT, writes),
        }
    }

    /// Проверка dispatch варианта `tone` без записи команд.
    pub(crate) fn check_for(&self, tone: ToneBaseKind, push: &PassPushConstants) -> Result<(), GpuError> {
        match &self.variants {
            Variants::Runtime(pass) => pass.check(SLOT, push),
            Variants::PerModel(_) => {
                let output = self.specialized(tone)?;
                output
                    .pipeline
                    .check_dispatch(SLOT, PUSH_CONSTANT_BYTES as usize, output.groups(push))
            }
        }
    }

    /// Записывает dispatch варианта `tone`.
    pub(crate) fn record_for(
        &self,
        tone: ToneBaseKind,
        command_buffer: vk::CommandBuffer,
        push: &PassPushConstants,
    ) -> Result<(), GpuError> {
        match &self.variants {
            Variants::Runtime(pass) => pass.record(command_buffer, SLOT, push),
            Variants::PerModel(_) => {
                let output = self.specialized(tone)?;
                output
                    .pipeline
                    .record_dispatch(command_buffer, SLOT, &push_bytes(push), output.groups(push))
            }
        }
    }

    /// Fence всех сабмитов проявки подтверждены: наборы всех вариантов можно привязывать заново (вариант, которого
    /// проявка не касалась, освобождается без последствий).
    pub(crate) fn release_slot_after_fence(&self) -> Result<(), GpuError> {
        match &self.variants {
            Variants::Runtime(pass) => pass.release_slot_after_fence(SLOT),
            Variants::PerModel(outputs) => outputs
                .iter()
                .try_for_each(|(_, output)| output.pipeline.release_slot_after_fence(SLOT)),
        }
    }
}
