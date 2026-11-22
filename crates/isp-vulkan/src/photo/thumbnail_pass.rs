//! GPU-часть миниатюры снимка (сессия 14g): проход уменьшения, цели и их жизнь внутри проявки.
//!
//! Формулы и правило охвата — в [`super::thumbnail`] (CPU-эталон, источник правды). Здесь — владение:
//! * [`ThumbnailPass`] — пайплайн (живёт столько же, сколько проявитель);
//! * [`ThumbnailTargets`] — маленькая картинка и буфер её копии в память хоста (≈ 0,2 МБ), выделяются вместе с
//!   остальными целями проявки (на потоке мастерской), для ЛЮБОГО режима `RgbaReadback` (у HEIF копии основной
//!   картинки нет, а миниатюра нужна);
//! * [`ThumbnailJob`] — состояние миниатюры внутри одной проявки: записывается в ТОТ ЖЕ командный буфер, что и
//!   последняя полоса (без нового сабмита и слота очереди), печатается тем же доказательством.
//!
//! # Инвариант «миниатюра не роняет проявку»
//! Любой сбой миниатюры (нет пайплайна, не выделились цели, проход не принят проверкой, печать отказана)
//! переводит её в [`ThumbnailUnavailable`]; проявка основной картинки идёт как шла.

use ash::vk;
use gpu_vulkan::compute::{BindingKind, BindingWrite, LocalSize};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::host_readback::{HostReadbackBuffer, SealedHostBytes};
use gpu_vulkan::one_shot_queue::{CompletedWork, SubmittedWork};
use gpu_vulkan::rgba8_image::Rgba8StorageImage;

use super::error::PhotoError;
use super::pixels::seal_or_leak;
use super::thumbnail::{ThumbnailOutcome, ThumbnailPlan, ThumbnailUnavailable, rotation_code};
use crate::quality::frame_params::CfaCode;
use crate::quality::pass::{PassPushConstants, QualityPass};
use crate::shaders::blobs;

/// Единственный слот наборов дескрипторов: проявление одно за раз.
const SLOT: usize = 0;

/// Привязки: 0 — проявленный снимок (шейдер объявляет его `readonly`; дескриптор тот же STORAGE_IMAGE, что у
/// записи, отдельного вида «чтение» у `gpu-vulkan` нет), 1 — миниатюра (запись). Раскладка обеих — `GENERAL`.
const THUMBNAIL_BINDINGS: [BindingKind; 2] = [BindingKind::StorageImageWrite, BindingKind::StorageImageWrite];

/// Пайплайн уменьшения.
pub(crate) struct ThumbnailPass {
    pass: QualityPass,
    /// Размер рабочей группы: группа на пиксель миниатюры, поэтому от него зависит `extent` push-констант.
    local: LocalSize,
}

impl ThumbnailPass {
    pub(crate) fn new(context: &GpuContext, local: LocalSize) -> Result<Self, GpuError> {
        Ok(Self {
            pass: QualityPass::new(
                context,
                "isp-photo-thumbnail",
                blobs::PHOTO_THUMBNAIL_COMP,
                &THUMBNAIL_BINDINGS,
                local,
                1,
            )?,
            local,
        })
    }

    /// Fence всех сабмитов проявки подтверждены: набор можно привязывать к новым ресурсам.
    pub(crate) fn release_slot_after_fence(&self) -> Result<(), GpuError> {
        self.pass.release_slot_after_fence(SLOT)
    }
}

/// Маленькая картинка и буфер копии: то, что нужно GPU, чтобы получить миниатюру.
#[derive(Debug)]
pub(crate) struct ThumbnailBuffers {
    image: Rgba8StorageImage,
    readback: HostReadbackBuffer,
}

/// Цели миниатюры, выделенные заранее (вместе с целями проявки).
#[derive(Debug)]
pub struct ThumbnailTargets {
    plan: ThumbnailPlan,
    /// Сбой выделения не ломает проявку: миниатюры просто не будет.
    buffers: Result<ThumbnailBuffers, ThumbnailUnavailable>,
}

impl ThumbnailTargets {
    /// Выделяет цели под `plan`. Только память и объекты Vulkan, без команд GPU: годится для любого потока.
    pub(crate) fn allocate(context: &GpuContext, plan: ThumbnailPlan) -> Self {
        let size = plan.size();
        let allocated = Rgba8StorageImage::new(
            context,
            vk::Extent2D {
                width: size.width,
                height: size.height,
            },
        )
        .and_then(|image| {
            let readback = HostReadbackBuffer::new(context, image.packed_size_bytes())?;
            Ok(ThumbnailBuffers { image, readback })
        });
        let buffers = allocated.map_err(|error| {
            tracing::warn!(
                target: "fasticamera::isp",
                %error,
                "цели миниатюры не выделены: снимок проявится без неё"
            );
            ThumbnailUnavailable::TargetsNotAllocated(error.to_string())
        });
        Self { plan, buffers }
    }

    /// План, под который выделены цели (проявка сверяет его со своим).
    pub fn plan(&self) -> ThumbnailPlan {
        self.plan
    }

    /// Получилось ли выделить цели (для тестов и диагностики).
    pub fn is_allocated(&self) -> bool {
        self.buffers.is_ok()
    }
}

/// Состояние миниатюры в одной проявке.
pub(crate) struct ThumbnailJob {
    plan: ThumbnailPlan,
    state: JobState,
}

enum JobState {
    /// Проход записывается в последнюю полосу.
    Active {
        buffers: Box<ThumbnailBuffers>,
        push: PassPushConstants,
    },
    /// Миниатюры не будет; причина попадёт в итог.
    Skipped(ThumbnailUnavailable),
}

impl ThumbnailJob {
    /// Начало проявки: привязывает проход к проявленной картинке и миниатюре и проверяет dispatch ДО записи
    /// каких-либо команд. Любая неудача — `Skipped`, не ошибка проявки.
    pub(crate) fn begin(
        targets: ThumbnailTargets,
        pass: Option<&mut ThumbnailPass>,
        developed: &Rgba8StorageImage,
        cfa: CfaCode,
    ) -> Self {
        let ThumbnailTargets { plan, buffers } = targets;
        let skipped = |reason: ThumbnailUnavailable| Self {
            plan,
            state: JobState::Skipped(reason),
        };
        let buffers = match buffers {
            Ok(buffers) => Box::new(buffers),
            Err(reason) => return skipped(reason),
        };
        let Some(pass) = pass else {
            return skipped(ThumbnailUnavailable::PipelineMissing);
        };
        let size = plan.size();
        // Одна рабочая группа на пиксель миниатюры (шейдер): `extent` — размер в НИТЯХ, из него `QualityPass`
        // считает число групп, и оно равно размеру миниатюры.
        let push = PassPushConstants {
            pass_mode: rotation_code(plan.rotation()),
            ..PassPushConstants::geometry(
                vk::Extent2D {
                    width: size.width.saturating_mul(pass.local.x.get()),
                    height: size.height.saturating_mul(pass.local.y.get()),
                },
                cfa,
            )
        };
        let prepared = pass
            .pass
            .bind(
                SLOT,
                &[
                    BindingWrite {
                        binding: 0,
                        resource: developed.storage_binding(),
                    },
                    BindingWrite {
                        binding: 1,
                        resource: buffers.image.storage_binding(),
                    },
                ],
            )
            .and_then(|()| pass.pass.check(SLOT, &push));
        match prepared {
            Ok(()) => Self {
                plan,
                state: JobState::Active { buffers, push },
            },
            Err(error) => {
                tracing::warn!(target: "fasticamera::isp", %error, "миниатюра не принята проверкой: проявка без неё");
                skipped(ThumbnailUnavailable::NotRecorded(error.to_string()))
            }
        }
    }

    /// Записывает уменьшение и копию в память хоста в `command_buffer` — в шаге отделки (после
    /// выходного прохода последней полосы вывода и зависимости, которую записал шаг), до перевода основной картинки
    /// к копированию. Не активная миниатюра — ничего не пишет.
    pub(crate) fn record(&self, pass: &ThumbnailPass, command_buffer: vk::CommandBuffer) -> Result<(), PhotoError> {
        let JobState::Active { buffers, push } = &self.state else {
            return Ok(());
        };
        // Зависимость «выходной проход → наш читатель» записал шаг отделки (`finishing`) перед вызовом.
        buffers.image.record_prepare_for_compute_write(command_buffer);
        pass.pass.record(command_buffer, SLOT, push)?;
        buffers.image.record_compute_write_to_transfer_read(command_buffer);
        buffers
            .image
            .record_copy_to_host_readback(command_buffer, &buffers.readback)?;
        Ok(())
    }

    /// Запоминает, какая отправленная работа пишет в буфер миниатюры (как `HostReadbackBuffer::mark_written_by`).
    pub(crate) fn mark_written_by(&mut self, work: &SubmittedWork) {
        if let JobState::Active { buffers, .. } = &mut self.state {
            buffers.readback.mark_written_by(work);
        }
    }

    /// Все полосы подтверждены: печатает буфер доказательством последней полосы. Картинку миниатюры (GPU
    /// закончил) отпускает; в слоте остаётся только запечатанный буфер.
    pub(crate) fn seal(self, proof: Option<&CompletedWork>) -> ThumbnailSlot {
        let plan = self.plan;
        match self.state {
            JobState::Skipped(reason) => ThumbnailSlot::Unavailable(reason),
            JobState::Active { buffers, .. } => {
                let Some(proof) = proof else {
                    // Нет доказательства — нельзя ни печатать, ни уничтожать: GPU мог писать (соглашение крейта).
                    std::mem::forget(buffers);
                    return ThumbnailSlot::Unavailable(ThumbnailUnavailable::SealRefused(
                        "нет доказательства завершения последней полосы".to_owned(),
                    ));
                };
                let ThumbnailBuffers { image, readback } = *buffers;
                drop(image);
                match seal_or_leak(readback, proof) {
                    Ok(bytes) => ThumbnailSlot::Sealed { plan, bytes },
                    Err(error) => ThumbnailSlot::Unavailable(ThumbnailUnavailable::SealRefused(error.to_string())),
                }
            }
        }
    }
}

/// Миниатюра внутри готового снимка: запечатанные байты или причина отсутствия.
pub(crate) enum ThumbnailSlot {
    Sealed {
        plan: ThumbnailPlan,
        bytes: SealedHostBytes,
    },
    Unavailable(ThumbnailUnavailable),
}

impl ThumbnailSlot {
    /// Отдаёт миниатюру ОДИН раз: байты копируются в обычную память, буфер GPU уничтожается.
    pub(crate) fn take(&mut self) -> ThumbnailOutcome {
        match std::mem::replace(self, Self::Unavailable(ThumbnailUnavailable::AlreadyTaken)) {
            Self::Unavailable(reason) => {
                // Причину повторно не теряем: первый `take` вернул её, дальше — «уже забрали».
                ThumbnailOutcome::Unavailable(reason)
            }
            Self::Sealed { plan, bytes } => {
                let rgba = bytes.bytes().to_vec();
                if u64::try_from(rgba.len()).ok() != Some(plan.byte_len()) {
                    return ThumbnailOutcome::Unavailable(ThumbnailUnavailable::SealRefused(format!(
                        "в буфере {} байт, а миниатюре {:?} нужно {}",
                        rgba.len(),
                        plan.size(),
                        plan.byte_len()
                    )));
                }
                ThumbnailOutcome::Ready {
                    size: plan.size(),
                    rgba,
                }
            }
        }
    }
}

impl std::fmt::Debug for ThumbnailSlot {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sealed { plan, .. } => write!(formatter, "ThumbnailSlot::Sealed({:?})", plan.size()),
            Self::Unavailable(reason) => write!(formatter, "ThumbnailSlot::Unavailable({reason})"),
        }
    }
}
