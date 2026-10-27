//! [`FrameSink`] — нейтральный «приёмник кадра» (задел V5 README): куда уходит обработанный кадр.
//!
//! Производитель (ISP полного кадра, стенд V4) не знает, кто потребитель: картинка для экрана, вход
//! кодера или кольцо кадров. Он спрашивает у приёмника цель записи, пишет её compute-шейдером и
//! отдаёт кадр обратно с меткой. Всё, что зависит от потребителя (откуда картинка, какие семафоры,
//! что делать после сабмита), спрятано за трейтом.
//!
//! # Жизненный цикл кадра (одна очередь, один слот кадра)
//! 1. [`FrameSink::begin_frame`] — пишет в командный буфер подготовку цели к compute-записи и выдаёт
//!    цель ([`FrameSinkGrant::Ready`]) или честно сообщает, что свободной цели нет
//!    ([`FrameSinkGrant::Busy`], обратное давление: кодер держит все входные буферы). При `Busy`
//!    приёмник ничего не записал, кадр пропускается вызывающим со счётчиком.
//! 2. Производитель записывает dispatch-и в цель (`FrameSinkTarget::storage`, раскладка `GENERAL`).
//! 3. [`FrameSink::finish_frame`] — пишет передачу записанного потребителю и запоминает метку кадра.
//! 4. Вызывающий отправляет командный буфер, добавив [`FrameSink::submit_dependencies`] (семафоры
//!    приёмника), и после отправки зовёт [`FrameSink::after_submit`] (публикация кадра потребителю).
//!
//! # Реализации
//! * [`OffscreenFrameSink`] (сейчас) — storage-картинки по слоту кадра; потребитель читает их на той же
//!   очереди (стенд V4, readback в тестах). Семафоров нет, публикация — только учёт.
//! * **Входная surface кодера `MediaCodec` (план `video`)** — проверено в сессии 00: `VkSurface` на
//!   `createInputSurface()` кодера `c2.qti.hevc.encoder.hdr`, swapchain 4K60 10 бит
//!   (`A2B10G10R10_UNORM_PACK32`, цветовое пространство HLG/PQ), PTS через `VK_GOOGLE_display_timing`.
//!   Реализация трейта поверх `swapchain`:
//!   - `begin_frame` — `vkAcquireNextImageKHR` с таймаутом 0: нет картинки → `Busy` (обратное давление
//!     кодера, а не ожидание на потоке рендера); есть → барьер `UNDEFINED → GENERAL` для compute-записи
//!     (нужен `VK_IMAGE_USAGE_STORAGE_BIT` в `supportedUsageFlags` surface; если его нет — запись
//!     фрагментным проходом, контракт трейта тот же);
//!   - `finish_frame` — барьер `GENERAL → PRESENT_SRC_KHR` и запоминание PTS = `presentation_time_ns`
//!     (время сенсора, решение V2);
//!   - `submit_dependencies` — ожидание семафора acquire на стадии compute и сигнал семафора present;
//!   - `after_submit` — `vkQueuePresentKHR` с `VkPresentTimeGOOGLE` = PTS.
//!
//!   Производителю (ISP) при этом ничего менять не придётся — в этом и смысл V5.

use ash::vk;

use crate::compute::DescriptorResource;
use crate::context::GpuContext;
use crate::error::GpuError;
use crate::hardware_buffer_cache::queue_handoff::ConsumerStage;
use crate::storage_image::{
    StorageFormatPolicy, StorageImage, StorageImageDesc, StorageImageFormat, StorageImagePreviousUse, StorageImageUsage,
};

/// Что за цели у приёмника: размер и формат (по формату производитель выбирает вариант шейдера).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSinkDescription {
    /// Размер цели, пиксели.
    pub extent: vk::Extent2D,
    /// Формат цели (квалификатор формата шейдера обязан совпасть).
    pub format: StorageImageFormat,
}

/// Цель записи одного кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSinkTarget {
    /// Ресурс для привязки `StorageImageWrite` (раскладка `GENERAL`).
    pub storage: DescriptorResource,
    /// Размер и формат цели.
    pub description: FrameSinkDescription,
}

/// Ответ приёмника на начало кадра.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSinkGrant {
    /// Цель выдана, подготовка к записи уже записана в командный буфер.
    Ready(FrameSinkTarget),
    /// Свободной цели нет (обратное давление потребителя); в командный буфер ничего не записано.
    Busy,
}

/// Метка кадра для потребителя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameSinkStamp {
    /// Монотонный номер кадра у производителя.
    pub sequence: u64,
    /// Время кадра на шкале источника, нс (для кодера — PTS; для камеры — время сенсора).
    pub presentation_time_ns: u64,
}

/// Семафоры, которые вызывающий добавляет в сабмит кадра (`VkSubmitInfo2`).
#[derive(Debug, Clone, Default)]
pub struct FrameSinkSubmitDependencies {
    /// Ждать перед работой кадра (например, семафор acquire картинки кодера).
    pub wait: Vec<vk::SemaphoreSubmitInfo<'static>>,
    /// Сигналить после работы кадра (например, семафор для present в кодер).
    pub signal: Vec<vk::SemaphoreSubmitInfo<'static>>,
}

impl FrameSinkSubmitDependencies {
    /// Нет ни ожиданий, ни сигналов (приёмник на той же очереди).
    pub fn is_empty(&self) -> bool {
        self.wait.is_empty() && self.signal.is_empty()
    }
}

/// Итог публикации кадра после сабмита.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameSinkPublish {
    /// Кадр передан потребителю.
    Published(FrameSinkStamp),
    /// В этом слоте нет законченного и ещё не опубликованного кадра (нечего публиковать).
    NothingToPublish,
}

/// Приёмник обработанного кадра (см. модуль). Объектно-безопасен: производитель хранит `&mut dyn FrameSink`.
pub trait FrameSink {
    /// Размер и формат целей.
    fn description(&self) -> FrameSinkDescription;

    /// Начинает кадр в слоте `frame_slot`: подготовка цели к compute-записи или `Busy`.
    /// Повторный `begin_frame` без `finish_frame` — `InvalidState`.
    fn begin_frame(&mut self, command_buffer: vk::CommandBuffer, frame_slot: usize)
    -> Result<FrameSinkGrant, GpuError>;

    /// Заканчивает кадр: передача записанного потребителю + метка. Без `begin_frame` — `InvalidState`.
    fn finish_frame(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        stamp: FrameSinkStamp,
    ) -> Result<(), GpuError>;

    /// Отменяет начатый кадр слота: производитель не смог дописать его после `begin_frame` (сбой проверки,
    /// которую нельзя сделать до выдачи цели). Приёмник возвращает слот в состояние ДО `begin_frame`.
    /// Командный буфер, в который записан `begin_frame`, вызывающий НЕ отправляет (сбрасывает): там
    /// подготовка цели без записи. Без начатого кадра — `InvalidState`.
    fn abort_frame(&mut self, frame_slot: usize) -> Result<(), GpuError>;

    /// Семафоры, которые нужно добавить в сабмит кадра слота.
    fn submit_dependencies(&self, frame_slot: usize) -> Result<FrameSinkSubmitDependencies, GpuError>;

    /// Публикует кадр слота после успешной отправки его командного буфера.
    fn after_submit(&mut self, frame_slot: usize) -> Result<FrameSinkPublish, GpuError>;
}

/// Описание [`OffscreenFrameSink`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffscreenFrameSinkDesc {
    /// Размер целей.
    pub extent: vk::Extent2D,
    /// Предпочтение формата.
    pub format_policy: StorageFormatPolicy,
    /// Число слотов кадров = число картинок (кадр k читают, пока пишется кадр k+1).
    pub frame_slots: usize,
    /// Кто читает законченный кадр (стадия для барьера передачи).
    pub reader: ConsumerStage,
}

/// Состояние картинки слота.
#[derive(Debug, Clone, Copy)]
struct OffscreenSlot {
    /// Последнее записанное в командный буфер использование (для барьера следующей записи).
    last_use: StorageImagePreviousUse,
    /// Кадр начат и ещё не закончен.
    in_frame: bool,
    /// Использование до `begin_frame` начатого кадра (для `abort_frame`: буфер с подготовкой не отправлен).
    use_before_frame: StorageImagePreviousUse,
    /// Метка последнего законченного кадра.
    finished: Option<FrameSinkStamp>,
    /// Законченный кадр уже опубликован.
    published: bool,
}

/// Приёмник в память GPU: storage-картинка на слот кадра.
///
/// Состояние картинки (`last_use`) обновляется при ЗАПИСИ команд: если командный буфер потом не
/// отправлен, вызывающий обязан пересоздать приёмник (то же правило, что у кольца кадров).
pub struct OffscreenFrameSink {
    images: Vec<StorageImage>,
    slots: Vec<OffscreenSlot>,
    reader: ConsumerStage,
    description: FrameSinkDescription,
    published_frames: u64,
}

impl std::fmt::Debug for OffscreenFrameSink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "OffscreenFrameSink({:?}, {} слотов)",
            self.description,
            self.images.len()
        )
    }
}

impl OffscreenFrameSink {
    /// Создаёт картинки. Ноль слотов — `InvalidArgument`; размер и формат проверяет `StorageImage`.
    pub fn new(context: &GpuContext, desc: &OffscreenFrameSinkDesc) -> Result<Self, GpuError> {
        if desc.frame_slots == 0 {
            return Err(GpuError::InvalidArgument {
                what: "OffscreenFrameSink без слотов".to_owned(),
            });
        }
        let image_desc = StorageImageDesc {
            extent: desc.extent,
            format_policy: desc.format_policy,
            usage: StorageImageUsage::ComputeWriteSampledRead,
        };
        let images = (0..desc.frame_slots)
            .map(|_| StorageImage::new(context, &image_desc))
            .collect::<Result<Vec<_>, _>>()?;
        // Все картинки создаются с одной политикой на одном устройстве — формат у них общий.
        let format = images
            .first()
            .map(StorageImage::format)
            .ok_or_else(|| GpuError::InvalidState {
                what: "OffscreenFrameSink: картинки не созданы".to_owned(),
            })?;
        let idle = OffscreenSlot {
            last_use: StorageImagePreviousUse::NotUsedYet,
            in_frame: false,
            use_before_frame: StorageImagePreviousUse::NotUsedYet,
            finished: None,
            published: false,
        };
        Ok(Self {
            slots: vec![idle; images.len()],
            images,
            reader: desc.reader,
            description: FrameSinkDescription {
                extent: desc.extent,
                format,
            },
            published_frames: 0,
        })
    }

    /// Картинка слота (для чтения потребителем на той же очереди или readback в тестах).
    pub fn image(&self, frame_slot: usize) -> Option<&StorageImage> {
        self.images.get(frame_slot)
    }

    /// Последнее записанное использование картинки слота (нужно readback-у, чтобы вернуть раскладку).
    pub fn last_use(&self, frame_slot: usize) -> Option<StorageImagePreviousUse> {
        self.slots.get(frame_slot).map(|slot| slot.last_use)
    }

    /// Метка последнего законченного кадра слота.
    pub fn finished_stamp(&self, frame_slot: usize) -> Option<FrameSinkStamp> {
        self.slots.get(frame_slot).and_then(|slot| slot.finished)
    }

    /// Сколько кадров опубликовано за всё время.
    pub fn published_frames(&self) -> u64 {
        self.published_frames
    }

    fn slot_index(&self, frame_slot: usize) -> Result<usize, GpuError> {
        if frame_slot < self.slots.len() {
            Ok(frame_slot)
        } else {
            Err(GpuError::InvalidArgument {
                what: format!("слот приёмника {frame_slot} вне 0..{}", self.slots.len()),
            })
        }
    }
}

impl FrameSink for OffscreenFrameSink {
    fn description(&self) -> FrameSinkDescription {
        self.description
    }

    fn begin_frame(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
    ) -> Result<FrameSinkGrant, GpuError> {
        let index = self.slot_index(frame_slot)?;
        let slot = self.slots[index];
        if slot.in_frame {
            return Err(GpuError::InvalidState {
                what: format!("кадр слота {frame_slot} уже начат и не закончен"),
            });
        }
        let image = &self.images[index];
        // Чтения прошлого кадра этой картинки (той же очереди) — до новой записи (WAR, см. transitions).
        image.record_prepare_for_compute_write(command_buffer, slot.last_use);
        self.slots[index] = OffscreenSlot {
            last_use: StorageImagePreviousUse::ComputeWrite,
            in_frame: true,
            use_before_frame: slot.last_use,
            finished: slot.finished,
            published: slot.published,
        };
        Ok(FrameSinkGrant::Ready(FrameSinkTarget {
            storage: image.storage_binding(),
            description: self.description,
        }))
    }

    fn finish_frame(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        stamp: FrameSinkStamp,
    ) -> Result<(), GpuError> {
        let index = self.slot_index(frame_slot)?;
        if !self.slots[index].in_frame {
            return Err(GpuError::InvalidState {
                what: format!("finish_frame слота {frame_slot} без begin_frame"),
            });
        }
        self.images[index].record_compute_write_to_sampled_read(command_buffer, self.reader);
        self.slots[index] = OffscreenSlot {
            last_use: StorageImagePreviousUse::SampledRead(self.reader),
            in_frame: false,
            use_before_frame: StorageImagePreviousUse::SampledRead(self.reader),
            finished: Some(stamp),
            published: false,
        };
        Ok(())
    }

    fn abort_frame(&mut self, frame_slot: usize) -> Result<(), GpuError> {
        let index = self.slot_index(frame_slot)?;
        let slot = self.slots[index];
        if !slot.in_frame {
            return Err(GpuError::InvalidState {
                what: format!("abort_frame слота {frame_slot} без begin_frame"),
            });
        }
        // Буфер с подготовкой не отправляется (контракт): картинка осталась в прежнем использовании.
        self.slots[index] = OffscreenSlot {
            last_use: slot.use_before_frame,
            in_frame: false,
            ..slot
        };
        Ok(())
    }

    fn submit_dependencies(&self, frame_slot: usize) -> Result<FrameSinkSubmitDependencies, GpuError> {
        self.slot_index(frame_slot)?;
        // Потребитель на той же очереди: порядок задают барьеры, семафоры не нужны.
        Ok(FrameSinkSubmitDependencies::default())
    }

    fn after_submit(&mut self, frame_slot: usize) -> Result<FrameSinkPublish, GpuError> {
        let index = self.slot_index(frame_slot)?;
        let slot = self.slots[index];
        match slot.finished {
            Some(stamp) if !slot.in_frame && !slot.published => {
                self.slots[index].published = true;
                self.published_frames += 1;
                Ok(FrameSinkPublish::Published(stamp))
            }
            _ => Ok(FrameSinkPublish::NothingToPublish),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_support::gpu_context_or_skip;
    use crate::live_objects::LiveObjectKind;

    fn desc(frame_slots: usize) -> OffscreenFrameSinkDesc {
        OffscreenFrameSinkDesc {
            extent: vk::Extent2D { width: 8, height: 4 },
            format_policy: StorageFormatPolicy::HalfFloatOnly,
            frame_slots,
            reader: ConsumerStage::Compute,
        }
    }

    const STAMP: FrameSinkStamp = FrameSinkStamp {
        sequence: 7,
        presentation_time_ns: 16_666_667,
    };

    #[test]
    fn frame_lifecycle_enforces_order_and_publishes_once() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        {
            let mut sink = OffscreenFrameSink::new(&context, &desc(2)).unwrap();
            assert_eq!(
                context.live_objects().count(LiveObjectKind::Image),
                baseline.count(LiveObjectKind::Image) + 2
            );
            assert_eq!(sink.description().format, StorageImageFormat::Rgba16Float);
            assert_eq!(sink.after_submit(0).unwrap(), FrameSinkPublish::NothingToPublish);
            context
                .run_one_shot(|command_buffer| {
                    // finish без begin — ошибка, состояние не меняется.
                    assert!(matches!(
                        sink.finish_frame(command_buffer, 0, STAMP),
                        Err(GpuError::InvalidState { .. })
                    ));
                    let grant = sink.begin_frame(command_buffer, 0)?;
                    let FrameSinkGrant::Ready(target) = grant else {
                        panic!("offscreen-приёмник не бывает занят: {grant:?}");
                    };
                    assert_eq!(target.storage, sink.image(0).unwrap().storage_binding());
                    assert!(matches!(
                        sink.begin_frame(command_buffer, 0),
                        Err(GpuError::InvalidState { .. })
                    ));
                    sink.finish_frame(command_buffer, 0, STAMP)
                })
                .unwrap();
            assert!(sink.submit_dependencies(0).unwrap().is_empty());
            assert_eq!(
                sink.last_use(0),
                Some(StorageImagePreviousUse::SampledRead(ConsumerStage::Compute))
            );
            assert_eq!(
                sink.last_use(1),
                Some(StorageImagePreviousUse::NotUsedYet),
                "другой слот не тронут"
            );
            assert_eq!(sink.finished_stamp(0), Some(STAMP));
            assert_eq!(sink.after_submit(0).unwrap(), FrameSinkPublish::Published(STAMP));
            assert_eq!(
                sink.after_submit(0).unwrap(),
                FrameSinkPublish::NothingToPublish,
                "второй раз — нечего"
            );
            assert_eq!(sink.published_frames(), 1);
            assert!(sink.after_submit(2).is_err(), "слот вне кольца");
        }
        context.wait_idle().unwrap();
        assert_eq!(context.live_objects(), baseline);
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }

    #[test]
    fn aborted_frame_restores_the_slot_and_the_sink_stays_usable() {
        let Some(context) = gpu_context_or_skip() else { return };
        let mut sink = OffscreenFrameSink::new(&context, &desc(1)).unwrap();
        assert!(
            matches!(sink.abort_frame(0), Err(GpuError::InvalidState { .. })),
            "нечего отменять"
        );
        // Кадр начат в буфере, который НЕ отправляется (контракт `abort_frame`).
        let mut discarded = None;
        let _ = context.run_one_shot(|command_buffer| {
            discarded = Some(sink.begin_frame(command_buffer, 0)?);
            Err(GpuError::InvalidState {
                what: "тест: буфер выбрасывается, не отправляется".to_owned(),
            })
        });
        assert!(matches!(discarded, Some(FrameSinkGrant::Ready(_))));
        sink.abort_frame(0).unwrap();
        assert_eq!(
            sink.last_use(0),
            Some(StorageImagePreviousUse::NotUsedYet),
            "как до begin_frame"
        );
        assert_eq!(sink.finished_stamp(0), None);
        assert_eq!(sink.after_submit(0).unwrap(), FrameSinkPublish::NothingToPublish);
        // Приёмник не застрял «в кадре»: следующий кадр проходит целиком.
        context
            .run_one_shot(|command_buffer| {
                sink.begin_frame(command_buffer, 0)?;
                sink.finish_frame(command_buffer, 0, STAMP)
            })
            .unwrap();
        assert_eq!(sink.after_submit(0).unwrap(), FrameSinkPublish::Published(STAMP));
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }

    #[test]
    fn zero_slots_are_rejected() {
        let Some(context) = gpu_context_or_skip() else { return };
        assert!(matches!(
            OffscreenFrameSink::new(&context, &desc(0)),
            Err(GpuError::InvalidArgument { .. })
        ));
    }
}
