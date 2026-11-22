//! Помощники GPU-тестов внутри crate: контекст с полной validation и политика «нет GPU».

use super::{GpuContext, GpuContextConfig};
use crate::device::DeviceProfile;
use crate::validation::ValidationMode;

/// Имя переменной окружения, разрешающей пропуск GPU-тестов на машине без GPU.
pub(crate) const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Контекст `Headless` с полной validation. Нет GPU → тест ПАДАЕТ с понятным текстом,
/// если только `FASTICAMERA_SKIP_GPU_TESTS=1` (тогда печатается явная причина пропуска и `None`).
pub(crate) fn gpu_context_or_skip() -> Option<GpuContext> {
    gpu_context_with_validation_or_skip(ValidationMode::FULL)
}

/// То же, но с заданным режимом validation (например, со списком известных отступлений).
pub(crate) fn gpu_context_with_validation_or_skip(validation: ValidationMode) -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::Headless, validation);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
                None
            } else {
                panic!("GPU недоступен: {error}. Поставьте {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно");
            }
        }
    }
}

/// Контекст профиля `HeadlessPresentation` (с `VK_EXT_headless_surface`) с полной validation: для тестов,
/// которым нужен `VkSurfaceKHR` без окна. Политика «нет GPU» та же, что у [`gpu_context_or_skip`].
pub(crate) fn presentation_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::HeadlessPresentation, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
                None
            } else {
                panic!("GPU недоступен: {error}. Поставьте {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно");
            }
        }
    }
}
