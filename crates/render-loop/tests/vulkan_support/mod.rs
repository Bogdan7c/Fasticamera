//! Общие помощники GPU-тестов потока рендера: бэкенд на настоящем драйвере с полной
//! validation, часы vsync, которые публикует отдельный поток, и проверка «ничего не утекло».
//!
//! Политика «нет GPU»: тест ПАДАЕТ с понятным сообщением, если только не задано
//! `FASTICAMERA_SKIP_GPU_TESTS=1` — тогда печатается явная причина пропуска.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]
#![allow(dead_code, reason = "каждый тестовый файл использует свою часть помощников")]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use gpu_vulkan::context::GpuContext;
use gpu_vulkan::validation::ValidationMode;
use render_loop::config::RenderLoopConfig;
use render_loop::thread::VsyncFrameClock;
use render_loop::vulkan_backend::{VulkanBackendConfig, VulkanDisplayBackend};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;
use shell_contract::vsync::{VsyncSlot, VsyncSubscription, VsyncTick};

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";

/// Нечётные размеры ловят ошибки округления и перепутанные оси.
pub const START_SIZE: SurfaceSize = SurfaceSize {
    width_px: 321,
    height_px: 201,
};
pub const RESIZED: SurfaceSize = SurfaceSize {
    width_px: 257,
    height_px: 145,
};

/// Период ~120 Гц, наносекунды.
const VSYNC_PERIOD: Duration = Duration::from_nanos(8_333_333);

/// Бэкенд ПК-стенда с полной validation или `None` при осознанном пропуске.
pub fn headless_backend_or_skip(render_config: &RenderLoopConfig) -> Option<VulkanDisplayBackend> {
    let config = VulkanBackendConfig::from_render_config(render_config, ValidationMode::FULL)
        .expect("конфиг бэкенда из конфига потока");
    match VulkanDisplayBackend::new_headless_presentation(config) {
        Ok(backend) => Some(backend),
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

/// Поток-производитель тиков vsync ~120 Гц. Остановка и join — при уничтожении.
pub struct VsyncPublisher {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for VsyncPublisher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            assert!(join.join().is_ok(), "поток vsync-производителя упал");
        }
    }
}

/// Часы кадров поверх настоящего `VsyncSlot`, в который тики публикует отдельный поток.
pub fn vsync_clock_at_120_hz() -> VsyncFrameClock {
    let slot = Arc::new(VsyncSlot::new());
    let stop = Arc::new(AtomicBool::new(false));
    let producer_slot = Arc::clone(&slot);
    let producer_stop = Arc::clone(&stop);
    let join = thread::Builder::new()
        .name("test-vsync".to_owned())
        .spawn(move || {
            let started = Instant::now();
            let mut sequence = VsyncSlot::FIRST_SEQUENCE;
            while !producer_stop.load(Ordering::SeqCst) {
                let frame_time = i64::try_from(started.elapsed().as_nanos()).unwrap_or(i64::MAX);
                let period = i64::try_from(VSYNC_PERIOD.as_nanos()).unwrap_or(i64::MAX);
                producer_slot.publish(VsyncTick {
                    sequence,
                    frame_time: MonotonicNanos::from_nanos(frame_time),
                    expected_present: MonotonicNanos::from_nanos(frame_time + 2 * period),
                    deadline: MonotonicNanos::from_nanos(frame_time + period),
                    vsync_id: i64::try_from(sequence).unwrap_or(0),
                });
                sequence += 1;
                thread::sleep(VSYNC_PERIOD);
            }
        })
        .expect("поток vsync-производителя");
    let publisher = VsyncPublisher { stop, join: Some(join) };
    VsyncFrameClock::new(VsyncSubscription::new(slot, Box::new(publisher)))
}

/// Финальная проверка: GPU простаивает, validation не сообщила ни одной ошибки.
pub fn assert_no_validation_errors(context: &GpuContext) {
    context.wait_idle().expect("wait_idle в конце теста");
    let report = context.validation_report();
    assert_eq!(report.errors, 0, "validation нашла ошибки: {report:?}");
}
