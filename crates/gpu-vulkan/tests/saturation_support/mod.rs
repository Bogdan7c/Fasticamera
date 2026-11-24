//! Работа-заглушка для тестов стенда насыщения: compute-проход «узор → storage-картинка» известной цены.
//!
//! Цена кадра растёт с площадью картинки (на каждый пиксель — запись в картинку и атомарный инкремент общего
//! счётчика). Счётчик НЕ обнуляется между кадрами, поэтому после прогона `счётчик == кадры × пиксели`:
//! это доказательство, что кадры реально исполнились на GPU, а не только были «отправлены».
//! Синий канал картинки несёт номер последнего исполненного кадра (`кадр % 64 / 64`).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    dead_code,
    reason = "стенд тестов: падение — сигнал о провале; не каждый тестовый бинарь берёт все хелперы"
)]

use ash::vk;
use gpu_vulkan::compute::{
    BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, SpecializationConstant, WorkgroupCount,
};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::device_buffer::record_compute_pass_dependency;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::read_host_buffer;
use gpu_vulkan::saturation::{FrameRecording, SaturationWorkload, SlotId};
use gpu_vulkan::shaders::blobs;
use gpu_vulkan::storage_image::{
    StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImagePreviousUse, StorageImageUsage,
};

/// Режим узора «градиент координат» тестового шейдера (`constant_id = 2`).
const MODE_GRADIENT: u32 = 1;
/// Период номера кадра в синем канале.
pub const BLUE_PERIOD: u64 = 64;

/// Работа-заглушка + журнал нарушений контракта (порядок вызовов стенда).
pub struct PatternWorkload {
    context: GpuContext,
    pipeline: ComputePipeline,
    image: StorageImage,
    colors: HostBuffer,
    /// Счётчик записанных пикселей за ВСЕ исполненные кадры.
    counter: HostBuffer,
    local: LocalSize,
    pixels_per_frame: u64,
    /// Кадр, который сейчас в полёте в каждом слоте (`None` — слот свободен с точки зрения работы).
    frame_in_slot: Vec<Option<u64>>,
    /// Номер кадра, на котором `record_frame` должен вернуть ошибку.
    fail_at_frame: Option<u64>,
    /// Сколько раз вызывали `record_frame` успешно (= отправлено кадров).
    pub recorded_frames: u64,
    /// Сколько раз вызвали `release_slot_after_fence`.
    pub released_frames: u64,
    /// Кадры в порядке уведомлений о завершении.
    pub release_order: Vec<u64>,
    /// Нарушения контракта стендом (пусто у корректного стенда).
    pub violations: Vec<String>,
}

impl PatternWorkload {
    /// `side` — сторона квадратной картинки; `frames_in_flight` — сколько слотов у стенда.
    pub fn new(context: &GpuContext, side: u32, frames_in_flight: usize) -> Self {
        let local = LocalSize::new(8, 4).unwrap();
        let mut pipeline = ComputePipeline::new(
            context,
            &ComputePipelineDesc {
                label: "saturation-test-pattern",
                spirv: blobs::KIT_TEST_PATTERN_RGBA16F_COMP,
                bindings: &[
                    BindingKind::StorageImageWrite,
                    BindingKind::UniformBuffer,
                    BindingKind::ReadWriteStorageBuffer,
                ],
                push_constant_bytes: 12,
                specialization: &[
                    SpecializationConstant {
                        constant_id: 0,
                        value: local.x.get(),
                    },
                    SpecializationConstant {
                        constant_id: 1,
                        value: local.y.get(),
                    },
                    SpecializationConstant {
                        constant_id: 2,
                        value: MODE_GRADIENT,
                    },
                ],
                frame_slots: frames_in_flight,
            },
        )
        .unwrap();
        let image = StorageImage::new(
            context,
            &StorageImageDesc {
                extent: vk::Extent2D {
                    width: side,
                    height: side,
                },
                format_policy: StorageFormatPolicy::HalfFloatOnly,
                usage: StorageImageUsage::ComputeWriteSampledRead,
            },
        )
        .unwrap();
        // Цвета четвертей шейдеру нужны для привязки, в градиентном режиме не используются.
        let colors = HostBuffer::new(context, 64, HostBufferUsage::UniformParameters).unwrap();
        let mut counter = HostBuffer::new(context, 4, HostBufferUsage::ReadWriteStorage).unwrap();
        counter.write_bytes(0, &0_u32.to_ne_bytes()).unwrap();
        for slot in 0..frames_in_flight {
            pipeline
                .bind(
                    slot,
                    &[
                        BindingWrite {
                            binding: 0,
                            resource: image.storage_binding(),
                        },
                        BindingWrite {
                            binding: 1,
                            resource: colors.binding(),
                        },
                        BindingWrite {
                            binding: 2,
                            resource: counter.binding(),
                        },
                    ],
                )
                .unwrap();
        }
        Self {
            context: context.clone(),
            pipeline,
            image,
            colors,
            counter,
            local,
            pixels_per_frame: u64::from(side) * u64::from(side),
            frame_in_slot: vec![None; frames_in_flight],
            fail_at_frame: None,
            recorded_frames: 0,
            released_frames: 0,
            release_order: Vec::new(),
            violations: Vec::new(),
        }
    }

    /// `record_frame` вернёт ошибку на кадре с этим номером (кадры до него записываются нормально).
    pub fn failing_at_frame(mut self, frame_index: u64) -> Self {
        self.fail_at_frame = Some(frame_index);
        self
    }

    pub fn pixels_per_frame(&self) -> u64 {
        self.pixels_per_frame
    }

    /// Счётчик пикселей, как его сейчас видит CPU (по модулю 2^32).
    pub fn counter_value(&self) -> u32 {
        let bytes = read_host_buffer(&self.counter).unwrap();
        u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }

    /// Картинка заглушки (для readback после прогона).
    pub fn image(&self) -> &StorageImage {
        &self.image
    }

    /// Раскладка картинки после последнего кадра (для `read_storage_image_rgba`).
    pub fn image_use_after_frames(&self) -> StorageImagePreviousUse {
        StorageImagePreviousUse::ComputeWrite
    }

    /// Ожидаемый синий канал после кадра с номером `frame_index`.
    pub fn expected_blue(frame_index: u64) -> f32 {
        (frame_index % BLUE_PERIOD) as f32 / BLUE_PERIOD as f32
    }

    fn push_constants(&self, frame_index: u64) -> Vec<u8> {
        let extent = self.image.extent();
        let mut bytes: Vec<u8> = [extent.width, extent.height]
            .iter()
            .flat_map(|word| word.to_ne_bytes())
            .collect();
        bytes.extend(Self::expected_blue(frame_index).to_ne_bytes());
        bytes
    }
}

impl SaturationWorkload for PatternWorkload {
    fn record_frame(&mut self, recording: FrameRecording) -> Result<(), GpuError> {
        let slot = recording.slot().index();
        let frame_index = recording.frame_index();
        if let Some(busy_frame) = self.frame_in_slot[slot] {
            self.violations.push(format!(
                "record_frame(слот {slot}, кадр {frame_index}): слот ещё занят кадром {busy_frame} (нет release)"
            ));
        }
        if self.fail_at_frame == Some(frame_index) {
            return Err(GpuError::InvalidState {
                what: format!("заглушка: намеренная ошибка записи на кадре {frame_index}"),
            });
        }
        let command_buffer = recording.command_buffer();
        // Барьер «все прошлые compute-записи (в т.ч. прошлых сабмитов) → этот кадр»: счётчик и картинка общие.
        record_compute_pass_dependency(&self.context, command_buffer);
        let previous = if frame_index == 0 {
            StorageImagePreviousUse::NotUsedYet
        } else {
            StorageImagePreviousUse::ComputeWrite
        };
        self.image.record_prepare_for_compute_write(command_buffer, previous);
        let extent = self.image.extent();
        self.pipeline.record_dispatch(
            command_buffer,
            slot,
            &self.push_constants(frame_index),
            WorkgroupCount::covering(extent.width, extent.height, self.local),
        )?;
        // Счётчик должен быть виден CPU к моменту release (проверка «release только после исполнения»).
        self.counter.record_compute_write_to_host_read(command_buffer);
        self.frame_in_slot[slot] = Some(frame_index);
        self.recorded_frames += 1;
        Ok(())
    }

    fn release_slot_after_fence(&mut self, slot: SlotId) {
        let Some(frame_index) = self.frame_in_slot[slot.index()].take() else {
            self.violations
                .push(format!("release для свободного слота {}", slot.index()));
            return;
        };
        self.released_frames += 1;
        self.release_order.push(frame_index);
        // GPU исполняет кадры по порядку: к сигналу fence кадра N счётчик обязан включать кадры 0..=N.
        // Сравнение по модулю 2^32 (счётчик u32 может переполниться).
        let required = ((frame_index + 1) * self.pixels_per_frame) as u32;
        let ahead = self.counter_value().wrapping_sub(required);
        if ahead >= u32::MAX / 2 {
            self.violations.push(format!(
                "release кадра {frame_index} до исполнения его на GPU (счётчик {} < {required})",
                self.counter_value()
            ));
        }
    }
}
