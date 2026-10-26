//! Разовые GPU-диагностики на настоящем Vulkan (только Android): `ahb_roundtrip` и `fence_order_check`.
//!
//! Диагностики работают на СВОЁМ `GpuContext` профиля `AndroidDisplay` (тот же режим validation,
//! что у потока рендера), отдельном от показа кадров. Контекст создаётся при первой проверке и
//! живёт до конца процесса — так же, как будет жить контекст продукта, через который в сессии 05
//! пойдёт импорт кадров камеры.
//!
//! Почему не «контекст на проверку» (факт сессии 03, `user/general_problems/general_backlog.md`, п. 2):
//! на телефоне под validation layer уничтожение instance, через который импортировались
//! AHardwareBuffer, примерно в каждом третьем–втором запуске убивало поток прямо в
//! `vkDestroyInstance` (стек `simpleperf`: драйвер Adreno, деструктор `BsQtiMapperExtensions`),
//! без паники и без падения процесса. С долгоживущим контекстом — 8 из 8 запусков без сбоя.
//! Здесь только перевод типов `gpu-vulkan` в нейтральные структуры [`crate::gpu_services`] — решений нет.

use std::sync::Mutex;

use gpu_vulkan::android::diagnostics::{AhbRoundtripOutcome, AhbRoundtripParams, ahb_roundtrip};
use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::hardware_buffer_cache::AcquireOutcome;
use gpu_vulkan::hardware_buffer_cache::raw_reference::RawPixelFormat;
use gpu_vulkan::sync::diagnostics::{FenceOrderParams, fence_order_check};
use gpu_vulkan::validation::ValidationMode;

use crate::gpu_services::{
    AhbCheckRequest, AhbCheckResult, AhbCheckSummary, AhbFormat, FenceCheckRequest, FenceCheckResult,
    FenceCheckSummary, GpuDiagnostics, ValidationState,
};

/// Диагностики поверх `gpu-vulkan`.
pub struct VulkanGpuDiagnostics {
    validation: ValidationMode,
    /// Контекст диагностик: `None` до первой проверки, дальше — до конца процесса (см. модуль).
    /// Замок держится только на время создания; сами проверки работают с клоном.
    context: Mutex<Option<GpuContext>>,
}

impl VulkanGpuDiagnostics {
    pub fn new(validation: ValidationMode) -> Self {
        Self {
            validation,
            context: Mutex::new(None),
        }
    }

    /// Контекст диагностик: поднимается при первом вызове и переиспользуется. Неудача создания
    /// не запоминается — следующая проверка попробует снова; текст ошибки идёт в отчёт.
    fn open_context(&self) -> Result<GpuContext, String> {
        let mut slot = self
            .context
            .lock()
            .map_err(|_| "замок контекста диагностик отравлен прошлой паникой".to_owned())?;
        if let Some(context) = slot.as_ref() {
            return Ok(context.clone());
        }
        let context = GpuContext::new(&GpuContextConfig::new(
            DeviceProfile::AndroidDisplay,
            self.validation.clone(),
        ))
        .map_err(|error| format!("GPU не поднялся: {error}"))?;
        *slot = Some(context.clone());
        Ok(context)
    }

    /// Одна проверка импорта AHB на уже поднятом контексте.
    fn check_one(&self, context: &GpuContext, request: AhbCheckRequest) -> AhbCheckResult {
        let params = AhbRoundtripParams {
            width: request.width,
            height: request.height,
            format: match request.format {
                AhbFormat::Raw16 => RawPixelFormat::Raw16,
                AhbFormat::Raw10 => RawPixelFormat::Raw10,
            },
        };
        match ahb_roundtrip(context, params) {
            Ok(AhbRoundtripOutcome::Completed(report)) => AhbCheckResult::Completed(AhbCheckSummary {
                matched: report.matched,
                second_import_hit: report.second_import == AcquireOutcome::Hit,
                live_objects_restored: report.live_objects_restored,
                validation_errors: report.validation_errors_during,
                known_deviations: report.known_deviations_during.clone(),
                validation: ValidationState::of(&self.validation),
                details: report.to_string(),
            }),
            Ok(AhbRoundtripOutcome::AllocationUnsupported { status, requested }) => {
                AhbCheckResult::AllocationUnsupported {
                    status,
                    requested: format!("{requested:?}"),
                }
            }
            Err(error) => AhbCheckResult::Failed {
                reason: error.to_string(),
            },
        }
    }
}

impl GpuDiagnostics for VulkanGpuDiagnostics {
    fn ahb_roundtrips(
        &self,
        requests: &[AhbCheckRequest],
        on_checked: &mut dyn FnMut(&AhbCheckRequest),
    ) -> Vec<AhbCheckResult> {
        // Один контекст на всю пачку (см. документацию трейта): без лишнего пересоздания instance.
        let context = match self.open_context() {
            Ok(context) => context,
            Err(reason) => {
                return requests
                    .iter()
                    .map(|request| {
                        on_checked(request);
                        AhbCheckResult::Failed { reason: reason.clone() }
                    })
                    .collect();
            }
        };
        requests
            .iter()
            .map(|request| {
                let result = self.check_one(&context, *request);
                on_checked(request);
                result
            })
            .collect()
    }

    fn fence_order(&self, request: FenceCheckRequest) -> FenceCheckResult {
        let context = match self.open_context() {
            Ok(context) => context,
            Err(reason) => return FenceCheckResult::Failed { reason },
        };
        let params = FenceOrderParams {
            busy_iterations: request.busy_iterations,
            ..FenceOrderParams::default()
        };
        // Контекст живёт дольше одной проверки: validation считаем приростом за эту проверку.
        let validation_before = context.validation_report();
        match fence_order_check(&context, params) {
            Ok(report) => {
                // Один снимок после: два чтения подряд могли бы разойтись, если слой успел выдать
                // сообщение между ними (число ошибок и их разбор относились бы к разным моментам).
                let validation_after = context.validation_report();
                FenceCheckResult::Completed(FenceCheckSummary {
                    expected_value: report.expected_value,
                    work_a_value: report.work_a_value,
                    work_b_value: report.work_b_value,
                    b_started_after_a_finished: report.b_started_after_a_finished,
                    work_a_duration_ns: report.work_a_duration_ns,
                    work_b_duration_ns: report.work_b_duration_ns,
                    gap_between_a_end_and_b_begin_ns: report.gap_between_a_end_and_b_begin_ns,
                    release_fence_pending_at_export: report.release_fence_pending_at_export,
                    exported_fence_was_already_signaled: report.exported_fence_was_already_signaled,
                    already_signaled_wait_semaphore_created: report.already_signaled_phase.wait_semaphore_created,
                    already_signaled_work_completed: report.already_signaled_phase.work_completed,
                    already_signaled_result_value: report.already_signaled_phase.result_value,
                    validation_errors: validation_after.errors.saturating_sub(validation_before.errors),
                    known_deviations: validation_after.known_deviations_since(&validation_before),
                    validation: ValidationState::of(&self.validation),
                })
            }
            Err(error) => FenceCheckResult::Failed {
                reason: error.to_string(),
            },
        }
    }
}
