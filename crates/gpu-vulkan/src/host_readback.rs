//! [`HostReadbackBuffer`] и [`SealedHostBytes`] — буфер в памяти, видимой CPU, который пишет GPU, а потом
//! читает CPU БЕЗ копирования (сессия 14, пакет P3: проявление снимка).
//!
//! # Зачем отдельный тип, а не `HostBuffer`
//! `HostBuffer::read_bytes` копирует: для снимка 4096×3072 это лишние 25 МБ (сырые отсчёты для DNG)
//! или 50 МБ (RGBA для JPEG). Потребителям нужен срез `&[u8]` прямо над отображённой памятью
//! (`still_contract::shot::PixelStorage`). Безопасно выдать такой срез можно, только когда GPU
//! гарантированно закончил писать, — поэтому состояний два (типы, а не флаг):
//!
//! ```text
//! HostReadbackBuffer  ──(poll → Completed)──▶  seal(&CompletedWork)  ──▶  SealedHostBytes
//!   GPU пишет, CPU НЕ читает                                CPU читает срез, GPU только читает
//! ```
//!
//! * `HostReadbackBuffer` — стадия записи: привязка «чтение-запись» для compute-шейдера, барьеры
//!   «запись GPU → чтение хоста», приёмник копирования из картинки. Среза байт у него НЕТ.
//! * [`SealedHostBytes`] — стадия чтения: срез [`SealedHostBytes::bytes`] и привязка «только чтение»
//!   (проявление читает те же сырые отсчёты шейдером). Записать в него уже нельзя — типы не дают.
//!
//! # Когерентность
//! Память `HOST_VISIBLE | HOST_COHERENT` (как у [`crate::host_buffer::HostBuffer`]): ручные flush и
//! invalidate не нужны. Порядок GPU → CPU: шейдер или копирование пишет, затем барьер в
//! `HOST`/`HOST_READ` (`record_*_write_to_host_read`), затем CPU дожидается fence сабмита и ТОЛЬКО ПОТОМ
//! зовёт [`HostReadbackBuffer::seal`], предъявив [`CompletedWork`].
//!
//! # Потоки и уничтожение
//! `SealedHostBytes` — `Send + Sync`: после запечатывания CPU только читает (много читателей — не гонка),
//! GPU тоже только читает. Уничтожение на любом потоке безопасно: `vkUnmapMemory`, `vkDestroyBuffer` и
//! `vkFreeMemory` требуют внешней синхронизации лишь по САМОМУ объекту (его уничтожает единственный
//! владелец — последний `Arc`), очередь и другие объекты не затрагиваются. Что GPU к этому моменту
//! закончил с буфером, обеспечивает владелец сабмитов (он держит клон `Arc` до fence последнего чтения).
//!
//! «Fence сигнален» проверяет тип: [`HostReadbackBuffer::seal`] принимает только `&CompletedWork`.
//!
//! Единственный `unsafe` с общей причиной — построение среза над отображением (`SealedHostBytes::bytes`)
//! и `Send/Sync` отображения: оба с `// SAFETY:` ниже.

use std::ptr::NonNull;

use ash::vk;

use crate::compute::DescriptorResource;
use crate::context::{GpuContext, MemoryLocation};
use crate::error::{GpuError, run_step};
use crate::live_objects::{OwnedBuffer, OwnedDeviceMemory};
use crate::one_shot_queue::{CompletedWork, SubmittedWork, WorkId};

/// Буфер + память + постоянное отображение. Порядок полей = порядок уничтожения: буфер → память
/// (отображение снимается в `Drop` до освобождения памяти).
struct MappedBytes {
    buffer: OwnedBuffer,
    memory: OwnedDeviceMemory,
    /// Начало отображения (`size_bytes` байт), валидно до `Drop`.
    mapped: NonNull<u8>,
    size_bytes: u64,
    context: GpuContext,
}

// SAFETY: отображение принадлежит только этой структуре. На стороне CPU после создания (обнуление в
// `new`) память только читается через `&self` (`SealedHostBytes::bytes`), записей через `&mut` нет, поэтому
// гонок данных между потоками CPU нет. Записи GPU идут до запечатывания и упорядочены fence-ом
// (`seal` требует `CompletedWork`). Хэндлы Vulkan можно использовать из любого потока.
unsafe impl Send for MappedBytes {}
// SAFETY: см. выше — через `&self` доступно только чтение.
unsafe impl Sync for MappedBytes {}

impl MappedBytes {
    fn new(context: &GpuContext, size_bytes: u64, usage: vk::BufferUsageFlags) -> Result<Self, GpuError> {
        if size_bytes == 0 {
            return Err(GpuError::InvalidArgument {
                what: "буфер чтения нулевого размера".to_owned(),
            });
        }
        let length = usize::try_from(size_bytes).map_err(|_| GpuError::InvalidArgument {
            what: format!("буфер чтения {size_bytes} байт не помещается в адресное пространство"),
        })?;
        let device = context.device();
        let info = vk::BufferCreateInfo::default()
            .size(size_bytes)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: устройство валидно; info — живая локальная структура.
        let buffer = OwnedBuffer::from_raw(
            context,
            run_step(
                unsafe { device.create_buffer(&info, None) },
                "vkCreateBuffer(HostReadbackBuffer)",
            )?,
        );
        // SAFETY: буфер только что создан этим устройством.
        let requirements = unsafe { device.get_buffer_memory_requirements(buffer.handle()) };
        let memory =
            context.allocate_memory(requirements, MemoryLocation::HostVisibleCoherent, "HostReadbackBuffer")?;
        // SAFETY: память подобрана по требованиям буфера и ещё не привязана; смещение 0.
        run_step(
            unsafe { device.bind_buffer_memory(buffer.handle(), memory.handle(), 0) },
            "vkBindBufferMemory(HostReadbackBuffer)",
        )?;
        // SAFETY: память host-visible, ещё не отображена; отображаем всё выделение (WHOLE_SIZE).
        let address = run_step(
            unsafe { device.map_memory(memory.handle(), 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) },
            "vkMapMemory(HostReadbackBuffer)",
        )?;
        let mapped = NonNull::new(address.cast::<u8>()).ok_or(GpuError::InvalidState {
            what: "vkMapMemory вернул нулевой указатель".to_owned(),
        })?;
        // Обнуляем: срез байт всегда над инициализированной памятью (даже если шейдер недописал).
        // SAFETY: отображение длиной ≥ size_bytes (requirements.size ≥ size), GPU буфер ещё не видел.
        unsafe { std::ptr::write_bytes(mapped.as_ptr(), 0, length) };
        Ok(Self {
            buffer,
            memory,
            mapped,
            size_bytes,
            context: context.clone(),
        })
    }
}

impl Drop for MappedBytes {
    fn drop(&mut self) {
        // SAFETY: память отображена в `new` и с тех пор не снималась; GPU закончил с буфером (инвариант
        // владельца, как у всех `Owned*`). Память освободится после — полем `memory`.
        unsafe { self.context.device().unmap_memory(self.memory.handle()) };
    }
}

/// Usage буфера: storage для шейдеров (запись при захвате, чтение при проявлении) и приёмник копирования
/// из картинки.
const READBACK_USAGE: vk::BufferUsageFlags = vk::BufferUsageFlags::from_raw(
    vk::BufferUsageFlags::STORAGE_BUFFER.as_raw() | vk::BufferUsageFlags::TRANSFER_DST.as_raw(),
);

/// Буфер на стадии записи GPU (см. модуль). Среза байт нет; после `poll → Completed` — [`Self::seal`].
pub struct HostReadbackBuffer {
    inner: MappedBytes,
    /// Работа, отправленная последней с записью в этот буфер (`None` — после создания, пока никто не отметил).
    /// Запечатать можно только доказательством именно этой работы (см. [`Self::seal`]).
    last_writer: Option<WorkId>,
}

impl std::fmt::Debug for HostReadbackBuffer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "HostReadbackBuffer({} байт)", self.inner.size_bytes)
    }
}

impl HostReadbackBuffer {
    /// Создаёт буфер `size_bytes` (> 0), отображает и обнуляет.
    pub fn new(context: &GpuContext, size_bytes: u64) -> Result<Self, GpuError> {
        Ok(Self {
            inner: MappedBytes::new(context, size_bytes, READBACK_USAGE)?,
            last_writer: None,
        })
    }

    /// Размер, байт.
    pub fn size_bytes(&self) -> u64 {
        self.inner.size_bytes
    }

    /// Привязка «чтение-запись» для compute-шейдера, который заполняет буфер.
    pub fn read_write_binding(&self) -> DescriptorResource {
        DescriptorResource::ReadWriteBuffer {
            buffer: self.inner.buffer.handle(),
            range_bytes: self.inner.size_bytes,
        }
    }

    /// Делает записи compute-шейдера видимыми хосту (после последней записи, до конца командного буфера).
    pub fn record_compute_write_to_host_read(&self, command_buffer: vk::CommandBuffer) {
        self.record_write_to_host_read(
            command_buffer,
            vk::PipelineStageFlags2::COMPUTE_SHADER,
            vk::AccessFlags2::SHADER_STORAGE_WRITE,
        );
    }

    /// Делает записи команд копирования (например, из картинки) видимыми хосту.
    pub fn record_transfer_write_to_host_read(&self, command_buffer: vk::CommandBuffer) {
        self.record_write_to_host_read(
            command_buffer,
            vk::PipelineStageFlags2::COPY,
            vk::AccessFlags2::TRANSFER_WRITE,
        );
    }

    /// Запоминает, какая отправленная работа пишет в буфер: после `OneShotSubmitter::submit`, команды которого
    /// писали в него (и записали барьер `record_*_write_to_host_read`). Каждая следующая отметка заменяет
    /// прежнюю: запечатать можно только доказательством ПОСЛЕДНЕЙ отмеченной записи.
    pub fn mark_written_by(&mut self, work: &SubmittedWork) {
        self.last_writer = Some(work.id());
    }

    /// Запечатывает буфер: GPU закончил писать, дальше буфер только читается.
    ///
    /// Условие «fence сабмита с ПОСЛЕДНЕЙ записью и барьером `record_*_write_to_host_read` сигнален» проверяет
    /// ТИП и связь с работой: `proof` ([`CompletedWork`]) создаёт только `OneShotSubmitter::poll`, увидев сигнал
    /// fence, а буфер помнит [`WorkId`] записавшей работы ([`Self::mark_written_by`]) и сверяет его с
    /// доказательством. Чужое доказательство (другая работа или другая очередь) или буфер без отметки записи
    /// отвергаются: буфер ВОЗВРАЩАЕТСЯ внутри [`SealError`] нетронутым (как `ProofMismatch` у цели кодера), чтобы
    /// вызывающий мог повторить с верным доказательством, а не потерял буфер, который GPU ещё пишет.
    pub fn seal(self, proof: &CompletedWork) -> Result<SealedHostBytes, SealError> {
        match self.last_writer {
            None => Err(SealError {
                reason: SealRefusal::WriterNotMarked,
                buffer: self,
            }),
            Some(writer) if writer != proof.id() => Err(SealError {
                reason: SealRefusal::ProofOfOtherWork,
                buffer: self,
            }),
            Some(_) => Ok(SealedHostBytes { inner: self.inner }),
        }
    }

    /// `VkBuffer` для команд копирования внутри crate (`Rgba8StorageImage::record_copy_to_host_readback`).
    pub(crate) fn buffer_handle(&self) -> vk::Buffer {
        self.inner.buffer.handle()
    }

    fn record_write_to_host_read(
        &self,
        command_buffer: vk::CommandBuffer,
        source_stage: vk::PipelineStageFlags2,
        source_access: vk::AccessFlags2,
    ) {
        let barrier = vk::BufferMemoryBarrier2::default()
            .src_stage_mask(source_stage)
            .src_access_mask(source_access)
            .dst_stage_mask(vk::PipelineStageFlags2::HOST)
            .dst_access_mask(vk::AccessFlags2::HOST_READ)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .buffer(self.inner.buffer.handle())
            .offset(0)
            .size(vk::WHOLE_SIZE);
        // SAFETY: командный буфер в состоянии записи вне рендеринга (контракт вызывающего); буфер
        // принадлежит этому устройству.
        unsafe {
            self.inner.context.device().cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().buffer_memory_barriers(&[barrier]),
            );
        }
    }
}

/// Почему буфер не запечатан.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SealRefusal {
    /// Буферу не сообщили, какая работа в него пишет ([`HostReadbackBuffer::mark_written_by`]): доказать нечем.
    WriterNotMarked,
    /// Доказательство относится не к той работе, что писала в буфер.
    ProofOfOtherWork,
}

/// Отказ [`HostReadbackBuffer::seal`]: буфер возвращён нетронутым.
#[derive(Debug)]
pub struct SealError {
    /// Причина отказа.
    pub reason: SealRefusal,
    /// Буфер, не тронутый запечатыванием (GPU мог ещё писать в него: уничтожать нельзя без доказательства).
    pub buffer: HostReadbackBuffer,
}

impl std::fmt::Display for SealError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.reason {
            SealRefusal::WriterNotMarked => {
                write!(formatter, "буфер не знает, какая работа в него пишет: запечатать нечем")
            }
            SealRefusal::ProofOfOtherWork => {
                write!(
                    formatter,
                    "доказательство завершения относится не к работе, писавшей в буфер"
                )
            }
        }
    }
}

impl std::error::Error for SealError {}

/// Запечатанный буфер: читают и CPU (срез байт), и GPU (привязка «только чтение»), писать нельзя.
pub struct SealedHostBytes {
    inner: MappedBytes,
}

impl std::fmt::Debug for SealedHostBytes {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "SealedHostBytes({} байт)", self.inner.size_bytes)
    }
}

impl SealedHostBytes {
    /// Размер, байт.
    pub fn size_bytes(&self) -> u64 {
        self.inner.size_bytes
    }

    /// Все байты буфера — прямо над отображённой памятью, без копирования.
    pub fn bytes(&self) -> &[u8] {
        // Размер проверен в `new` (`usize::try_from`), поэтому приведение не теряет значение; если вдруг
        // не поместилось — пустой срез, а не паника.
        let length = usize::try_from(self.inner.size_bytes).unwrap_or(0);
        // SAFETY: отображение живёт, пока жив `self` (`unmap` — в `Drop` внутренней структуры), длиной не
        // меньше `size_bytes`; память инициализирована (обнулена в `new`, далее писал только GPU); GPU
        // закончил писать до запечатывания (`seal` требует `CompletedWork`) и после него только читает, CPU
        // записей не делает — изменяемых ссылок или конкурирующих записей на время жизни среза нет.
        unsafe { std::slice::from_raw_parts(self.inner.mapped.as_ptr(), length) }
    }

    /// Привязка «только чтение» для compute-шейдера (проявление читает сырые отсчёты захвата).
    pub fn read_only_binding(&self) -> DescriptorResource {
        DescriptorResource::ReadOnlyBuffer {
            buffer: self.inner.buffer.handle(),
            range_bytes: self.inner.size_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::test_support::gpu_context_or_skip;
    use crate::live_objects::LiveObjectKind;

    /// Тип отправляется между потоками и читается с нескольких (проверка на этапе компиляции).
    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn sealed_bytes_can_move_and_be_shared_across_threads() {
        assert_send_sync::<SealedHostBytes>();
        assert_send_sync::<HostReadbackBuffer>();
    }

    #[test]
    fn zero_size_is_rejected_and_nothing_leaks() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        let error = HostReadbackBuffer::new(&context, 0).unwrap_err();
        assert!(matches!(error, GpuError::InvalidArgument { .. }), "{error:?}");
        assert_eq!(context.live_objects(), baseline);
    }

    #[test]
    fn gpu_fill_becomes_visible_to_cpu_after_seal_and_buffer_dies_with_last_owner() {
        let Some(context) = gpu_context_or_skip() else { return };
        let baseline = context.live_objects();
        {
            let mut buffer = HostReadbackBuffer::new(&context, 64).unwrap();
            assert_eq!(buffer.size_bytes(), 64);
            assert!(matches!(
                buffer.read_write_binding(),
                DescriptorResource::ReadWriteBuffer { range_bytes: 64, .. }
            ));
            let run = crate::kit_test_support::run_one_shot_blocking_traced(&context, |command_buffer| {
                // SAFETY: командный буфер в состоянии записи; буфер имеет usage TRANSFER_DST.
                unsafe {
                    context
                        .device()
                        .cmd_fill_buffer(command_buffer, buffer.buffer_handle(), 0, 64, 0xAB_AB_AB_AB);
                }
                buffer.record_transfer_write_to_host_read(command_buffer);
                Ok(())
            })
            .unwrap();
            buffer.mark_written_by(&run.work);
            let sealed = std::sync::Arc::new(buffer.seal(&run.proof).unwrap());
            assert_eq!(sealed.bytes(), &[0xAB_u8; 64][..], "байты видны без копирования");
            assert!(matches!(
                sealed.read_only_binding(),
                DescriptorResource::ReadOnlyBuffer { range_bytes: 64, .. }
            ));
            assert_eq!(context.live_objects().count(LiveObjectKind::Buffer), 1);
            // Читает и другой поток; уничтожение — в потоке-последнем владельце.
            let reader = std::sync::Arc::clone(&sealed);
            let sum: u32 = std::thread::spawn(move || reader.bytes().iter().map(|byte| u32::from(*byte)).sum())
                .join()
                .unwrap();
            assert_eq!(sum, 64 * 0xAB);
        }
        assert_eq!(context.live_objects(), baseline, "буфер и память освобождены");
        assert_eq!(
            context.validation_report().errors,
            0,
            "{:?}",
            context.validation_report()
        );
    }

    #[test]
    fn fresh_buffer_is_zeroed_so_unwritten_bytes_are_defined() {
        let Some(context) = gpu_context_or_skip() else { return };
        let mut buffer = HostReadbackBuffer::new(&context, 16).unwrap();
        // Пустая работа: GPU ничего не пишет, но доказательство настоящее (fence сигнален) и отмечено как
        // «работа, писавшая в буфер».
        let run = crate::kit_test_support::run_one_shot_blocking_traced(&context, |_| Ok(())).unwrap();
        buffer.mark_written_by(&run.work);
        let sealed = buffer.seal(&run.proof).unwrap();
        assert_eq!(sealed.bytes(), &[0_u8; 16][..]);
    }

    #[test]
    fn seal_refuses_buffer_without_a_marked_writer_and_returns_it_untouched() {
        let Some(context) = gpu_context_or_skip() else { return };
        let buffer = HostReadbackBuffer::new(&context, 16).unwrap();
        let run = crate::kit_test_support::run_one_shot_blocking_traced(&context, |_| Ok(())).unwrap();
        let error = buffer.seal(&run.proof).unwrap_err();
        assert_eq!(error.reason, SealRefusal::WriterNotMarked);
        assert_eq!(error.buffer.size_bytes(), 16, "буфер возвращён нетронутым");
    }

    #[test]
    fn seal_refuses_proof_of_another_work_then_accepts_the_right_one() {
        let Some(context) = gpu_context_or_skip() else { return };
        let mut buffer = HostReadbackBuffer::new(&context, 32).unwrap();
        // Писавшая работа: заполняет буфер.
        let writer = crate::kit_test_support::run_one_shot_blocking_traced(&context, |command_buffer| {
            // SAFETY: командный буфер в состоянии записи; буфер имеет usage TRANSFER_DST.
            unsafe {
                context
                    .device()
                    .cmd_fill_buffer(command_buffer, buffer.buffer_handle(), 0, 32, 0x5A_5A_5A_5A);
            }
            buffer.record_transfer_write_to_host_read(command_buffer);
            Ok(())
        })
        .unwrap();
        buffer.mark_written_by(&writer.work);
        // Чужая работа (другая очередь, другая работа): её доказательство не годится.
        let stranger = crate::kit_test_support::run_one_shot_blocking_traced(&context, |_| Ok(())).unwrap();
        let error = buffer.seal(&stranger.proof).unwrap_err();
        assert_eq!(error.reason, SealRefusal::ProofOfOtherWork);
        // Буфер вернулся нетронутым и запечатывается верным доказательством, данные писавшей работы целы.
        let sealed = error.buffer.seal(&writer.proof).unwrap();
        assert_eq!(sealed.bytes(), &[0x5A_u8; 32][..]);
    }
}
