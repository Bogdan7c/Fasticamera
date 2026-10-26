//! Диагностика порядка fence: «работа A → sync_fd → работа B».
//!
//! Что доказывает. Работа A — долгий compute (цикл заданной длины), после которой сигналится
//! экспортируемый семафор. Из него достаётся `sync_fd`, он превращается в [`GpuWaitFence`],
//! импортируется обратно в семафор, и работа B ждёт его, читает итог A и копирует в буфер
//! результата. Метки времени GPU на начале/конце обеих работ показывают, что B стартовала не
//! раньше конца A, а значение в буфере результата совпадает с известным итогом A. Вторая фаза
//! проверяет путь «fence уже сигнален»: семафор не создаётся, работа проходит без ожидания.
//!
//! Функция одинакова на ПК (RADV) и на телефоне (Adreno): smoke-сценарий `gpu-fence` вызывает её
//! и печатает отчёт.
//!
//! Нет 64-битных атомиков (Adreno), поэтому весь обмен — 32-битные значения.

mod host_buffer;
mod rig;
mod timestamps;

use ash::vk;

use self::rig::DiagnosticRig;
pub use self::timestamps::TimestampAnalysis;
use self::timestamps::analyze_timestamps;
use super::{
    ExportableSemaphore, FenceImportError, FenceWait, QueueSubmission, WaitSemaphore, import_wait_fence,
    wait_release_fence_on_cpu,
};
use crate::context::GpuContext;
use crate::error::{GpuError, run_step};
use crate::live_objects::OwnedFence;
use shell_contract::sync_fence::{GpuReleaseFence, GpuWaitFence};

/// Стадия, на которой B ждёт A и на которой ставятся метки: compute shader.
const WORK_STAGE: vk::PipelineStageFlags2 = vk::PipelineStageFlags2::COMPUTE_SHADER;

/// Значение, которым заполняется буфер A перед запуском: если B прочитает его, а не итог A,
/// порядок нарушен.
const WORK_A_BUFFER_SENTINEL: u32 = 0xDEAD_BEEF;

/// Значение, которым заполняется буфер результата перед запуском каждой фазы.
const RESULT_BUFFER_SENTINEL: u32 = 0xFFFF_FFFF;

/// Зависит ли работа B от работы A. `NoDependency` — контрольный прогон: показывает, что
/// происходит без ожидания (на однóй очереди драйвер может сериализовать работу и сам).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkBDependency {
    /// B ждёт импортированный `sync_fd` работы A (рабочий режим).
    WaitsOnWorkA,
    /// B не ждёт ничего (контроль).
    NoDependency,
}

/// Параметры диагностики.
#[derive(Debug, Clone, Copy)]
pub struct FenceOrderParams {
    /// Длина цикла работы A. Чем больше, тем дольше A; подобрано так, чтобы на RADV 780M
    /// A занимала заметно больше миллисекунды (см. [`DEFAULT_BUSY_ITERATIONS`]).
    pub busy_iterations: u32,
    /// Начальное состояние генератора в цикле A (чтобы итог не был константой).
    pub busy_seed: u32,
    /// Ждёт ли B работу A.
    pub work_b_dependency: WorkBDependency,
}

/// Число итераций по умолчанию: ~14 мс на RADV (780M) в сборке с validation
/// (точное время фиксируется отчётом `work_a_duration_ns`).
pub const DEFAULT_BUSY_ITERATIONS: u32 = 500_000;

impl Default for FenceOrderParams {
    fn default() -> Self {
        Self {
            busy_iterations: DEFAULT_BUSY_ITERATIONS,
            busy_seed: 0x1234_5678,
            work_b_dependency: WorkBDependency::WaitsOnWorkA,
        }
    }
}

/// Итог фазы «fence уже сигнален».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AlreadySignaledPhase {
    /// Создавался ли семафор для ожидания (должно быть `false`: ждать нечего).
    pub wait_semaphore_created: bool,
    /// Завершилась ли работа B (fence дождались в пределах таймаута).
    pub work_completed: bool,
    /// Значение, которое B записала в буфер результата.
    pub result_value: u32,
}

/// Отчёт диагностики порядка fence.
#[derive(Debug, Clone, PartialEq)]
pub struct FenceOrderReport {
    /// Итог цикла A, посчитанный на CPU ([`busy_loop_expected_value`]).
    pub expected_value: u32,
    /// Что A записала в свой буфер (прочитано с GPU).
    pub work_a_value: u32,
    /// Что B скопировала в буфер результата.
    pub work_b_value: u32,
    /// Метки времени в наносекундах относительно начала A и вывод о порядке.
    pub timestamps: TimestampAnalysis,
    /// `true`, если B началась не раньше конца A (вывод `timestamps`, вынесен для удобства).
    pub b_started_after_a_finished: bool,
    /// Длительность работы A, нс.
    pub work_a_duration_ns: u64,
    /// Длительность работы B, нс.
    pub work_b_duration_ns: u64,
    /// Зазор между концом A и началом B, нс (отрицательный — пересечение).
    pub gap_between_a_end_and_b_begin_ns: i64,
    /// Режим зависимости, с которым шёл прогон.
    pub work_b_dependency: WorkBDependency,
    /// Экспортированный fence сразу после экспорта ещё не был сигнален (`Some(true)`: это
    /// настоящий незавершённый `sync_fd`); `None` — проверить нельзя (нет `VK_KHR_external_fence_fd`).
    pub release_fence_pending_at_export: Option<bool>,
    /// Драйвер вернул «уже сигнален» (`-1`) вместо дескриптора.
    pub exported_fence_was_already_signaled: bool,
    /// Итог второй фазы (путь «fence уже сигнален»).
    pub already_signaled_phase: AlreadySignaledPhase,
}

/// Ошибка диагностики.
#[derive(Debug, thiserror::Error)]
pub enum FenceOrderError {
    /// Сбой Vulkan/таймаут/неподдерживаемая возможность.
    #[error(transparent)]
    Gpu(#[from] GpuError),
    /// Не удалось импортировать только что экспортированный fence (сам fence — в ошибке).
    #[error(transparent)]
    FenceImport(#[from] FenceImportError),
}

/// Итог цикла работы A, посчитанный на CPU той же формулой, что в `fence_busy.comp`
/// (линейный конгруэнтный генератор по модулю `2^32`).
pub fn busy_loop_expected_value(seed: u32, iterations: u32) -> u32 {
    (0..iterations).fold(seed, |state, _| {
        state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
    })
}

/// Запускает диагностику порядка fence. См. описание модуля.
///
/// Требует метки времени на очереди и `VK_KHR_external_semaphore_fd` (оба обязательны в профилях
/// проекта). Если после отправки работы что-то пошло не так, объекты стенда **не уничтожаются**
/// (утечка, как в `run_one_shot`): GPU мог ещё ими пользоваться.
pub fn fence_order_check(context: &GpuContext, params: FenceOrderParams) -> Result<FenceOrderReport, FenceOrderError> {
    let Some(timestamp_support) = context.capabilities().timestamps else {
        return Err(GpuError::Unsupported {
            what: "метки времени GPU на очереди".to_owned(),
        }
        .into());
    };
    let mut run = FenceOrderRun::new(context)?;
    match run.execute(params, timestamp_support.valid_bits, timestamp_support.period_ns) {
        Ok(report) => Ok(report),
        Err(error) => {
            if run.work_was_submitted {
                tracing::error!(
                    target: "fasticamera::gpu",
                    %error,
                    "fence_order_check упал после отправки работы; объекты стенда оставлены (утечка)"
                );
                std::mem::forget(run);
            }
            Err(error)
        }
    }
}

/// Стенд и состояние одного прогона.
struct FenceOrderRun {
    rig: DiagnosticRig,
    export_semaphore: ExportableSemaphore,
    context: GpuContext,
    /// Отправлена ли хоть одна работа (после этого при ошибке объекты нельзя уничтожать).
    work_was_submitted: bool,
}

impl FenceOrderRun {
    fn new(context: &GpuContext) -> Result<Self, GpuError> {
        Ok(Self {
            rig: DiagnosticRig::new(context)?,
            export_semaphore: ExportableSemaphore::new(context)?,
            context: context.clone(),
            work_was_submitted: false,
        })
    }

    fn execute(
        &mut self,
        params: FenceOrderParams,
        timestamp_valid_bits: u32,
        timestamp_period_ns: f32,
    ) -> Result<FenceOrderReport, FenceOrderError> {
        self.rig.work_a_buffer.write(WORK_A_BUFFER_SENTINEL)?;
        self.rig.result_buffer.write(RESULT_BUFFER_SENTINEL)?;
        self.rig.record_work_a(params.busy_iterations, params.busy_seed)?;
        self.rig.record_work_b()?;
        self.rig.record_work_b_without_timestamps()?;

        // --- Работа A: отправка и экспорт release-fence. ---
        let mut work_a = QueueSubmission::new()
            .run_command_buffer(self.rig.command_buffers.work_a)
            .signal_for_export(&self.export_semaphore, WORK_STAGE)
            .submit(&self.context)?;
        self.work_was_submitted = true;
        let release_fence = work_a
            .take_release_signal()
            .ok_or(GpuError::InvalidState {
                what: "сабмит с экспортом не выдал токен".to_owned(),
            })?
            .export()?;
        let exported_fence_was_already_signaled = release_fence.is_already_signaled();
        // 0 мс: «сигнален ли fence прямо сейчас». A длится миллисекунды, значит, настоящий fence — нет.
        let release_fence_pending_at_export =
            match wait_release_fence_on_cpu(&self.context, &release_fence, std::time::Duration::ZERO) {
                Ok(wait) => Some(wait == FenceWait::TimedOut),
                Err(GpuError::ExtensionNotEnabled { .. }) => None,
                Err(error) => return Err(error.into()),
            };

        // --- Работа B: ждёт импортированный fence (или контроль без ожидания). ---
        let wait_semaphore = match params.work_b_dependency {
            WorkBDependency::WaitsOnWorkA => import_wait_fence(&self.context, into_wait_fence(release_fence))?,
            WorkBDependency::NoDependency => {
                drop(release_fence);
                WaitSemaphore::NoWait
            }
        };
        let work_b_fence = create_fence(&self.context)?;
        let _work_b = QueueSubmission::new()
            .wait_for_imported_fence(&wait_semaphore, WORK_STAGE)
            .run_command_buffer(self.rig.command_buffers.work_b)
            .signal_fence(&work_b_fence)
            .submit(&self.context)?;
        self.wait_for_fence(&work_b_fence, "ожидание работы B")?;

        let work_a_value = self.rig.work_a_buffer.read()?;
        let work_b_value = self.rig.result_buffer.read()?;
        let analysis = analyze_timestamps(
            self.rig.read_timestamp_ticks()?,
            timestamp_valid_bits,
            timestamp_period_ns,
        );

        // --- Фаза 2: «fence уже сигнален» — ждать нечего. ---
        let already_signaled_phase = self.run_already_signaled_phase()?;

        Ok(FenceOrderReport {
            expected_value: busy_loop_expected_value(params.busy_seed, params.busy_iterations),
            work_a_value,
            work_b_value,
            timestamps: analysis,
            b_started_after_a_finished: analysis.b_started_after_a_finished,
            work_a_duration_ns: analysis.work_a_end_ns - analysis.work_a_begin_ns,
            work_b_duration_ns: analysis.work_b_end_ns.saturating_sub(analysis.work_b_begin_ns),
            gap_between_a_end_and_b_begin_ns: analysis.work_b_begin_ns as i64 - analysis.work_a_end_ns as i64,
            work_b_dependency: params.work_b_dependency,
            release_fence_pending_at_export,
            exported_fence_was_already_signaled,
            already_signaled_phase,
        })
    }

    /// Вторая фаза: B' (копирование без меток) с `GpuWaitFence::already_signaled()`.
    /// Буфер A к этому моменту уже содержит итог (фаза 1 дождалась GPU).
    fn run_already_signaled_phase(&mut self) -> Result<AlreadySignaledPhase, FenceOrderError> {
        self.rig.result_buffer.write(RESULT_BUFFER_SENTINEL)?;
        let wait_semaphore = import_wait_fence(&self.context, GpuWaitFence::already_signaled())?;
        let wait_semaphore_created = !matches!(wait_semaphore, WaitSemaphore::NoWait);
        let fence = create_fence(&self.context)?;
        let _submitted = QueueSubmission::new()
            .wait_for_imported_fence(&wait_semaphore, WORK_STAGE)
            .run_command_buffer(self.rig.command_buffers.work_b_without_wait)
            .signal_fence(&fence)
            .submit(&self.context)?;
        self.wait_for_fence(&fence, "ожидание работы B без ожидания fence")?;
        Ok(AlreadySignaledPhase {
            wait_semaphore_created,
            work_completed: true,
            result_value: self.rig.result_buffer.read()?,
        })
    }

    /// Ждёт `VkFence` не дольше таймаута контекста; таймаут — `GpuError::Timeout`.
    fn wait_for_fence(&self, fence: &OwnedFence, step: &'static str) -> Result<(), GpuError> {
        let timeout = super::cpu_wait::timeout_nanos(self.context.submit_wait_timeout());
        // SAFETY: fence принадлежит этому устройству.
        match unsafe { self.context.device().wait_for_fences(&[fence.handle()], true, timeout) } {
            Ok(()) => Ok(()),
            Err(vk::Result::TIMEOUT) => Err(GpuError::Timeout { step }),
            Err(result) => Err(GpuError::Vulkan { step, result }),
        }
    }
}

fn create_fence(context: &GpuContext) -> Result<OwnedFence, GpuError> {
    // SAFETY: устройство валидно; create_info — локальная структура.
    let handle = run_step(
        unsafe { context.device().create_fence(&vk::FenceCreateInfo::default(), None) },
        "vkCreateFence",
    )?;
    Ok(OwnedFence::from_raw(context, handle))
}

/// Превращает release-fence работы A в wait-fence для работы B (дескриптор переезжает целиком).
fn into_wait_fence(release: GpuReleaseFence) -> GpuWaitFence {
    match release.into_owned_fd() {
        Some(fd) => GpuWaitFence::from_sync_fd(fd),
        None => GpuWaitFence::already_signaled(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_loop_expected_value_matches_hand_computed_steps() {
        // 0 итераций — само начальное значение.
        assert_eq!(busy_loop_expected_value(7, 0), 7);
        // Одна итерация: 7 * 1664525 + 1013904223 (по модулю 2^32).
        assert_eq!(
            busy_loop_expected_value(7, 1),
            7_u32.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
        );
        // Две итерации идут последовательно, а не повторяют первую.
        let first = busy_loop_expected_value(7, 1);
        assert_eq!(
            busy_loop_expected_value(7, 2),
            first.wrapping_mul(1_664_525).wrapping_add(1_013_904_223)
        );
    }
}
