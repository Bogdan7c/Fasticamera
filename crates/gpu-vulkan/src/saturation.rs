//! Стенд насыщения: честная цена GPU-работы на телефоне (сессия 16a, пакет P4-kit).
//!
//! # Зачем (простыми словами)
//! На Adreno 740 драйвер сам понижает частоту GPU, когда работы мало. Лёгкая работа, запускаемая раз в
//! кадр (1/60 с), «занимает» почти весь кадр просто потому, что GPU при этом тактуется на минимуме.
//! Поэтому «миллисекунды в темпе vsync» — не цена работы, а цена работы плюс настроение драйвера.
//!
//! # Что такое насыщение
//! Мы отправляем кадры работы ПОДРЯД, без пауз и без показа на экран, и всегда держим в очереди GPU
//! следующий кадр: GPU не простаивает, драйвер поднимает частоту до максимума. В таком режиме
//! `цена кадра = 1 / пропускная способность` (кадров в секунду по настенным часам хоста). Timestamp-запросы
//! по проходам, снятые в этом же режиме, показывают доли внутри кадра (это дело самой работы).
//!
//! # Как устроено
//! ```text
//! SaturationBench::run(работа, токен отмены)
//!   цикл: [забрать готовые кадры] → [нет свободного слота? ждать fence самого старого]
//!         → работа.record_frame(слот) → отправка
//!   прогрев (кадры не считаем) → окно замера (считаем по моменту завершения) → хвост (дожидаемся)
//! ```
//! * Стенд владеет своими командными буферами и fence-ами; работа — только содержимым кадра
//!   ([`SaturationWorkload`]). Что внутри работы, стенд не знает.
//! * Кадр засчитывается в то время, когда стенд увидел fence сигнальным (монотонные часы `Instant`),
//!   и в тот шаг шкалы, где это случилось. Ошибка квантования — не больше одного кадра на границе шага.
//! * Остановка: ошибка работы или GPU — прогон возвращает ошибку, но сначала дожидается отправленных кадров
//!   (ничего не уничтожается, пока GPU работает). Отмена снаружи ([`CancelToken`]) — не ошибка: итог с
//!   пометкой [`EndReason::Cancelled`] и тем, что успели намерить.
//! * В цикле кадров стенд не выделяет память и не пишет в журнал.

mod config;
mod pass_times;
mod raw_frame;
mod report;
mod slots;
mod workload;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

pub use self::config::{
    DEFAULT_FENCE_TIMEOUT, DEFAULT_MEASURE, DEFAULT_TIMELINE_STEP, DEFAULT_WARMUP, MAX_TIMELINE_STEPS,
    SaturationConfig, SaturationConfigError,
};
pub use self::pass_times::{PassTime, PassTimeLog, PassTimeSummary};
pub use self::raw_frame::{HostRawFrame, RawFrameError};
pub use self::report::{EndReason, SaturationReport, Throughput, TimelineStep};
pub use self::workload::{FrameRecording, SaturationWorkload, SlotId};

use self::report::FrameTally;
use self::slots::{OldestWait, SlotRing};
use crate::context::GpuContext;
use crate::error::GpuError;

/// Признак «остановить прогон». Клонируется: одну копию держит поток прогона, другую — тот, кто остановит
/// (например, обработчик ухода приложения в фон). Остановка необратима для этого токена.
#[derive(Debug, Clone, Default)]
pub struct CancelToken {
    cancelled: Arc<AtomicBool>,
}

impl CancelToken {
    /// Новый токен, прогон не остановлен.
    pub fn new() -> Self {
        Self::default()
    }

    /// Просит остановить прогон (виден всем копиям токена).
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Остановка запрошена?
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// Ошибка прогона. Ошибка работы и ошибка GPU не сливаются; в обеих — сколько кадров успело завершиться.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SaturationError {
    /// Настройки неверны; GPU не трогали.
    #[error("неверные настройки стенда насыщения: {0}")]
    InvalidConfig(#[from] SaturationConfigError),
    /// Не удалось создать ресурсы стенда или он непригоден после прошлого сбоя.
    #[error("не удалось подготовить стенд насыщения: {0}")]
    Setup(GpuError),
    /// Работа не смогла записать кадр. Кадр не отправлен, отправленные дождались.
    #[error("работа не записала кадр (завершено кадров: {frames_completed}): {source}")]
    Workload {
        /// Ошибка работы.
        source: GpuError,
        /// Сколько кадров завершилось до ошибки (прогрев + окно + хвост).
        frames_completed: u64,
    },
    /// Отправка или ожидание GPU не удались (в том числе таймаут fence).
    #[error("ошибка GPU при прогоне (завершено кадров: {frames_completed}): {source}")]
    Gpu {
        /// Ошибка GPU.
        source: GpuError,
        /// Сколько кадров завершилось до ошибки.
        frames_completed: u64,
    },
}

/// Почему цикл кадров остановился раньше срока (внутренний тип; наружу — [`SaturationError`]).
enum LoopFailure {
    Workload(GpuError),
    Gpu(GpuError),
}

/// Стенд насыщения: гоняет [`SaturationWorkload`] без пауз и меряет пропускную способность.
pub struct SaturationBench {
    config: SaturationConfig,
    slots: SlotRing,
    /// После неудачного дожидания кадров в полёте слоты нельзя переиспользовать (GPU мог их ещё читать).
    unusable: bool,
}

impl SaturationBench {
    /// Проверяет настройки (ДО любых команд GPU) и создаёт слоты.
    pub fn new(context: &GpuContext, config: SaturationConfig) -> Result<Self, SaturationError> {
        config.validate()?;
        let slots = SlotRing::new(context, config.frames_in_flight).map_err(SaturationError::Setup)?;
        Ok(Self {
            config,
            slots,
            unusable: false,
        })
    }

    /// Настройки, с которыми создан стенд.
    pub fn config(&self) -> &SaturationConfig {
        &self.config
    }

    /// Гоняет `workload` в режиме насыщения и возвращает итог. Блокирует поток до конца прогона.
    ///
    /// Прогон: прогрев → окно замера → дожидание хвоста очереди. `cancel` проверяется на каждом кадре и
    /// во время ожидания fence: остановка не дольше одного кадра (и ≤ 20 мс ожидания).
    pub fn run(
        &mut self,
        workload: &mut dyn SaturationWorkload,
        cancel: &CancelToken,
    ) -> Result<SaturationReport, SaturationError> {
        if self.unusable {
            return Err(SaturationError::Setup(GpuError::InvalidState {
                what: "прошлый прогон не дождался кадров в полёте: слоты стенда нельзя использовать".to_owned(),
            }));
        }
        let mut tally = FrameTally::new(&self.config, Instant::now());
        let loop_result = self.run_frame_loop(workload, cancel, &mut tally);
        // Хвост очереди дожидаемся при любом исходе: ничего не уничтожается, пока GPU работает.
        let drain_result = self.drain_in_flight(workload, &mut tally);
        let frames_completed = tally.total_completed();
        match (loop_result, drain_result) {
            (Err(LoopFailure::Workload(source)), _) => Err(SaturationError::Workload {
                source,
                frames_completed,
            }),
            (Err(LoopFailure::Gpu(source)), _) | (Ok(_), Err(source)) => Err(SaturationError::Gpu {
                source,
                frames_completed,
            }),
            (Ok(end_reason), Ok(())) => Ok(tally.finish(end_reason)),
        }
    }

    /// Основной цикл: пока идёт окно, держит GPU занятым. Возвращает причину нормального окончания.
    fn run_frame_loop(
        &mut self,
        workload: &mut dyn SaturationWorkload,
        cancel: &CancelToken,
        tally: &mut FrameTally,
    ) -> Result<EndReason, LoopFailure> {
        let mut frame_index: u64 = 0;
        loop {
            if cancel.is_cancelled() {
                tally.close_window_at(Instant::now());
                return Ok(EndReason::Cancelled);
            }
            if Instant::now() >= tally.window_end() {
                return Ok(EndReason::Completed);
            }
            self.collect_signaled_frames(workload, tally)?;
            if !self.slots.has_free_slot() {
                let wait = self
                    .slots
                    .wait_oldest(self.config.fence_timeout, || cancel.is_cancelled())
                    .map_err(LoopFailure::Gpu)?;
                match wait {
                    OldestWait::Signaled => self.complete_oldest(workload, tally),
                    OldestWait::Interrupted => {
                        tally.close_window_at(Instant::now());
                        return Ok(EndReason::Cancelled);
                    }
                }
            }
            self.record_and_submit(workload, frame_index)?;
            frame_index += 1;
        }
    }

    /// Забирает все уже готовые кадры (без ожидания), чтобы момент завершения не отставал от GPU.
    fn collect_signaled_frames(
        &mut self,
        workload: &mut dyn SaturationWorkload,
        tally: &mut FrameTally,
    ) -> Result<(), LoopFailure> {
        while self.slots.oldest_is_signaled().map_err(LoopFailure::Gpu)? {
            self.complete_oldest(workload, tally);
        }
        Ok(())
    }

    /// Самый старый кадр завершён (fence наблюдён сигнальным): учёт по часам и уведомление работы.
    fn complete_oldest(&mut self, workload: &mut dyn SaturationWorkload, tally: &mut FrameTally) {
        // Момент снимаем ДО уведомления работы: её чтение результатов не должно сдвигать время завершения.
        let observed_at = Instant::now();
        if let Some(slot) = self.slots.release_oldest() {
            tally.record_completion(observed_at);
            workload.release_slot_after_fence(slot);
        }
    }

    /// Записывает кадр работы в свободный слот и отправляет его.
    fn record_and_submit(
        &mut self,
        workload: &mut dyn SaturationWorkload,
        frame_index: u64,
    ) -> Result<(), LoopFailure> {
        let Some(slot) = self.slots.free_slot() else {
            return Err(LoopFailure::Gpu(GpuError::InvalidState {
                what: "после ожидания нет свободного слота".to_owned(),
            }));
        };
        let command_buffer = self.slots.begin_recording(slot).map_err(LoopFailure::Gpu)?;
        workload
            .record_frame(FrameRecording {
                command_buffer,
                slot,
                frame_index,
            })
            .map_err(LoopFailure::Workload)?;
        self.slots.end_and_submit(slot).map_err(LoopFailure::Gpu)
    }

    /// Дожидается всех кадров в полёте (с таймаутом fence, без отмены) и уведомляет работу о каждом.
    fn drain_in_flight(
        &mut self,
        workload: &mut dyn SaturationWorkload,
        tally: &mut FrameTally,
    ) -> Result<(), GpuError> {
        while self.slots.in_flight_count() > 0 {
            // Хвост дожидаемся даже при отмене: уничтожать ресурсы под работающим GPU нельзя.
            let wait = self
                .slots
                .wait_oldest(self.config.fence_timeout, || false)
                .inspect_err(|_| self.unusable = true)?;
            debug_assert_eq!(wait, OldestWait::Signaled, "без отмены ожидание не прерывается");
            self.complete_oldest(workload, tally);
        }
        Ok(())
    }
}
