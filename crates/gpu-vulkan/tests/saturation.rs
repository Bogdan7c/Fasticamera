//! Тесты стенда насыщения на реальном GPU (RADV, полная validation).
//!
//! Работа-заглушка (`saturation_support`) — compute-проход с известной ценой. Каждый тест проверяет наблюдаемый
//! итог: счётчики, пиксели после readback, значения итога прогона, ошибки, число живых объектов Vulkan.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале; хелперы вне #[test] clippy не считает тестовым кодом"
)]

mod common;
mod saturation_support;

use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use common::{assert_no_validation_errors, gpu_context_or_skip};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::kit_test_support::read_storage_image_rgba;
use gpu_vulkan::saturation::{
    CancelToken, EndReason, SaturationBench, SaturationConfig, SaturationConfigError, SaturationError,
    SaturationReport, Throughput,
};
use gpu_vulkan::swapchain::FramesInFlight;
use saturation_support::PatternWorkload;

/// Замер времени честен только когда GPU занят одним тестом: тесты этого файла идут строго по очереди.
static GPU_TIMING_LOCK: Mutex<()> = Mutex::new(());

fn serialize_gpu_timing() -> MutexGuard<'static, ()> {
    // Паника соседнего теста отравляет замок, но сам замок всё ещё исправен.
    GPU_TIMING_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Короткий конфиг для тестов: прогрев 100 мс, окно 400 мс, шаг 100 мс.
fn quick_config(frames_in_flight: usize) -> SaturationConfig {
    SaturationConfig {
        frames_in_flight: FramesInFlight::new(frames_in_flight).unwrap(),
        warmup: Duration::from_millis(100),
        measure: Duration::from_millis(400),
        timeline_step: Duration::from_millis(100),
        fence_timeout: Duration::from_secs(10),
    }
}

/// Прогон заглушки со стороной `side` и слотами; возвращает итог и саму заглушку (для сверки счётчиков).
fn run_pattern(context: &GpuContext, side: u32, config: SaturationConfig) -> (SaturationReport, PatternWorkload) {
    let mut workload = PatternWorkload::new(context, side, config.frames_in_flight.get());
    let mut bench = SaturationBench::new(context, config).unwrap();
    let report = bench.run(&mut workload, &CancelToken::new()).unwrap();
    (report, workload)
}

fn measured_fps(report: &SaturationReport) -> f64 {
    match report.throughput {
        Throughput::Measured { frames_per_second, .. } => frames_per_second,
        Throughput::NoFramesCompleted => panic!("в окне замера нет кадров: {report:?}"),
    }
}

#[test]
fn saturated_run_executes_every_counted_frame_on_the_gpu() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    for frames_in_flight in [1, 2, 3] {
        let side = 256;
        let (report, workload) = run_pattern(&context, side, quick_config(frames_in_flight));
        context.wait_idle().unwrap();

        assert_eq!(report.end_reason, EndReason::Completed);
        assert!(workload.violations.is_empty(), "{:?}", workload.violations);
        // Сверка счётчиков: работа получила release ровно на каждый кадр трёх корзин итога, и все они записаны.
        assert_eq!(workload.released_frames, report.total_completed_frames());
        assert_eq!(workload.recorded_frames, report.total_completed_frames());
        assert!(report.measured_frames > 0, "{report:?}");
        assert!(
            report.warmup_frames > 0,
            "прогрев 100 мс обязан завершить кадры: {report:?}"
        );
        // Уведомления идут строго по порядку отправки: кадры не теряются и не дублируются.
        let expected_order: Vec<u64> = (0..workload.recorded_frames).collect();
        assert_eq!(workload.release_order, expected_order);
        // GPU реально исполнил ВСЕ кадры: счётчик пикселей = кадры × пиксели (по модулю 2^32).
        let executed = (workload.recorded_frames * workload.pixels_per_frame()) as u32;
        assert_eq!(workload.counter_value(), executed);
        // Картинка содержит результат ПОСЛЕДНЕГО кадра: синий канал несёт его номер, красный/зелёный — градиент.
        let pixels = read_storage_image_rgba(workload.image(), workload.image_use_after_frames()).unwrap();
        let last_frame = workload.recorded_frames - 1;
        let expected_blue = PatternWorkload::expected_blue(last_frame);
        assert!(
            (pixels[0][2] - expected_blue).abs() < 2e-3,
            "{:?} против {expected_blue}",
            pixels[0]
        );
        let side_len = side as usize;
        let top_right = pixels[side_len - 1];
        assert!(
            (top_right[0] - 1.0).abs() < 2e-3 && top_right[1].abs() < 2e-3,
            "{top_right:?}"
        );
        assert_eq!(report.window, Duration::from_millis(400));
        // Шкала: сумма по шагам = итог; шагов столько, сколько помещается в окно.
        assert_eq!(report.timeline.len(), 4);
        assert_eq!(
            report.timeline.iter().map(|step| step.frames).sum::<u64>(),
            report.measured_frames
        );
        let Throughput::Measured {
            frames_per_second,
            frame_cost,
        } = report.throughput
        else {
            panic!("ожидали Measured: {report:?}");
        };
        let expected_fps = report.measured_frames as f64 / 0.4;
        assert!(
            (frames_per_second - expected_fps).abs() < 1e-6,
            "{frames_per_second} против {expected_fps}"
        );
        assert!((frame_cost.as_secs_f64() * frames_per_second - 1.0).abs() < 1e-3);
    }
    assert_no_validation_errors(&context);
    assert_eq!(
        context.live_objects(),
        baseline,
        "после drop стенда и работы объекты Vulkan должны вернуться"
    );
}

#[test]
fn heavier_work_reports_lower_throughput() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    // Сторона ×2 = площадь ×4. Размеры подобраны так, чтобы кадр был дороже накладных расходов CPU на
    // отправку (иначе обе цифры упёрлись бы в CPU и не различались).
    let light = measured_fps(&run_pattern(&context, 1024, quick_config(2)).0);
    let heavy = measured_fps(&run_pattern(&context, 2048, quick_config(2)).0);
    eprintln!("пропускная способность заглушки: 1024x1024 = {light:.1} fps, 2048x2048 = {heavy:.1} fps");
    // Идеально тяжёлая работа даёт 1/4 пропускной способности; порог 0.8 — мягкий: ловит «цифра не зависит
    // от цены работы», но не мигает от шума частот и планировщика.
    assert!(
        heavy < light * 0.8,
        "тяжёлая {heavy:.1} fps против лёгкой {light:.1} fps"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn warmup_frames_are_excluded_from_the_measured_result() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let config = SaturationConfig {
        warmup: Duration::from_millis(300),
        measure: Duration::from_millis(200),
        ..quick_config(2)
    };
    let (report, workload) = run_pattern(&context, 256, config);
    assert!(report.warmup_frames > 0, "{report:?}");
    // Прогрев втрое длиннее окна: если бы он попадал в итог, окно содержало бы большинство кадров.
    assert!(
        report.measured_frames < report.warmup_frames,
        "окно 200 мс не может содержать больше кадров, чем прогрев 300 мс: {report:?}"
    );
    assert_eq!(report.window, Duration::from_millis(200));
    assert_eq!(
        report.timeline.iter().map(|step| step.frames).sum::<u64>(),
        report.measured_frames
    );
    assert_eq!(workload.released_frames, report.total_completed_frames());
}

#[test]
fn workload_error_stops_run_after_draining_sent_frames_and_releases_everything() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let failing_frame = 5;
        let mut workload = PatternWorkload::new(&context, 256, 2).failing_at_frame(failing_frame);
        let mut bench = SaturationBench::new(&context, quick_config(2)).unwrap();
        let error = bench.run(&mut workload, &CancelToken::new()).unwrap_err();
        // Ошибка работы не маскируется под ошибку GPU и сообщает число успешно завершённых кадров.
        match error {
            SaturationError::Workload {
                source: GpuError::InvalidState { .. },
                frames_completed,
            } => assert_eq!(frames_completed, failing_frame),
            other => panic!("ожидали ошибку работы, получили {other:?}"),
        }
        // Отправленные кадры дождались и освобождены; сбойный кадр не отправлен.
        assert_eq!(workload.recorded_frames, failing_frame);
        assert_eq!(workload.released_frames, failing_frame);
        assert!(workload.violations.is_empty(), "{:?}", workload.violations);
        // GPU исполнил ровно успешные кадры, сбойный не попал в очередь.
        assert_eq!(
            workload.counter_value(),
            (failing_frame * workload.pixels_per_frame()) as u32
        );
    }
    assert_no_validation_errors(&context);
    assert_eq!(context.live_objects(), baseline);
}

#[test]
fn cancel_from_another_thread_returns_partial_report_not_an_error() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let config = SaturationConfig {
        warmup: Duration::ZERO,
        measure: Duration::from_secs(30),
        ..quick_config(2)
    };
    let mut workload = PatternWorkload::new(&context, 256, 2);
    let mut bench = SaturationBench::new(&context, config).unwrap();
    let token = CancelToken::new();
    let canceller = {
        let token = token.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(300));
            token.cancel();
        })
    };
    let started = Instant::now();
    let report = bench.run(&mut workload, &token).unwrap();
    canceller.join().unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "отмена не остановила прогон за разумное время"
    );
    assert_eq!(report.end_reason, EndReason::Cancelled);
    assert!(report.measured_frames > 0, "за 300 мс кадры должны быть: {report:?}");
    assert!(
        report.window < Duration::from_secs(5) && report.window >= Duration::from_millis(250),
        "{:?}",
        report.window
    );
    assert_eq!(
        report.timeline.iter().map(|step| step.frames).sum::<u64>(),
        report.measured_frames
    );
    // Хвост очереди дождались: у работы нет неосвобождённых слотов.
    assert_eq!(workload.released_frames, report.total_completed_frames());
    assert_eq!(workload.recorded_frames, report.total_completed_frames());
    assert!(workload.violations.is_empty());
}

#[test]
fn run_cancelled_before_start_submits_nothing() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let mut workload = PatternWorkload::new(&context, 256, 2);
    let mut bench = SaturationBench::new(&context, quick_config(2)).unwrap();
    let token = CancelToken::new();
    token.cancel();
    let report = bench.run(&mut workload, &token).unwrap();
    assert_eq!(report.end_reason, EndReason::Cancelled);
    assert_eq!(report.throughput, Throughput::NoFramesCompleted);
    assert_eq!(report.total_completed_frames(), 0);
    assert_eq!(workload.recorded_frames, 0);
    assert_eq!(workload.counter_value(), 0);
    assert_eq!(report.window, Duration::ZERO);
}

#[test]
fn window_shorter_than_one_frame_is_a_distinct_no_frames_state() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    // Кадр 4096×4096 с атомарным счётчиком на каждый пиксель стоит заметно больше миллисекунды, а окно — 1 мс:
    // ни один кадр не успевает завершиться внутри окна, и стенд обязан сказать это явно, а не поделить на ноль.
    let config = SaturationConfig {
        warmup: Duration::ZERO,
        measure: Duration::from_millis(1),
        timeline_step: Duration::from_millis(1),
        ..quick_config(2)
    };
    let (report, workload) = run_pattern(&context, 4096, config);
    assert_eq!(report.throughput, Throughput::NoFramesCompleted, "{report:?}");
    assert_eq!(report.measured_frames, 0);
    assert!(
        report.drained_frames >= 1,
        "кадры отправлены и дождались после окна: {report:?}"
    );
    assert_eq!(workload.released_frames, report.total_completed_frames());
}

#[test]
fn invalid_config_is_rejected_before_any_gpu_object_is_created() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let cases = [
        (
            SaturationConfig {
                measure: Duration::ZERO,
                ..quick_config(2)
            },
            SaturationConfigError::ZeroMeasureWindow,
        ),
        (
            SaturationConfig {
                timeline_step: Duration::ZERO,
                ..quick_config(2)
            },
            SaturationConfigError::ZeroTimelineStep,
        ),
        (
            SaturationConfig {
                fence_timeout: Duration::ZERO,
                ..quick_config(2)
            },
            SaturationConfigError::ZeroFenceTimeout,
        ),
    ];
    for (config, expected) in cases {
        let error = SaturationBench::new(&context, config).err().expect("настройки неверны");
        assert_eq!(error, SaturationError::InvalidConfig(expected));
        assert_eq!(
            context.live_objects(),
            baseline,
            "GPU не должен был трогаться: {expected:?}"
        );
    }
    // Ноль слотов невозможен по построению типа.
    assert!(matches!(FramesInFlight::new(0), Err(GpuError::InvalidArgument { .. })));
}

#[test]
fn bench_can_run_twice_and_second_run_is_independent() {
    let _serial = serialize_gpu_timing();
    let Some(context) = gpu_context_or_skip() else { return };
    let mut bench = SaturationBench::new(&context, quick_config(2)).unwrap();
    let mut first = PatternWorkload::new(&context, 256, 2);
    let first_report = bench.run(&mut first, &CancelToken::new()).unwrap();
    let mut second = PatternWorkload::new(&context, 256, 2);
    let second_report = bench.run(&mut second, &CancelToken::new()).unwrap();
    assert_eq!(first.released_frames, first_report.total_completed_frames());
    assert_eq!(second.released_frames, second_report.total_completed_frames());
    assert!(second.violations.is_empty(), "{:?}", second.violations);
    assert_eq!(
        second.release_order.first(),
        Some(&0),
        "нумерация кадров второго прогона начинается с нуля"
    );
    assert!(second_report.measured_frames > 0);
}
