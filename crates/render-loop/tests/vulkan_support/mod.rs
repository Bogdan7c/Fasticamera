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

use camera_contract::three_a::{DirectiveSlot, WhiteBalanceDirective};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::validation::ValidationMode;
use raw_frame_stream::ViewfinderFrames;
use render_loop::config::RenderLoopConfig;
use render_loop::thread::VsyncFrameClock;
use render_loop::vulkan_backend::{CameraImporterFactory, VulkanBackendConfig, VulkanDisplayBackend};
use shell_contract::clock::MonotonicNanos;
use shell_contract::surface::SurfaceSize;
use shell_contract::vsync::{FrameTimelines, VsyncSlot, VsyncSubscription, VsyncTick, VsyncTimeline};

const SKIP_ENV: &str = "FASTICAMERA_SKIP_GPU_TESTS";
/// Окружение второго режима стенда: `skip` — весь набор идёт под `SkipWhenUnchanged`.
const UNCHANGED_POLICY_ENV: &str = "FASTICAMERA_TEST_UNCHANGED_POLICY";

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

/// Настройки бэкенда ПК-стенда: полная validation и заготовка вариантов ISP на потоке рендера. Фоновая заготовка
/// (умолчание продукта) делала бы число пайплайнов и живых объектов зависимым от времени, а тесты их считают.
fn stand_backend_config(render_config: &RenderLoopConfig) -> Result<VulkanBackendConfig, gpu_vulkan::error::GpuError> {
    let mut config = VulkanBackendConfig::from_render_config(render_config, ValidationMode::FULL)?;
    config.viewfinder.isp.variant_prewarm = isp_vulkan::viewfinder::VariantPrewarm::OnRenderThread;
    Ok(config)
}

/// Бэкенд ПК-стенда с полной validation или `None` при осознанном пропуске.
pub fn headless_backend_or_skip(render_config: &RenderLoopConfig) -> Option<VulkanDisplayBackend> {
    let config = stand_backend_config(render_config).expect("конфиг бэкенда из конфига потока");
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

/// Бэкенд ПК-стенда, показывающий кадры камеры (импорт — через `make_importer`), с полной validation
/// или `None` при осознанном пропуске.
pub fn headless_camera_backend_or_skip(
    render_config: &RenderLoopConfig,
    frames: ViewfinderFrames,
    make_importer: CameraImporterFactory,
) -> Option<VulkanDisplayBackend> {
    headless_camera_backend_with_directives_or_skip(render_config, frames, make_importer, DirectiveSlot::new())
}

/// То же, но с «почтовым ящиком» указаний AWB, которым управляет тест (писатель — у теста).
///
/// Политика пропуска тиков берётся из `render_config` (умолчание — `AlwaysRedraw`, решение 214). Для прогона ВСЕГО
/// старого набора под пропуском задайте окружение `FASTICAMERA_TEST_UNCHANGED_POLICY=skip`: помощник подменит политику
/// на `SkipWhenUnchanged` (второй режим стенда; так найдены тесты, которым нужен `Presented` на каждом тике).
pub fn headless_camera_backend_with_directives_or_skip(
    render_config: &RenderLoopConfig,
    frames: ViewfinderFrames,
    make_importer: CameraImporterFactory,
    directives: DirectiveSlot<WhiteBalanceDirective>,
) -> Option<VulkanDisplayBackend> {
    let mut config = render_config.clone();
    if std::env::var(UNCHANGED_POLICY_ENV).as_deref() == Ok("skip") {
        config.unchanged_frame.policy = render_loop::config::UnchangedFramePolicy::SkipWhenUnchanged;
    }
    headless_camera_backend_keeping_unchanged_policy_or_skip(&config, frames, make_importer, directives)
}

/// Бэкенд камеры ПК-стенда с политикой пропуска тиков РОВНО из `render_config` (по умолчанию пропуск включён).
pub fn headless_camera_backend_keeping_unchanged_policy_or_skip(
    render_config: &RenderLoopConfig,
    frames: ViewfinderFrames,
    make_importer: CameraImporterFactory,
    directives: DirectiveSlot<WhiteBalanceDirective>,
) -> Option<VulkanDisplayBackend> {
    let config = stand_backend_config(render_config).expect("конфиг бэкенда из конфига потока");
    match VulkanDisplayBackend::new_headless_camera_preview(config, frames, make_importer, directives) {
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

/// Тик показа стенда вернул `Presented` — или `SkippedUnchanged`, когда набор идёт под пропуском тиков без изменений
/// (`FASTICAMERA_TEST_UNCHANGED_POLICY=skip`). Любой другой исход — провал: стенд ждёт, что тик дошёл до показа либо
/// осознанно пропущен как «ничего не изменилось», но не ошибку, таймаут или пересоздание.
pub fn assert_presented_or_skipped(outcome: render_loop::backend::PresentOutcome) {
    use render_loop::backend::PresentOutcome;
    let skipping = std::env::var(UNCHANGED_POLICY_ENV).as_deref() == Ok("skip");
    let allowed = outcome == PresentOutcome::Presented || (skipping && outcome == PresentOutcome::SkippedUnchanged);
    assert!(
        allowed,
        "тик стенда: {outcome:?} (пропуск тиков включён окружением: {skipping})"
    );
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
                producer_slot.publish(VsyncTick::from_timelines(
                    sequence,
                    MonotonicNanos::from_nanos(frame_time),
                    FrameTimelines::only(VsyncTimeline {
                        vsync_id: i64::try_from(sequence).unwrap_or(0),
                        expected_present: MonotonicNanos::from_nanos(frame_time + 2 * period),
                        deadline: MonotonicNanos::from_nanos(frame_time + period),
                    }),
                ));
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
