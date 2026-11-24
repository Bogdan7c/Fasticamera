//! Реестр шейдеров накопления: пайплайны ВСЕХ приборов создаются один раз при создании
//! [`crate::scopes::ColorScopes`]; смена прибора, осей и размера позже пайплайнов не создаёт.
//!
//! Все шейдеры накопления имеют ОДИН интерфейс (`scope_accumulate_common.glsl`): привязки 0…2 и push-константы
//! `SamplingPush`; различается только SPIR-V. Поэтому новый прибор регистрируется парой «идентификатор + SPIR-V»
//! ([`AccumulatorShader`]) и не требует кода в проходах.

use gpu_vulkan::compute::{BindingKind, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant};
use gpu_vulkan::context::GpuContext;

use super::error::ScopeError;
use super::rule::AccumulatorId;
use crate::shaders::blobs;

/// Привязки шейдера накопления: показанная картинка, параметры, счётчики.
pub(super) const ACCUMULATE_BINDINGS: [BindingKind; 3] = [
    BindingKind::SampledImage,
    BindingKind::ReadOnlyStorageBuffer,
    BindingKind::ReadWriteStorageBuffer,
];

/// Размер push-констант прохода накопления: 7 слов `SamplingPush`.
pub(super) const SAMPLING_PUSH_BYTES: u32 = 28;

/// Размер рабочей группы всех проходов приборов (128 нитей — гарантия спецификации Vulkan для любого устройства).
pub(super) const LOCAL_WIDTH: u32 = 16;
/// Высота рабочей группы.
pub(super) const LOCAL_HEIGHT: u32 = 8;

/// `constant_id` ширины группы в шейдерах (`layout(local_size_x_id = 0)`).
pub(super) const LOCAL_SIZE_X_ID: u32 = 0;
/// `constant_id` высоты группы в шейдерах (`layout(local_size_y_id = 1)`).
pub(super) const LOCAL_SIZE_Y_ID: u32 = 1;

/// Specialization-константы размера группы: шейдеры получают размер от Rust, а не из своих литералов.
pub(super) fn local_size_specialization(local: LocalSize) -> [SpecializationConstant; 2] {
    [
        SpecializationConstant {
            constant_id: LOCAL_SIZE_X_ID,
            value: local.x.get(),
        },
        SpecializationConstant {
            constant_id: LOCAL_SIZE_Y_ID,
            value: local.y.get(),
        },
    ]
}

/// Сколько отсчётов по y обходит нить у `scope_accumulate_integer_runs.comp` (`SCOPE_ROWS_PER_THREAD` шейдера;
/// совпадение сверяет тест `shader_literals_tests`).
pub(super) const INTEGER_RUNS_ROWS_PER_THREAD: u32 = 8;

/// Шейдер накопления для регистрации.
#[derive(Debug, Clone, Copy)]
pub struct AccumulatorShader {
    /// Идентификатор, по которому прибор выбирает шейдер.
    pub id: AccumulatorId,
    /// SPIR-V (точка входа `main`, интерфейс — `scope_accumulate_common.glsl`).
    pub spirv: &'static [u8],
    /// Сколько отсчётов подряд по оси y обходит одна нить (`SCOPE_ROWS_PER_THREAD` шейдера; 1 — «одна нить — один
    /// отсчёт»). Rust делит на это число высоту dispatch, поэтому значение обязано совпасть с шейдером.
    pub rows_per_thread: u32,
}

/// Шейдеры приборов `scope-model`: целочисленная проекция (прямая запись и серии в регистре) и OKLab.
pub fn standard_accumulators() -> [AccumulatorShader; 3] {
    [
        AccumulatorShader {
            id: AccumulatorId::INTEGER_PROJECTION_RUNS,
            spirv: blobs::SCOPE_ACCUMULATE_INTEGER_RUNS_COMP,
            rows_per_thread: INTEGER_RUNS_ROWS_PER_THREAD,
        },
        AccumulatorShader {
            id: AccumulatorId::INTEGER_PROJECTION,
            spirv: blobs::SCOPE_ACCUMULATE_INTEGER_COMP,
            rows_per_thread: 1,
        },
        AccumulatorShader {
            id: AccumulatorId::OKLAB_VECTORSCOPE,
            spirv: blobs::SCOPE_ACCUMULATE_OKLAB_COMP,
            rows_per_thread: 1,
        },
    ]
}

/// Шейдеры со слиянием одинаковых ячеек в subgroup (вектороскопы): регистрируются, только если устройство умеет
/// баллот subgroup в compute (`GpuCapabilities::subgroup_ballot_in_compute`).
pub fn merged_accumulators() -> [AccumulatorShader; 2] {
    [
        AccumulatorShader {
            id: AccumulatorId::INTEGER_PROJECTION_MERGED,
            spirv: blobs::SCOPE_ACCUMULATE_INTEGER_MERGED_COMP,
            rows_per_thread: 1,
        },
        AccumulatorShader {
            id: AccumulatorId::OKLAB_VECTORSCOPE_MERGED,
            spirv: blobs::SCOPE_ACCUMULATE_OKLAB_MERGED_COMP,
            rows_per_thread: 1,
        },
    ]
}

/// Пайплайны накопления по идентификаторам.
pub(super) struct AccumulatorSet {
    entries: Vec<(AccumulatorId, ComputePipeline)>,
    /// Отсчётов по y на нить у каждого шейдера (параллельно `entries`).
    rows_per_thread: Vec<u32>,
}

impl AccumulatorSet {
    /// Создаёт пайплайн на каждый шейдер. Повтор идентификатора — ошибка.
    pub(super) fn new(
        context: &GpuContext,
        shaders: &[AccumulatorShader],
        local: LocalSize,
        frame_slots: usize,
    ) -> Result<Self, ScopeError> {
        let mut entries: Vec<(AccumulatorId, ComputePipeline)> = Vec::with_capacity(shaders.len());
        let mut rows_per_thread = Vec::with_capacity(shaders.len());
        for shader in shaders {
            if shader.rows_per_thread == 0 {
                return Err(ScopeError::InvalidConfig {
                    what: format!("шейдер {}: rows_per_thread = 0", shader.id.name()),
                });
            }
            if entries.iter().any(|(id, _)| *id == shader.id) {
                return Err(ScopeError::DuplicateAccumulator { id: shader.id });
            }
            let pipeline = ComputePipeline::new(
                context,
                &ComputePipelineDesc {
                    label: shader.id.name(),
                    spirv: shader.spirv,
                    bindings: &ACCUMULATE_BINDINGS,
                    push_constant_bytes: SAMPLING_PUSH_BYTES,
                    specialization: &local_size_specialization(local),
                    frame_slots,
                },
            )?;
            entries.push((shader.id, pipeline));
            rows_per_thread.push(shader.rows_per_thread);
        }
        Ok(Self {
            entries,
            rows_per_thread,
        })
    }

    /// Сколько пайплайнов накопления создано.
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }

    /// Идентификатор, под которым шейдер прибора реально зарегистрирован: сам `id` или его запасной
    /// ([`AccumulatorId::fallback`]), если устройство не умеет вариант прибора.
    pub(super) fn resolve(&self, id: AccumulatorId) -> Result<AccumulatorId, ScopeError> {
        let registered = |candidate: AccumulatorId| self.entries.iter().any(|(known, _)| *known == candidate);
        if registered(id) {
            return Ok(id);
        }
        id.fallback()
            .filter(|fallback| registered(*fallback))
            .ok_or(ScopeError::UnknownAccumulator { id })
    }

    /// Сколько отсчётов по y обходит одна нить шейдера прибора (с учётом запасного шейдера).
    pub(super) fn rows_per_thread(&self, id: AccumulatorId) -> Result<u32, ScopeError> {
        let id = self.resolve(id)?;
        self.entries
            .iter()
            .position(|(candidate, _)| *candidate == id)
            .and_then(|index| self.rows_per_thread.get(index).copied())
            .ok_or(ScopeError::UnknownAccumulator { id })
    }

    pub(super) fn get(&self, id: AccumulatorId) -> Result<&ComputePipeline, ScopeError> {
        let id = self.resolve(id)?;
        self.entries
            .iter()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, pipeline)| pipeline)
            .ok_or(ScopeError::UnknownAccumulator { id })
    }

    pub(super) fn get_mut(&mut self, id: AccumulatorId) -> Result<&mut ComputePipeline, ScopeError> {
        let id = self.resolve(id)?;
        self.entries
            .iter_mut()
            .find(|(candidate, _)| *candidate == id)
            .map(|(_, pipeline)| pipeline)
            .ok_or(ScopeError::UnknownAccumulator { id })
    }

    /// Fence слота сигнален: наборы слота всех пайплайнов снова можно переписывать.
    pub(super) fn release_slot_after_fence(&self, frame_slot: usize) -> Result<(), ScopeError> {
        for (_, pipeline) in &self.entries {
            pipeline.release_slot_after_fence(frame_slot)?;
        }
        Ok(())
    }
}

/// Размер рабочей группы проходов приборов.
pub(super) fn local_size() -> Result<LocalSize, ScopeError> {
    Ok(LocalSize::new(LOCAL_WIDTH, LOCAL_HEIGHT)?)
}
