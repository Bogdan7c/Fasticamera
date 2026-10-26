//! Timeline-семафор для внутренних зависимостей между сабмитами (счётчик только растёт).

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use ash::vk;

use super::cpu_wait::timeout_nanos;
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedSemaphore;

/// Итог ожидания значения на CPU. Таймаут — штатный исход, не ошибка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimelineWait {
    /// Счётчик достиг запрошенного значения.
    Reached,
    /// За отведённое время значение не достигнуто.
    TimedOut,
}

/// Владеемый timeline-семафор.
///
/// Инвариант (VUID-VkSubmitInfo2-semaphore-03882): каждое следующее
/// значение сигнала строго больше предыдущего отправленного. Тип помнит последнее отправленное
/// значение и отвергает нарушение до обращения к драйверу (`InvalidArgument`).
#[derive(Debug)]
pub struct TimelineSemaphore {
    semaphore: OwnedSemaphore,
    /// Наибольшее значение, чей сигнал успешно отправлен (стартовое значение — начальное).
    last_submitted_value: AtomicU64,
    context: GpuContext,
}

impl TimelineSemaphore {
    /// Создаёт timeline-семафор с начальным значением.
    pub fn new(context: &GpuContext, initial_value: u64) -> Result<Self, GpuError> {
        let mut type_info = vk::SemaphoreTypeCreateInfo::default()
            .semaphore_type(vk::SemaphoreType::TIMELINE)
            .initial_value(initial_value);
        let create_info = vk::SemaphoreCreateInfo::default().push_next(&mut type_info);
        // SAFETY: устройство валидно (фича timelineSemaphore включена контекстом).
        let semaphore = OwnedSemaphore::from_raw(
            context,
            run_step(
                unsafe { context.device().create_semaphore(&create_info, None) },
                "vkCreateSemaphore(timeline)",
            )?,
        );
        Ok(Self {
            semaphore,
            last_submitted_value: AtomicU64::new(initial_value),
            context: context.clone(),
        })
    }

    /// Дескриптор (для `SubmitInfo2`).
    pub fn handle(&self) -> vk::Semaphore {
        self.semaphore.handle()
    }

    /// Наибольшее значение, сигнал которого отправлен на GPU или зарезервирован отправкой,
    /// идущей прямо сейчас (не обязательно уже выполнен).
    pub fn last_submitted_value(&self) -> u64 {
        self.last_submitted_value.load(Ordering::Acquire)
    }

    /// Значение, которого GPU уже достиг (`vkGetSemaphoreCounterValue`); не убывает.
    pub fn completed_value(&self) -> Result<u64, GpuError> {
        // SAFETY: семафор принадлежит этому устройству.
        run_step(
            unsafe { self.context.device().get_semaphore_counter_value(self.handle()) },
            "vkGetSemaphoreCounterValue",
        )
    }

    /// Ждёт на CPU, пока счётчик достигнет `value`, не дольше `timeout`.
    pub fn wait_cpu(&self, value: u64, timeout: Duration) -> Result<TimelineWait, GpuError> {
        let handles = [self.handle()];
        let values = [value];
        let wait_info = vk::SemaphoreWaitInfo::default().semaphores(&handles).values(&values);
        // SAFETY: семафор принадлежит устройству, массивы живут до конца вызова.
        match unsafe {
            self.context
                .device()
                .wait_semaphores(&wait_info, timeout_nanos(timeout))
        } {
            Ok(()) => Ok(TimelineWait::Reached),
            Err(vk::Result::TIMEOUT) => Ok(TimelineWait::TimedOut),
            Err(result) => Err(GpuError::Vulkan {
                step: "vkWaitSemaphores",
                result,
            }),
        }
    }

    /// Сигналит значение с CPU (`vkSignalSemaphore`) — для ворот, которые открывает не GPU.
    ///
    /// Значение подчиняется тому же правилу монотонности, что и сигнал сабмита.
    pub fn signal_from_cpu(&self, value: u64) -> Result<(), GpuError> {
        let reservation = self.reserve_signal_value(value)?;
        let signal_info = vk::SemaphoreSignalInfo::default().semaphore(self.handle()).value(value);
        // SAFETY: семафор принадлежит этому устройству; значение больше всех прежних зарезервировано выше.
        let result = run_step(
            unsafe { self.context.device().signal_semaphore(&signal_info) },
            "vkSignalSemaphore",
        );
        if result.is_err() {
            self.cancel_reservation(reservation);
        }
        result
    }

    /// Атомарно проверяет и занимает значение сигнала: проверка «больше отправленного» и запись
    /// нового значения — один шаг, поэтому два потока не могут оба получить одно значение
    /// (раздельные «проверить» и «записать после сабмита» это допускали).
    pub(super) fn reserve_signal_value(&self, value: u64) -> Result<SignalReservation, GpuError> {
        let mut last = self.last_submitted_value.load(Ordering::Acquire);
        loop {
            if value <= last {
                return Err(GpuError::InvalidArgument {
                    what: format!("значение сигнала timeline {value} не больше уже отправленного {last}"),
                });
            }
            match self
                .last_submitted_value
                .compare_exchange(last, value, Ordering::AcqRel, Ordering::Acquire)
            {
                Ok(_) => return Ok(SignalReservation { previous: last, value }),
                // Другой поток успел раньше: перечитываем и проверяем заново.
                Err(actual) => last = actual,
            }
        }
    }

    /// Отменяет резерв после неудачной отправки. Если за это время другой поток уже занял большее
    /// значение, откатывать нечего: его резерв остаётся в силе.
    pub(super) fn cancel_reservation(&self, reservation: SignalReservation) {
        if self
            .last_submitted_value
            .compare_exchange(
                reservation.value,
                reservation.previous,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            tracing::debug!(
                target: "fasticamera::gpu::sync",
                value = reservation.value,
                "откат резерва timeline не нужен: занято большее значение"
            );
        }
    }
}

/// Занятое значение сигнала timeline: нужно, чтобы откатить его, если отправка не удалась.
#[derive(Debug, Clone, Copy)]
pub(super) struct SignalReservation {
    previous: u64,
    value: u64,
}
