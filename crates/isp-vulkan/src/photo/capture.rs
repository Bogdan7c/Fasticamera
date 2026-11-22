//! Стадия «захват» снимка: сырые отсчёты полезной области кадра камеры → принадлежащий снимку буфер,
//! видимый процессору (u16 little-endian, строка за строкой).
//!
//! # Зачем отдельная быстрая стадия
//! Кадр камеры принадлежит камере: его нужно вернуть как можно скорее (камера ждёт буфер из ограниченного
//! пула). Захват — один дешёвый dispatch (≈ чтение + запись 25 МБ), после fence которого рендер возвращает
//! лизинг, а проявление (дорогое, по полосам) читает отсчёты уже из СВОЕГО буфера.
//!
//! # Порядок
//! 1. Рендер забирает кадр для compute (`MosaicSource::record_acquire_for(.., Compute)`) и зовёт
//!    [`RawCapturer::record_capture`] в тот же командный буфер → [`PendingRawCapture`].
//! 2. Рендер отправляет буфер с fence. После fence: [`RawCapturer::complete_after_fence`] → [`RawCapture`]
//!    и ТОЛЬКО ТЕПЕРЬ возврат кадра камере (`release_after_gpu_fence`).
//! 3. [`RawCapture::samples`] → `RawSamples16` для DNG; [`RawCapture`] же — вход проявления.
//!
//! Одновременно в полёте один захват (общий набор дескрипторов): второй `record_capture` до
//! `complete_after_fence` — `Busy`.

use std::sync::Arc;

use ash::vk;
use camera_contract::geometry::PixelSize;
use gpu_vulkan::compute::{
    BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, DescriptorResource, LocalSize,
    SpecializationConstant, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::host_readback::HostReadbackBuffer;
use gpu_vulkan::one_shot_queue::{CompletedWork, SubmittedWork};
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, ValidArea};
use still_contract::shot::RawSamples16;

use super::config::PhotoConfig;
use super::error::PhotoError;
use super::pixels::{GpuPixels, seal_or_leak};
use crate::quality::input::source_addressing;
use crate::shaders::blobs;

/// `constant_id` размера рабочей группы по X и Y (`local_size_x_id`/`local_size_y_id`).
const LOCAL_SIZE_X_ID: u32 = 0;
const LOCAL_SIZE_Y_ID: u32 = 1;
/// Байт на отсчёт в буфере захвата (u16).
const BYTES_PER_SAMPLE: u64 = 2;
/// Слот набора дескрипторов: захват один за раз.
const SLOT: usize = 0;

/// Push-константы захвата (раскладка — `CaptureGeometry` в `photo_capture_main.glsl`, 20 байт).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CapturePush {
    area: ValidArea,
    row_stride_bytes: u32,
}

impl CapturePush {
    const BYTES: u32 = 20;

    fn to_bytes(self) -> [u8; 20] {
        let words = [
            self.area.left,
            self.area.top,
            self.area.width,
            self.area.height,
            self.row_stride_bytes,
        ];
        let mut bytes = [0_u8; 20];
        for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
            chunk.copy_from_slice(&word.to_ne_bytes());
        }
        bytes
    }
}

/// Захватчик сырых отсчётов: пайплайны (RAW16-картинка и упакованный RAW10) и учёт «захват в полёте».
///
/// Порядок полей = порядок уничтожения: пайплайны раньше контекста.
pub struct RawCapturer {
    raw16: ComputePipeline,
    raw10: ComputePipeline,
    local: LocalSize,
    /// Есть записанный, но не подтверждённый fence-ом захват.
    capture_in_flight: bool,
    /// Буфер, выделенный заранее ([`Self::prepare`]): `record_capture` берёт его, если размер подошёл.
    prepared: Option<HostReadbackBuffer>,
    context: GpuContext,
}

impl std::fmt::Debug for RawCapturer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "RawCapturer(in_flight: {})", self.capture_in_flight)
    }
}

impl RawCapturer {
    /// Создаёт пайплайны захвата. Размер рабочей группы — из конфига и проверяется против лимитов устройства.
    pub fn new(context: &GpuContext, config: &PhotoConfig) -> Result<Self, PhotoError> {
        config.validate()?;
        let local = config.local_size()?;
        local.validate_for_device(context)?;
        let pipeline = |label: &'static str, spirv: &[u8], source: BindingKind| {
            ComputePipeline::new(
                context,
                &ComputePipelineDesc {
                    label,
                    spirv,
                    bindings: &[source, BindingKind::ReadWriteStorageBuffer],
                    push_constant_bytes: CapturePush::BYTES,
                    specialization: &[
                        SpecializationConstant {
                            constant_id: LOCAL_SIZE_X_ID,
                            value: local.x.get(),
                        },
                        SpecializationConstant {
                            constant_id: LOCAL_SIZE_Y_ID,
                            value: local.y.get(),
                        },
                    ],
                    frame_slots: 1,
                },
            )
        };
        Ok(Self {
            raw16: pipeline(
                "isp-photo-capture-raw16",
                blobs::PHOTO_CAPTURE_RAW16_COMP,
                BindingKind::SampledImage,
            )?,
            raw10: pipeline(
                "isp-photo-capture-raw10",
                blobs::PHOTO_CAPTURE_RAW10_COMP,
                BindingKind::ReadOnlyStorageBuffer,
            )?,
            local,
            capture_in_flight: false,
            prepared: None,
            context: context.clone(),
        })
    }

    /// Выделяет буфер под полезную область заранее, ВНЕ пути затвора.
    ///
    /// Буфер 4096×3072 u16 — 24 МиБ: выделение, отображение и обнуление (обнуление нужно, чтобы срез байт
    /// всегда лежал над инициализированной памятью) стоят миллисекунды CPU (на ПК около 2–4 мс), а
    /// `record_capture` вызывается на потоке рендера между кадрами видоискателя. Рендер зовёт `prepare`
    /// заранее (экран камеры показан, снимка ещё нет) и сразу после подтверждения предыдущего захвата;
    /// без `prepare` `record_capture` выделяет буфер сам — медленнее, но верно. Подготовленный буфер другого
    /// размера отбрасывается при следующем `prepare`/`record_capture`.
    pub fn prepare(&mut self, valid_size: PixelSize) -> Result<(), PhotoError> {
        let size_bytes = Self::buffer_bytes(valid_size);
        if self
            .prepared
            .as_ref()
            .is_some_and(|buffer| buffer.size_bytes() == size_bytes)
        {
            return Ok(());
        }
        self.prepared = None;
        self.prepared = Some(HostReadbackBuffer::new(&self.context, size_bytes)?);
        Ok(())
    }

    /// Размер буфера захвата под полезную область `valid_size`, байт (u16 на отсчёт).
    pub fn buffer_bytes(valid_size: PixelSize) -> u64 {
        u64::from(valid_size.width) * u64::from(valid_size.height) * BYTES_PER_SAMPLE
    }

    /// Принимает буфер, выделенный вне захватчика (`HostReadbackBuffer::new` на другом потоке, размер —
    /// [`Self::buffer_bytes`]): так выделение и обнуление 24 МиБ уходят с потока рендера (решение 144).
    /// Прежний подготовленный буфер заменяется; буфер не того размера `record_capture` отбросит сам.
    pub fn install_prepared(&mut self, buffer: HostReadbackBuffer) {
        self.prepared = Some(buffer);
    }

    /// Занят ли захватчик заранее выделенным буфером (диагностика и тесты).
    pub fn has_prepared_buffer(&self) -> bool {
        self.prepared.is_some()
    }

    /// Записывает захват полезной области `valid_area` кадра `source` в командный буфер.
    ///
    /// Предусловия вызывающего: кадр уже забран для compute (`record_acquire_for(.., Compute)`), командный
    /// буфер в состоянии записи. Все проверки — до первой команды: при ошибке в буфер ничего не записано.
    /// Полезная область: чётные границы и размер ≥ 2×2 (ячейка CFA 2×2), целиком внутри кадра.
    pub fn record_capture(
        &mut self,
        command_buffer: vk::CommandBuffer,
        source: &MosaicSource,
        valid_area: ValidArea,
    ) -> Result<PendingRawCapture, PhotoError> {
        if self.capture_in_flight {
            return Err(PhotoError::Busy {
                what: "предыдущий захват не подтверждён fence-ом (complete_after_fence)",
            });
        }
        if valid_area.width < 2
            || valid_area.height < 2
            || !valid_area.width.is_multiple_of(2)
            || !valid_area.height.is_multiple_of(2)
        {
            return Err(PhotoError::InvalidInput {
                what: format!("полезная область {valid_area:?}: размер должен быть чётным и не меньше 2x2"),
            });
        }
        // Границы, чётность угла, влезание в кадр и в буфер RAW10 — общая проверка стенда V4.
        let extent = vk::Extent2D {
            width: valid_area.width,
            height: valid_area.height,
        };
        let addressing = source_addressing(source, valid_area, extent)?;
        let size_bytes = Self::buffer_bytes(PixelSize::new(valid_area.width, valid_area.height));
        // Заранее выделенный буфер (если подошёл по размеру) избавляет путь затвора от выделения и обнуления.
        let destination = match self.prepared.take() {
            Some(buffer) if buffer.size_bytes() == size_bytes => buffer,
            _ => HostReadbackBuffer::new(&self.context, size_bytes)?,
        };

        let pipeline = match source {
            MosaicSource::SampledR16 { .. } => &mut self.raw16,
            MosaicSource::PackedRaw10 { .. } => &mut self.raw10,
        };
        let bound = pipeline.bind(
            SLOT,
            &[
                BindingWrite {
                    binding: 0,
                    resource: source.compute_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: destination.read_write_binding(),
                },
            ],
        );
        let push = CapturePush {
            area: valid_area,
            row_stride_bytes: addressing.row_stride_bytes,
        };
        // Одна нить = два отсчёта (одно слово): по X вдвое меньше нитей.
        let groups = WorkgroupCount::covering(valid_area.width / 2, valid_area.height, self.local);
        let recorded = bound
            .and_then(|()| pipeline.check_dispatch(SLOT, CapturePush::BYTES as usize, groups))
            .and_then(|()| pipeline.record_dispatch(command_buffer, SLOT, &push.to_bytes(), groups));
        if let Err(error) = recorded {
            // Буфер назначения сейчас уничтожится, а набор запомнил его хэндл: без сброса запомненного
            // следующий захват с тем же значением хэндла пропустил бы запись и читал бы чужую память.
            // Набор ни в одном отправленном буфере не использовался, поэтому освободить его можно сразу.
            pipeline.release_slot_after_fence(SLOT)?;
            return Err(error.into());
        }
        // Записанное шейдером должно стать видимым CPU после fence (DNG читает байты сразу).
        destination.record_compute_write_to_host_read(command_buffer);
        self.capture_in_flight = true;
        Ok(PendingRawCapture {
            buffer: Some(destination),
            valid_area,
            raw10: matches!(source, MosaicSource::PackedRaw10 { .. }),
        })
    }

    /// Подтверждает: сабмит с захватом завершён (`proof` — `OneShotSubmitter::poll → Completed` именно его).
    /// Запечатывает буфер и освобождает набор дескрипторов
    /// (следующий захват может привязать другой кадр). ТОЛЬКО после этого кадр можно вернуть камере.
    ///
    /// `pending` обязан быть от ЭТОГО захватчика (иначе `OutOfOrder`).
    pub fn complete_after_fence(
        &mut self,
        mut pending: PendingRawCapture,
        proof: &CompletedWork,
    ) -> Result<RawCapture, PhotoError> {
        if !self.capture_in_flight {
            return Err(PhotoError::OutOfOrder {
                what: "complete_after_fence без записанного захвата (чужой PendingRawCapture?)",
            });
        }
        let Some(buffer) = pending.buffer.take() else {
            return Err(PhotoError::OutOfOrder {
                what: "захват уже подтверждён",
            });
        };
        let pipeline = if pending.raw10 { &self.raw10 } else { &self.raw16 };
        pipeline.release_slot_after_fence(SLOT)?;
        self.capture_in_flight = false;
        Ok(RawCapture {
            storage: Arc::new(GpuPixels::new(seal_or_leak(buffer, proof)?)),
            valid_area: pending.valid_area,
        })
    }

    /// Отменяет записанный, но НЕ отправленный на GPU захват (ошибка после `record_capture`, командный буфер
    /// выброшен). Буфер освобождается сразу — безопасно, потому что GPU его не видел; набор дескрипторов
    /// освобождается тоже (его привязка жила только в выброшенном командном буфере).
    pub fn discard_unsubmitted(&mut self, mut pending: PendingRawCapture) -> Result<(), PhotoError> {
        if pending.buffer.take().is_none() {
            return Ok(());
        }
        // Набор дескрипторов помечен «привязан в командный буфер»; буфер выброшен, значит, набор свободен.
        let pipeline = if pending.raw10 { &self.raw10 } else { &self.raw16 };
        pipeline.release_slot_after_fence(SLOT)?;
        self.capture_in_flight = false;
        Ok(())
    }
}

/// Захват записан в командный буфер, но fence ещё не подтверждён: буфер пока пишет GPU.
///
/// Выброшенный без `complete_after_fence`/`discard_unsubmitted` — буфер ТЕРЯЕТСЯ намеренно (лучше утечка,
/// чем уничтожение памяти, в которую ещё может писать GPU; соглашение крейта, `conventions` → «GPU»).
pub struct PendingRawCapture {
    buffer: Option<HostReadbackBuffer>,
    valid_area: ValidArea,
    raw10: bool,
}

impl std::fmt::Debug for PendingRawCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "PendingRawCapture({:?})", self.valid_area)
    }
}

impl PendingRawCapture {
    /// Сообщает захвату, какой отправленной работой записан его командный буфер (сразу после
    /// `OneShotSubmitter::submit`). Без этого [`RawCapturer::complete_after_fence`] откажется запечатать буфер:
    /// доказательство нечем связать с записавшей работой.
    pub fn note_submitted(&mut self, work: &SubmittedWork) {
        if let Some(buffer) = self.buffer.as_mut() {
            buffer.mark_written_by(work);
        }
    }

    /// Полезная область, которую захватывает этот захват.
    pub fn valid_area(&self) -> ValidArea {
        self.valid_area
    }
}

impl Drop for PendingRawCapture {
    fn drop(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            tracing::error!(
                target: "fasticamera::isp",
                "захват снимка выброшен без подтверждения fence: буфер намеренно утекает ({buffer:?})"
            );
            std::mem::forget(buffer);
        }
    }
}

/// Готовые сырые отсчёты снимка: u16 little-endian по полезной области кадра, строка за строкой.
/// Дёшево клонируется (общий владелец буфера); буфер живёт, пока жив последний клон.
#[derive(Clone)]
pub struct RawCapture {
    storage: Arc<GpuPixels>,
    valid_area: ValidArea,
}

impl std::fmt::Debug for RawCapture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "RawCapture({:?})", self.valid_area)
    }
}

impl RawCapture {
    /// Полезная область в координатах буфера кадра камеры (откуда сняты отсчёты).
    pub fn valid_area(&self) -> ValidArea {
        self.valid_area
    }

    /// Размер отсчётов (ширина × высота полезной области).
    pub fn size(&self) -> PixelSize {
        PixelSize::new(self.valid_area.width, self.valid_area.height)
    }

    /// Отсчёты для DNG: байты — прямо над памятью GPU-буфера (без копирования), буфер держит `Arc`.
    pub fn samples(&self) -> Result<RawSamples16, PhotoError> {
        RawSamples16::new(self.size(), self.storage.clone()).map_err(|mismatch| PhotoError::InvalidInput {
            what: mismatch.to_string(),
        })
    }

    /// Привязка «только чтение» для шейдера проявления.
    pub(crate) fn read_only_binding(&self) -> DescriptorResource {
        self.storage.sealed().read_only_binding()
    }
}
