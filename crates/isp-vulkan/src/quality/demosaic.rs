//! [`QualityDemosaic`] — демозаик Menon 2007 с шагом уточнения на GPU, полное разрешение.
//!
//! Семантика (включая граничные правила и выбор H/V при равенстве) — как у эталона
//! `isp_model::demosaic` (`DemosaicMethod::Quality`). Восемь проходов ([`MenonPass`]) над плоскостями f32
//! ([`QualityPlanes`]); каждый проход — одна формула эталона, порядок сложений повторён (`precise` в GLSL),
//! поэтому до шага уточнения GPU совпадает с CPU бит в бит, а после — с точностью деления на 3
//! (OpFDiv в Vulkan — до 2,5 ULP, на CPU деление точное).

use ash::vk;
use gpu_vulkan::compute::{BindingKind, BindingWrite, LocalSize};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::record_compute_pass_dependency;
use gpu_vulkan::error::GpuError;

use super::error::QualityError;
use super::frame_params::CfaCode;
use super::pass::{PassPushConstants, QualityPass};
use super::planes::{PlaneSlot, QualityPlanes};
use crate::shaders::blobs;

/// Проход демозаика (в порядке исполнения).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenonPass {
    /// 1. Оценки зелёного по строке и по столбцу.
    DirectionalGreen,
    /// 2а. Модули цветовых разностей D_H, D_V для классификатора.
    ColorDifferences,
    /// 2б. Решение H/V по классификатору и выбранный зелёный.
    Decision,
    /// 3. R и B в зелёных узлах.
    ChromaAtGreen,
    /// 4. R в синих узлах, B в красных.
    ChromaAtOpposite,
    /// 5. Уточнение: зелёный в R/B-узлах.
    RefineGreen,
    /// 6. Уточнение: R и B в зелёных узлах.
    RefineChromaAtGreen,
    /// 7. Уточнение: R в синих, B в красных.
    RefineChromaAtOpposite,
}

/// Доступ прохода к плоскости: номер привязки = позиция в списке.
#[derive(Debug, Clone, Copy)]
pub(crate) enum PlaneAccess {
    Read(PlaneSlot),
    Write(PlaneSlot),
}

impl MenonPass {
    /// Порядок исполнения.
    pub const ORDER: [Self; 8] = [
        Self::DirectionalGreen,
        Self::ColorDifferences,
        Self::Decision,
        Self::ChromaAtGreen,
        Self::ChromaAtOpposite,
        Self::RefineGreen,
        Self::RefineChromaAtGreen,
        Self::RefineChromaAtOpposite,
    ];

    /// Короткое имя (метки времени, логи).
    pub fn label(self) -> &'static str {
        match self {
            Self::DirectionalGreen => "menon_green",
            Self::ColorDifferences => "menon_differences",
            Self::Decision => "menon_decision",
            Self::ChromaAtGreen => "menon_chroma_green",
            Self::ChromaAtOpposite => "menon_chroma_opposite",
            Self::RefineGreen => "menon_refine_green",
            Self::RefineChromaAtGreen => "menon_refine_chroma_green",
            Self::RefineChromaAtOpposite => "menon_refine_chroma_opposite",
        }
    }

    pub(crate) fn spirv(self) -> &'static [u8] {
        match self {
            Self::DirectionalGreen => blobs::QUALITY_MENON_GREEN_COMP,
            Self::ColorDifferences => blobs::QUALITY_MENON_DIFFERENCES_COMP,
            Self::Decision => blobs::QUALITY_MENON_DECISION_COMP,
            Self::ChromaAtGreen => blobs::QUALITY_MENON_CHROMA_GREEN_COMP,
            Self::ChromaAtOpposite => blobs::QUALITY_MENON_CHROMA_OPPOSITE_COMP,
            Self::RefineGreen => blobs::QUALITY_MENON_REFINE_GREEN_COMP,
            Self::RefineChromaAtGreen => blobs::QUALITY_MENON_REFINE_CHROMA_GREEN_COMP,
            Self::RefineChromaAtOpposite => blobs::QUALITY_MENON_REFINE_CHROMA_OPPOSITE_COMP,
        }
    }

    /// Плоскости прохода в порядке привязок шейдера (таблица слотов — `planes.rs`).
    pub(crate) fn plane_accesses(self) -> &'static [PlaneAccess] {
        use PlaneAccess::{Read, Write};
        use PlaneSlot::{Blue, Direction, Green, Mosaic, Red, ScratchHorizontal, ScratchVertical};
        match self {
            Self::DirectionalGreen => &[Read(Mosaic), Write(ScratchHorizontal), Write(ScratchVertical)],
            // D_H/D_V временно занимают слоты Red/Blue: до прохода 4 они свободны.
            Self::ColorDifferences => &[
                Read(Mosaic),
                Read(ScratchHorizontal),
                Read(ScratchVertical),
                Write(Red),
                Write(Blue),
            ],
            Self::Decision => &[
                Read(Red),
                Read(Blue),
                Read(ScratchHorizontal),
                Read(ScratchVertical),
                Write(Green),
                Write(Direction),
            ],
            Self::ChromaAtGreen => &[
                Read(Mosaic),
                Read(Green),
                Write(ScratchHorizontal),
                Write(ScratchVertical),
            ],
            Self::ChromaAtOpposite => &[
                Read(ScratchHorizontal),
                Read(ScratchVertical),
                Read(Direction),
                Write(Red),
                Write(Blue),
            ],
            // Уточнённый зелёный ложится на место мозаики: она больше не нужна.
            Self::RefineGreen => &[Read(Red), Read(Green), Read(Blue), Read(Direction), Write(Mosaic)],
            Self::RefineChromaAtGreen => &[
                Read(Red),
                Read(Blue),
                Read(Mosaic),
                Write(ScratchHorizontal),
                Write(ScratchVertical),
            ],
            Self::RefineChromaAtOpposite => &[
                Read(ScratchHorizontal),
                Read(ScratchVertical),
                Read(Direction),
                Write(Red),
                Write(Blue),
            ],
        }
    }
}

/// Создаёт пайплайны всех проходов Menon над плоскостями `planes`; привязки плоскостей пишутся сразу во все
/// слоты. Общая часть стенда V4 и снимка (`photo`): снимок держит плоскости под полосу, а не под кадр, и
/// сам решает, какого размера dispatch писать, но пайплайны и привязки плоскостей у них одни.
pub(crate) fn create_pass_pipelines(
    context: &GpuContext,
    planes: &QualityPlanes,
    local: LocalSize,
    frame_slots: usize,
) -> Result<Vec<(MenonPass, QualityPass)>, GpuError> {
    let mut passes = Vec::with_capacity(MenonPass::ORDER.len());
    for pass in MenonPass::ORDER {
        let accesses = pass.plane_accesses();
        let kinds: Vec<BindingKind> = accesses
            .iter()
            .map(|access| match access {
                PlaneAccess::Read(_) => BindingKind::ReadOnlyStorageBuffer,
                PlaneAccess::Write(_) => BindingKind::ReadWriteStorageBuffer,
            })
            .collect();
        let mut pipeline = QualityPass::new(context, pass.label(), pass.spirv(), &kinds, local, frame_slots)?;
        let writes: Vec<BindingWrite> = (0_u32..)
            .zip(accesses)
            .map(|(binding, access)| BindingWrite {
                binding,
                resource: match *access {
                    PlaneAccess::Read(slot) => planes.plane(slot).read_only_binding(),
                    PlaneAccess::Write(slot) => planes.plane(slot).read_write_binding(),
                },
            })
            .collect();
        pipeline.bind_all_slots(&writes)?;
        passes.push((pass, pipeline));
    }
    Ok(passes)
}

/// Демозаик Menon 2007 на GPU: владеет плоскостями и пайплайнами проходов.
///
/// Порядок полей = порядок уничтожения: пайплайны (их наборы ссылаются на плоскости) → плоскости.
pub struct QualityDemosaic {
    passes: Vec<(MenonPass, QualityPass)>,
    planes: QualityPlanes,
}

impl std::fmt::Debug for QualityDemosaic {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "QualityDemosaic({:?})", self.planes)
    }
}

impl QualityDemosaic {
    /// Создаёт плоскости размера `extent` и пайплайны; привязки плоскостей пишутся сразу во все слоты.
    pub fn new(
        context: &GpuContext,
        extent: vk::Extent2D,
        local: LocalSize,
        frame_slots: usize,
    ) -> Result<Self, QualityError> {
        let planes = QualityPlanes::new(context, extent)?;
        let passes = create_pass_pipelines(context, &planes, local, frame_slots)?;
        Ok(Self { passes, planes })
    }

    /// Плоскости (мозаика — вход, `linear_rgb` — итог).
    pub fn planes(&self) -> &QualityPlanes {
        &self.planes
    }

    /// Проверяет dispatch всех проходов слота без записи команд (звать до первой команды кадра).
    pub(crate) fn check_all(&self, frame_slot: usize, cfa: CfaCode) -> Result<(), GpuError> {
        let push = PassPushConstants::geometry(self.planes.extent(), cfa);
        for (_, pipeline) in &self.passes {
            pipeline.check(frame_slot, &push)?;
        }
        Ok(())
    }

    /// Записывает один проход. Барьер перед ним (зависимость от прошлого прохода) пишет вызывающий:
    /// так стенд ставит метку времени между барьером и следующим проходом.
    pub fn record_pass(
        &self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        pass: MenonPass,
        cfa: CfaCode,
    ) -> Result<(), GpuError> {
        let pipeline = self
            .passes
            .iter()
            .find(|(candidate, _)| *candidate == pass)
            .map(|(_, pipeline)| pipeline)
            .ok_or_else(|| GpuError::InvalidState {
                what: format!("проход {pass:?} не создан"),
            })?;
        pipeline.record(
            command_buffer,
            frame_slot,
            &PassPushConstants::geometry(self.planes.extent(), cfa),
        )
    }

    /// Все проходы подряд с барьером перед каждым (первый барьер ждёт и записи мозаики, и прошлый кадр).
    pub fn record_all(
        &self,
        context: &GpuContext,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        cfa: CfaCode,
    ) -> Result<(), GpuError> {
        for pass in MenonPass::ORDER {
            record_compute_pass_dependency(context, command_buffer);
            self.record_pass(command_buffer, frame_slot, pass, cfa)?;
        }
        Ok(())
    }
}
