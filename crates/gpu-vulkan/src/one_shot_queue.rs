//! [`OneShotSubmitter`] — одноразовая работа на GPU БЕЗ ожидания CPU (сессия 14, пакет P4a).
//!
//! # Зачем
//! `GpuContext::run_one_shot` блокирует вызывающего до fence: на потоке рендера (продукт) это
//! недопустимо. Здесь — тот же «записать командный буфер и отправить», но результат наблюдается опросом:
//!
//! ```text
//! submit(waits, record) ──▶ SubmittedWork ──poll()──▶ Pending | Completed(CompletedWork) | Failed
//!        CPU не ждёт             токен             vkGetFenceStatus, без ожидания
//! ```
//!
//! # Доказательство завершения
//! [`CompletedWork`] — типовое доказательство «fence этой работы сигнален». Создаётся ТОЛЬКО в
//! [`OneShotSubmitter::poll`] (конструктор приватный), поэтому API, которым нужно «GPU закончил»
//! (`HostReadbackBuffer::seal`, `EncoderInputTarget::detach`), принимают `&CompletedWork`, а не верят
//! документации. Доказательство несёт [`WorkId`] работы: потребитель может проверить, что оно про ту работу.
//!
//! # Ёмкость и ресурсы
//! Фиксированное число слотов (командный пул + буфер + fence на слот). Слот освобождается ЛЕНИВО, и только
//! когда завершение уже наблюдено (`poll` вернул `Completed` или `drain` дождался): тогда «токен переиспользованного
//! слота» всё равно честно означает «завершено». Исчерпание ёмкости — явная ошибка
//! [`SubmitError::CapacityExhausted`], а не блокировка.
//!
//! # Уничтожение
//! Ни одного бесконечного ожидания. Остановка — [`OneShotSubmitter::drain`] с таймаутом. Если GPU не
//! закончил к моменту `Drop`, пул и fence занятых слотов намеренно утекают (соглашение crate: лучше
//! утечка, чем уничтожение занятого).

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ash::vk;

use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::frame_ring::timeout_nanos;
use crate::live_objects::{OwnedCommandPool, OwnedFence};
use crate::sync::WaitSemaphore;

#[cfg(test)]
mod tests;

/// Источник уникальных номеров очередей: токен чужой очереди нельзя спутать со своим.
static NEXT_SUBMITTER_ID: AtomicU64 = AtomicU64::new(1);

/// Стадия ожидания семафоров и сигналов: `ALL_COMMANDS` (без maintenance8 точнее нельзя, см. `conventions`).
const SYNC_STAGE: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::ALL_COMMANDS;

/// Идентификатор одной отправленной работы: очередь-владелец + порядковый номер.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WorkId {
    submitter: u64,
    serial: u64,
}

/// Токен отправленной работы. Не `Clone`: единственный владелец решает, когда его опрашивать.
#[derive(Debug)]
pub struct SubmittedWork {
    id: WorkId,
    slot: usize,
}

impl SubmittedWork {
    /// Идентификатор работы (сверяется с [`CompletedWork::id`]).
    pub fn id(&self) -> WorkId {
        self.id
    }
}

/// Доказательство: fence работы сигнален (GPU закончил и записи видны). Создаётся только [`OneShotSubmitter::poll`].
///
/// `Clone`: факт «работа завершена» не пропадает, а размножение доказательства ничего не ломает.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletedWork {
    id: WorkId,
}

impl CompletedWork {
    /// Какой работе принадлежит доказательство.
    pub fn id(&self) -> WorkId {
        self.id
    }
}

/// Почему опрос не смог дать ответ «идёт / готово».
#[derive(Debug)]
pub enum WorkFailure {
    /// Токен выдан другой очередью (или слот вне диапазона): ответить за чужую работу нельзя.
    ForeignWork,
    /// Слот потерян (ошибка опроса fence раньше, например потеря устройства): исход работы неизвестен.
    SlotLost,
    /// `vkGetFenceStatus` вернул ошибку (устройство потеряно): исход неизвестен, слот потерян.
    Gpu(GpuError),
}

/// Результат неблокирующего опроса работы.
#[derive(Debug)]
pub enum WorkStatus {
    /// GPU ещё выполняет (или ждёт семафор).
    Pending,
    /// GPU закончил; доказательство для API, требующих «fence сигнален».
    Completed(CompletedWork),
    /// Опрос не удался; см. [`WorkFailure`].
    Failed(WorkFailure),
}

/// Итог [`OneShotSubmitter::drain`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DrainOutcome {
    /// Все отправленные работы завершились.
    Drained,
    /// Таймаут: столько работ ещё выполняются (объекты НЕ освобождены).
    TimedOut {
        /// Сколько слотов осталось занятыми.
        still_in_flight: usize,
    },
}

/// Ошибка отправки. `E` — ошибка замыкания записи вызывающего (не сливается с ошибками GPU).
#[derive(Debug, thiserror::Error)]
pub enum SubmitError<E: fmt::Debug> {
    /// Все слоты заняты работой, исход которой ещё не наблюдён: опросите `poll` / дождитесь `drain`.
    #[error("все {capacity} слотов одноразовой очереди заняты")]
    CapacityExhausted {
        /// Ёмкость очереди.
        capacity: usize,
    },
    /// Замыкание записи вернуло ошибку: ничего не отправлено, слот свободен.
    #[error("запись команд не удалась: {0:?}")]
    Record(E),
    /// Ошибка Vulkan при подготовке или отправке: работа не принята, слот свободен.
    #[error(transparent)]
    Gpu(GpuError),
}

/// Состояние слота.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    /// Свободен: пул сброшен, fence несигнальный.
    Free,
    /// Отправлен; завершение ещё не наблюдалось.
    InFlight,
    /// Завершение наблюдено (fence сигнален), слот ждёт ленивого сброса при следующей отправке.
    Settled,
    /// Исход неизвестен (ошибка опроса): слот не используется и при `Drop` утекает.
    Lost,
}

/// Один слот: пул с одним командным буфером и fence.
struct Slot {
    // Порядок полей = порядок уничтожения: удерживаемые ожидания → fence → пул.
    held_waits: Vec<WaitSemaphore>,
    fence: OwnedFence,
    pool: OwnedCommandPool,
    command_buffer: vk::CommandBuffer,
    state: SlotState,
    /// Номер последней принятой в этот слот работы (0 — слот не использовался).
    last_serial: u64,
}

/// Очередь одноразовой работы (см. модуль).
pub struct OneShotSubmitter {
    context: GpuContext,
    id: u64,
    slots: Vec<Slot>,
    next_serial: u64,
}

impl fmt::Debug for OneShotSubmitter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "OneShotSubmitter(ёмкость {}, в работе {})",
            self.slots.len(),
            self.in_flight_count()
        )
    }
}

impl OneShotSubmitter {
    /// Создаёт очередь на `capacity` слотов (> 0).
    pub fn new(context: &GpuContext, capacity: usize) -> Result<Self, GpuError> {
        if capacity == 0 {
            return Err(GpuError::InvalidArgument {
                what: "ёмкость одноразовой очереди должна быть > 0".to_owned(),
            });
        }
        let mut slots = Vec::with_capacity(capacity);
        for _ in 0..capacity {
            slots.push(create_slot(context)?);
        }
        Ok(Self {
            context: context.clone(),
            id: NEXT_SUBMITTER_ID.fetch_add(1, Ordering::Relaxed),
            slots,
            next_serial: 1,
        })
    }

    /// Ёмкость (число слотов).
    pub fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Сколько слотов занято работой, чьё завершение ещё не наблюдено (без опроса GPU).
    pub fn in_flight_count(&self) -> usize {
        self.slots
            .iter()
            .filter(|slot| slot.state == SlotState::InFlight)
            .count()
    }

    /// Записывает и отправляет работу; CPU не ждёт.
    ///
    /// `waits` — импортированные fence'ы камеры: очередь ВЛАДЕЕТ ими до завершения работы (семафор нельзя
    /// уничтожать, пока ждущий его сабмит не закончился). `NoWait` игнорируется.
    /// Ошибка записи или отправки не занимает слот.
    pub fn submit<E: fmt::Debug>(
        &mut self,
        waits: Vec<WaitSemaphore>,
        record: impl FnOnce(vk::CommandBuffer) -> Result<(), E>,
    ) -> Result<SubmittedWork, SubmitError<E>> {
        self.submit_with_sync(waits, &SemaphoreSync::default(), record)
    }

    /// То же, но с сырыми бинарными семафорами (acquire/«рендер готов» цели кодера). Семафоры принадлежат
    /// вызывающему и обязаны жить до завершения работы.
    pub(crate) fn submit_with_sync<E: fmt::Debug>(
        &mut self,
        waits: Vec<WaitSemaphore>,
        sync: &SemaphoreSync<'_>,
        record: impl FnOnce(vk::CommandBuffer) -> Result<(), E>,
    ) -> Result<SubmittedWork, SubmitError<E>> {
        let capacity = self.slots.len();
        let index = self
            .claim_free_slot()
            .map_err(SubmitError::Gpu)?
            .ok_or(SubmitError::CapacityExhausted { capacity })?;
        let command_buffer = self.slots[index].command_buffer;

        if let Err(error) = self.record_commands(command_buffer, record) {
            // Буфер мог остаться недозаписанным: пул сбрасываем, слот остаётся свободным.
            self.reset_pool_after_unsubmitted(index);
            return Err(error);
        }

        let wait_infos: Vec<vk::SemaphoreSubmitInfo<'_>> = waits
            .iter()
            .filter_map(WaitSemaphore::semaphore)
            .chain(sync.waits.iter().copied())
            .map(|semaphore| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(semaphore)
                    .stage_mask(SYNC_STAGE)
            })
            // Timeline-ожидания: `value` — значение, которого ждём (у бинарных оно игнорируется, там 0).
            .chain(sync.timeline_waits.iter().map(|(semaphore, value)| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(*semaphore)
                    .value(*value)
                    .stage_mask(SYNC_STAGE)
            }))
            .collect();
        let signal_infos: Vec<vk::SemaphoreSubmitInfo<'_>> = sync
            .signals
            .iter()
            .map(|semaphore| {
                vk::SemaphoreSubmitInfo::default()
                    .semaphore(*semaphore)
                    .stage_mask(SYNC_STAGE)
            })
            .collect();
        let command_buffer_infos = [vk::CommandBufferSubmitInfo::default().command_buffer(command_buffer)];
        let submit = vk::SubmitInfo2::default()
            .wait_semaphore_infos(&wait_infos)
            .command_buffer_infos(&command_buffer_infos)
            .signal_semaphore_infos(&signal_infos);
        let fence = self.slots[index].fence.handle();
        if let Err(error) = self.context.submit(&[submit], fence) {
            // Драйвер работу не принял: GPU буфер не видел, объекты уничтожать/сбрасывать безопасно.
            self.reset_pool_after_unsubmitted(index);
            return Err(SubmitError::Gpu(error));
        }

        let serial = self.next_serial;
        self.next_serial += 1;
        let slot = &mut self.slots[index];
        slot.state = SlotState::InFlight;
        slot.last_serial = serial;
        slot.held_waits = waits;
        Ok(SubmittedWork {
            id: WorkId {
                submitter: self.id,
                serial,
            },
            slot: index,
        })
    }

    /// Неблокирующий опрос (`vkGetFenceStatus`). Повторный опрос завершённой работы снова даёт `Completed`.
    pub fn poll(&mut self, work: &SubmittedWork) -> WorkStatus {
        if work.id.submitter != self.id {
            return WorkStatus::Failed(WorkFailure::ForeignWork);
        }
        let Some(slot) = self.slots.get_mut(work.slot) else {
            return WorkStatus::Failed(WorkFailure::ForeignWork);
        };
        // Слот уже принял более новую работу: он освобождается только после наблюдённого завершения,
        // значит эта работа гарантированно завершилась.
        if slot.last_serial > work.id.serial {
            return WorkStatus::Completed(CompletedWork { id: work.id });
        }
        if slot.last_serial < work.id.serial {
            return WorkStatus::Failed(WorkFailure::ForeignWork);
        }
        match slot.state {
            SlotState::Settled => WorkStatus::Completed(CompletedWork { id: work.id }),
            SlotState::Lost | SlotState::Free => WorkStatus::Failed(WorkFailure::SlotLost),
            SlotState::InFlight => {
                // SAFETY: fence принадлежит этому устройству; опрос не требует внешней синхронизации.
                match unsafe { self.context.device().get_fence_status(slot.fence.handle()) } {
                    Ok(true) => {
                        slot.state = SlotState::Settled;
                        WorkStatus::Completed(CompletedWork { id: work.id })
                    }
                    Ok(false) => WorkStatus::Pending,
                    Err(result) => {
                        slot.state = SlotState::Lost;
                        WorkStatus::Failed(WorkFailure::Gpu(GpuError::Vulkan {
                            step: "vkGetFenceStatus(one_shot_queue)",
                            result,
                        }))
                    }
                }
            }
        }
    }

    /// Ограниченное ожидание ВСЕХ работ в полёте (остановка). После `Drained` их токены дают `Completed`.
    pub fn drain(&mut self, timeout: Duration) -> Result<DrainOutcome, GpuError> {
        let in_flight: Vec<vk::Fence> = self
            .slots
            .iter()
            .filter(|slot| slot.state == SlotState::InFlight)
            .map(|slot| slot.fence.handle())
            .collect();
        if in_flight.is_empty() {
            return Ok(DrainOutcome::Drained);
        }
        // SAFETY: fence'ы принадлежат этому устройству и отправлены в очередь; срок конечный.
        let wait = unsafe {
            self.context
                .device()
                .wait_for_fences(&in_flight, true, timeout_nanos(timeout))
        };
        match wait {
            Ok(()) => {
                for slot in &mut self.slots {
                    if slot.state == SlotState::InFlight {
                        slot.state = SlotState::Settled;
                    }
                }
                Ok(DrainOutcome::Drained)
            }
            Err(vk::Result::TIMEOUT) => {
                // Часть работ могла успеть: учитываем их, чтобы счётчик «осталось» был точным.
                self.settle_signaled_slots()?;
                Ok(DrainOutcome::TimedOut {
                    still_in_flight: self.in_flight_count(),
                })
            }
            Err(result) => Err(GpuError::Vulkan {
                step: "vkWaitForFences(one_shot_queue drain)",
                result,
            }),
        }
    }

    /// Переводит в `Settled` все слоты, чей fence уже сигнален.
    fn settle_signaled_slots(&mut self) -> Result<(), GpuError> {
        let device = self.context.device();
        for slot in &mut self.slots {
            if slot.state != SlotState::InFlight {
                continue;
            }
            // SAFETY: fence принадлежит этому устройству.
            match unsafe { device.get_fence_status(slot.fence.handle()) } {
                Ok(true) => slot.state = SlotState::Settled,
                Ok(false) => {}
                Err(result) => {
                    slot.state = SlotState::Lost;
                    return Err(GpuError::Vulkan {
                        step: "vkGetFenceStatus(one_shot_queue)",
                        result,
                    });
                }
            }
        }
        Ok(())
    }

    /// Находит свободный слот; при необходимости лениво сбрасывает завершённый. `None` — все заняты.
    fn claim_free_slot(&mut self) -> Result<Option<usize>, GpuError> {
        if let Some(index) = self.slots.iter().position(|slot| slot.state == SlotState::Free) {
            return Ok(Some(index));
        }
        // Дёшево подтягиваем завершения, которые ещё никто не опрашивал: иначе забытый токен держал бы слот.
        self.settle_signaled_slots()?;
        let Some(index) = self.slots.iter().position(|slot| slot.state == SlotState::Settled) else {
            return Ok(None);
        };
        let device = self.context.device();
        let slot = &mut self.slots[index];
        // SAFETY: fence сигнален (завершение наблюдено) и ни в одной очереди не ждёт.
        run_step(
            unsafe { device.reset_fences(&[slot.fence.handle()]) },
            "vkResetFences(one_shot_queue)",
        )?;
        // SAFETY: GPU закончил с буфером пула (fence сигнален).
        run_step(
            unsafe { device.reset_command_pool(slot.pool.handle(), vk::CommandPoolResetFlags::empty()) },
            "vkResetCommandPool(one_shot_queue)",
        )?;
        slot.held_waits.clear();
        slot.state = SlotState::Free;
        Ok(Some(index))
    }

    /// `begin` → замыкание → `end`.
    fn record_commands<E: fmt::Debug>(
        &self,
        command_buffer: vk::CommandBuffer,
        record: impl FnOnce(vk::CommandBuffer) -> Result<(), E>,
    ) -> Result<(), SubmitError<E>> {
        let device = self.context.device();
        let begin_info = vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: буфер в исходном состоянии (пул сброшен), принадлежит этому потоку.
        run_step(
            unsafe { device.begin_command_buffer(command_buffer, &begin_info) },
            "vkBeginCommandBuffer(one_shot_queue)",
        )
        .map_err(SubmitError::Gpu)?;
        record(command_buffer).map_err(SubmitError::Record)?;
        // SAFETY: запись завершена в этом же потоке.
        run_step(
            unsafe { device.end_command_buffer(command_buffer) },
            "vkEndCommandBuffer(one_shot_queue)",
        )
        .map_err(SubmitError::Gpu)
    }

    /// Работа не принята драйвером/не записана: возвращаем пул слота в исходное состояние.
    fn reset_pool_after_unsubmitted(&mut self, index: usize) {
        let slot = &self.slots[index];
        // SAFETY: буфер слота GPU не отправлялся, значит ни в какой очереди не выполняется.
        let reset = unsafe {
            self.context
                .device()
                .reset_command_pool(slot.pool.handle(), vk::CommandPoolResetFlags::empty())
        };
        if let Err(result) = reset {
            // Слот остаётся «свободным» формально, но пул не сброшен: безопаснее вывести его из оборота.
            tracing::error!(target: "fasticamera::gpu", ?result, "не удалось сбросить пул неотправленной работы; слот выведен из оборота");
            self.slots[index].state = SlotState::Lost;
        }
    }
}

impl Drop for OneShotSubmitter {
    fn drop(&mut self) {
        for slot in std::mem::take(&mut self.slots) {
            let busy = match slot.state {
                SlotState::Free | SlotState::Settled => false,
                SlotState::Lost => true,
                // SAFETY: fence принадлежит этому устройству; опрос без ожидания.
                SlotState::InFlight => !matches!(
                    unsafe { self.context.device().get_fence_status(slot.fence.handle()) },
                    Ok(true)
                ),
            };
            if busy {
                tracing::error!(
                    target: "fasticamera::gpu",
                    "очередь одноразовой работы уничтожена с работой в полёте: пул, fence и ожидания слота намеренно НЕ уничтожаются (утечка)"
                );
                std::mem::forget(slot);
            }
        }
    }
}

/// Сырые семафоры, которые сабмит ждёт и сигналит (бинарные — для цели кодера; timeline — внутренние зависимости).
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct SemaphoreSync<'a> {
    /// Бинарные семафоры, которых ждать до команд.
    pub(crate) waits: &'a [vk::Semaphore],
    /// Timeline-семафоры и значения, которых ждать до команд (внутренние зависимости между сабмитами).
    pub(crate) timeline_waits: &'a [(vk::Semaphore, u64)],
    /// Сигналить после команд.
    pub(crate) signals: &'a [vk::Semaphore],
}

/// Создаёт слот: пул (TRANSIENT) с одним первичным буфером и несигнальный fence.
fn create_slot(context: &GpuContext) -> Result<Slot, GpuError> {
    let device = context.device();
    let pool_info = vk::CommandPoolCreateInfo::default()
        .queue_family_index(context.queue_family_index())
        .flags(vk::CommandPoolCreateFlags::TRANSIENT);
    // SAFETY: устройство валидно; pool_info — живая локальная структура.
    let pool = OwnedCommandPool::from_raw(
        context,
        run_step(
            unsafe { device.create_command_pool(&pool_info, None) },
            "vkCreateCommandPool(one_shot_queue)",
        )?,
    );
    let allocate_info = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool.handle())
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: пул только что создан этим устройством; буфер освободится вместе с пулом.
    let buffers = run_step(
        unsafe { device.allocate_command_buffers(&allocate_info) },
        "vkAllocateCommandBuffers(one_shot_queue)",
    )?;
    let command_buffer = buffers.first().copied().ok_or(GpuError::InvalidState {
        what: "vkAllocateCommandBuffers не вернул буфер".to_owned(),
    })?;
    // SAFETY: устройство валидно.
    let fence = OwnedFence::from_raw(
        context,
        run_step(
            unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) },
            "vkCreateFence(one_shot_queue)",
        )?,
    );
    Ok(Slot {
        held_waits: Vec::new(),
        fence,
        pool,
        command_buffer,
        state: SlotState::Free,
        last_serial: 0,
    })
}
