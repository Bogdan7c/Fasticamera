//! Общие помощники интеграционных GPU-тестов.
//!
//! Политика «нет GPU»: тест ПАДАЕТ с понятным сообщением, если только не задано
//! `FASTICAMERA_SKIP_GPU_TESTS=1` — тогда печатается явная причина пропуска.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

use gpu_vulkan::context::{GpuContext, GpuContextConfig};
use gpu_vulkan::device::DeviceProfile;
use gpu_vulkan::validation::ValidationMode;

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Контекст `Headless` с полной validation (включая синхронизацию) или `None` при осознанном пропуске.
pub fn gpu_context_or_skip() -> Option<GpuContext> {
    let config = GpuContextConfig::new(DeviceProfile::Headless, ValidationMode::FULL);
    match GpuContext::new(&config) {
        Ok(context) => Some(context),
        Err(error) => {
            if std::env::var(SKIP_ENV).as_deref() == Ok("1") {
                eprintln!("ПРОПУСК GPU-теста: {error} ({SKIP_ENV}=1)");
                None
            } else {
                panic!("GPU недоступен: {error}. Установите {SKIP_ENV}=1, чтобы пропустить GPU-тесты осознанно");
            }
        }
    }
}

/// Финальная проверка теста: GPU простаивает, validation не сообщила ни одной ошибки.
pub fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}
