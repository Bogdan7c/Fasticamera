//! Тесты адаптера «кадр показа» под стенд насыщения на реальном GPU (RADV, полная validation).
//!
//! Каждый тест проверяет наблюдаемый итог: пиксели после readback (сверка с кадром, записанным обычным
//! путём вне стенда), счётчики пирамиды рендера UI, времена проходов, типизированные ошибки, число живых
//! объектов Vulkan.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod saturation_support;
mod support;

use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use ash::vk;
use gpu_vulkan::color_target::ColorTargetPreviousUse;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::kit_test_support::read_color_target_rgba;
use gpu_vulkan::saturation::{
    CancelToken, SaturationBench, SaturationConfig, SaturationError, SaturationReport, Throughput,
};
use gpu_vulkan::swapchain::FramesInFlight;
use saturation_support::{FREE_VIEWFINDER_PIXEL, pixel_index, render_reference, typical_scene};
use support::{assert_no_validation_errors, gpu_context_or_skip};
use ui_render_vulkan::UiRenderCounters;
use ui_render_vulkan::saturation::{
    BackdropCadence, FrameComposition, UiFrameSaturation, UiFrameSaturationConfig, UiSaturationConfigError,
    UiSaturationError,
};

/// Замер времени честен только когда GPU занят одним тестом: тесты этого файла идут строго по очереди.
static GPU_LOCK: Mutex<()> = Mutex::new(());

fn serialize_gpu() -> MutexGuard<'static, ()> {
    GPU_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Слотов у стенда и адаптера в тестах.
const SLOTS: usize = 2;

fn adapter_config(composition: FrameComposition, cadence: BackdropCadence) -> UiFrameSaturationConfig {
    UiFrameSaturationConfig {
        frames_in_flight: FramesInFlight::new(SLOTS).unwrap(),
        composition,
        backdrop_cadence: cadence,
        ..UiFrameSaturationConfig::default()
    }
}

/// Короткий конфиг стенда: прогрев 50 мс, окно 250 мс.
fn bench_config(slots: usize) -> SaturationConfig {
    SaturationConfig {
        frames_in_flight: FramesInFlight::new(slots).unwrap(),
        warmup: Duration::from_millis(50),
        measure: Duration::from_millis(250),
        timeline_step: Duration::from_millis(125),
        fence_timeout: Duration::from_secs(20),
    }
}

/// Прогон адаптера стендом; возвращает итог стенда и сам адаптер (для проверки пикселей и счётчиков).
fn run_adapter(
    context: &GpuContext,
    composition: FrameComposition,
    cadence: BackdropCadence,
) -> (SaturationReport, UiFrameSaturation) {
    let mut adapter =
        UiFrameSaturation::new(context, adapter_config(composition, cadence), typical_scene(context)).unwrap();
    let mut bench = SaturationBench::new(context, bench_config(SLOTS)).unwrap();
    let report = bench.run(&mut adapter, &CancelToken::new()).unwrap();
    (report, adapter)
}

/// Пиксели цели последнего записанного кадра (GPU уже закончил: прогон стенда дождался хвоста).
fn last_frame_pixels(adapter: &UiFrameSaturation) -> Vec<[f32; 4]> {
    let slot = adapter.last_recorded_slot().expect("кадры записаны");
    read_color_target_rgba(adapter.target(slot).unwrap(), ColorTargetPreviousUse::Rendered).unwrap()
}

#[test]
fn compositor_then_ui_draws_the_same_frame_as_the_ordinary_path() {
    let _lock = serialize_gpu();
    let Some(context) = gpu_context_or_skip() else { return };
    let reference = render_reference(&context, FrameComposition::CompositorThenUi);
    let (report, adapter) = run_adapter(
        &context,
        FrameComposition::CompositorThenUi,
        BackdropCadence::NewFrameEveryFrame,
    );

    assert!(report.measured_frames > 0, "стенд досчитал кадры: {report:?}");
    assert_eq!(adapter.release_failures(), 0);
    assert_eq!(adapter.frames_recorded(), report.total_completed_frames());
    let pixels = last_frame_pixels(&adapter);
    assert_eq!(pixels.len(), reference.len());
    // Бит-в-бит: тот же компоновщик, тот же рендер UI, те же входы.
    assert!(pixels == reference, "кадр стенда отличается от кадра обычного пути");
    // Кадр не пустой: под видоискателем фото, а не очистка.
    let free = pixels[pixel_index(FREE_VIEWFINDER_PIXEL.0, FREE_VIEWFINDER_PIXEL.1)];
    assert!(
        free[0] + free[1] + free[2] > 0.05,
        "в видоискателе виден кадр фона: {free:?}"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn background_cadence_decides_whether_the_pyramid_is_rebuilt_or_reused() {
    let _lock = serialize_gpu();
    let Some(context) = gpu_context_or_skip() else { return };
    let reference = render_reference(&context, FrameComposition::CompositorThenUi);

    let (_, fresh) = run_adapter(
        &context,
        FrameComposition::CompositorThenUi,
        BackdropCadence::NewFrameEveryFrame,
    );
    let frames = fresh.frames_recorded();
    assert!(frames > 2, "для проверки нужно несколько кадров: {frames}");
    let counters = fresh.renderer_counters();
    assert_eq!(
        counters.pyramid_rebuilds, frames,
        "новый кадр камеры на каждом кадре: пирамида строится каждый раз"
    );
    assert_eq!(counters.pyramid_reuses, 0);

    let (_, held) = run_adapter(&context, FrameComposition::CompositorThenUi, BackdropCadence::SameFrame);
    let frames = held.frames_recorded();
    assert!(frames > 2, "для проверки нужно несколько кадров: {frames}");
    let counters = held.renderer_counters();
    assert_eq!(
        counters.pyramid_rebuilds, 1,
        "тот же кадр камеры: пирамида строится один раз"
    );
    assert_eq!(
        counters.pyramid_reuses,
        frames - 1,
        "остальные кадры берут готовую пирамиду"
    );
    // Путь переиспользования рисует тот же кадр, что и перестройка.
    assert!(
        last_frame_pixels(&held) == reference,
        "кадр на переиспользованной пирамиде совпал с эталоном"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn compositor_only_draws_no_ui_and_ui_only_skips_the_compositor() {
    let _lock = serialize_gpu();
    let Some(context) = gpu_context_or_skip() else { return };
    let cadence = BackdropCadence::NewFrameEveryFrame;

    let (_, compositor_only) = run_adapter(&context, FrameComposition::CompositorOnly, cadence);
    assert_eq!(
        compositor_only.renderer_counters(),
        UiRenderCounters::default(),
        "рендер UI не вызывался ни разу"
    );
    let pixels = last_frame_pixels(&compositor_only);
    assert!(
        pixels == render_reference(&context, FrameComposition::CompositorOnly),
        "пиксели == только компоновщик"
    );
    let free = pixels[pixel_index(FREE_VIEWFINDER_PIXEL.0, FREE_VIEWFINDER_PIXEL.1)];
    assert!(
        free[0] + free[1] + free[2] > 0.05,
        "компоновщик положил кадр фона: {free:?}"
    );

    let (_, ui_only) = run_adapter(&context, FrameComposition::UiOnly, cadence);
    assert_eq!(ui_only.renderer_counters().frames_recorded, ui_only.frames_recorded());
    let pixels = last_frame_pixels(&ui_only);
    assert!(
        pixels == render_reference(&context, FrameComposition::UiOnly),
        "пиксели == очистка + UI"
    );
    let free = pixels[pixel_index(FREE_VIEWFINDER_PIXEL.0, FREE_VIEWFINDER_PIXEL.1)];
    assert_eq!(
        (free[0], free[1], free[2]),
        (0.0, 0.0, 0.0),
        "компоновщик не записан: в видоискателе вне UI осталась очистка"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn pass_times_have_labels_in_recording_order_and_are_measured() {
    let _lock = serialize_gpu();
    let Some(context) = gpu_context_or_skip() else { return };
    for (composition, labels) in [
        (FrameComposition::CompositorThenUi, vec!["compose", "pyramid", "ui"]),
        (FrameComposition::UiOnly, vec!["clear", "pyramid", "ui"]),
        (FrameComposition::CompositorOnly, vec!["compose"]),
    ] {
        let (report, adapter) = run_adapter(&context, composition, BackdropCadence::NewFrameEveryFrame);
        let summary = adapter.pass_times();
        let found: Vec<&str> = summary.passes.iter().map(|pass| pass.label).collect();
        assert_eq!(found, labels, "{composition:?}: метки и порядок");
        assert!(
            summary.measured_frames > 0,
            "{composition:?}: кадры измерены: {summary:?}"
        );
        assert_eq!(
            summary.mismatched_frames, 0,
            "{composition:?}: число отрезков совпало с числом меток"
        );
        assert_eq!(
            summary.measured_frames + summary.unmeasured_frames,
            report.total_completed_frames(),
            "{composition:?}: каждый досчитанный кадр попал в журнал"
        );
        assert!(
            summary
                .passes
                .iter()
                .all(|pass| pass.median > Duration::ZERO && pass.p95 >= pass.median && pass.max >= pass.p95),
            "{composition:?}: времена проходов осмысленны: {summary:?}"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn invalid_config_and_scene_are_rejected_before_any_gpu_object_is_created() {
    let _lock = serialize_gpu();
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let base = UiFrameSaturationConfig::default();

    let zero_extent = UiFrameSaturationConfig {
        target_extent: vk::Extent2D { width: 0, height: 2412 },
        ..base
    };
    let error = UiFrameSaturation::new(&context, zero_extent, typical_scene(&context))
        .err()
        .unwrap();
    assert!(matches!(
        error,
        UiSaturationError::InvalidConfig(UiSaturationConfigError::ZeroTargetExtent { width: 0, height: 2412 })
    ));

    let mut scene = typical_scene(&context);
    scene.viewfinder.height = 0;
    // Сцена с синтетическим фоном уже создала объекты GPU: baseline снимаем заново.
    let baseline_with_scene = context.live_objects();
    let error = UiFrameSaturation::new(&context, base, scene).err().unwrap();
    assert!(matches!(
        error,
        UiSaturationError::InvalidConfig(UiSaturationConfigError::ZeroViewfinderRect { width: 1080, height: 0 })
    ));
    // Ошибка вернула сцену в drop: объекты фона освобождены, адаптер ничего не создал.
    assert_eq!(
        context.live_objects().differences_from(&baseline).len(),
        0,
        "после отказа живых объектов не больше исходного (сцена {baseline_with_scene:?})"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn slot_count_mismatch_with_the_bench_is_a_workload_error_and_everything_is_released() {
    let _lock = serialize_gpu();
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let mut adapter = UiFrameSaturation::new(
            &context,
            adapter_config(FrameComposition::CompositorThenUi, BackdropCadence::NewFrameEveryFrame),
            typical_scene(&context),
        )
        .unwrap();
        // У стенда слотов больше, чем у адаптера: третий слот адаптеру неизвестен.
        let mut bench = SaturationBench::new(&context, bench_config(SLOTS + 1)).unwrap();
        let error = bench.run(&mut adapter, &CancelToken::new()).unwrap_err();
        match error {
            SaturationError::Workload { frames_completed, .. } => {
                assert_eq!(
                    frames_completed,
                    adapter.frames_recorded(),
                    "дождались всего отправленного"
                );
                assert!(frames_completed >= SLOTS as u64, "слоты 0 и 1 успели отработать");
            }
            other => panic!("ожидалась ошибка работы, получено {other:?}"),
        }
        assert_eq!(adapter.release_failures(), 0);
    }
    assert_eq!(
        context.live_objects(),
        baseline,
        "после drop объекты вернулись к исходному числу"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn objects_created_by_the_adapter_are_released_on_drop() {
    let _lock = serialize_gpu();
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let (_, adapter) = run_adapter(
            &context,
            FrameComposition::CompositorThenUi,
            BackdropCadence::NewFrameEveryFrame,
        );
        assert_ne!(context.live_objects(), baseline, "пока адаптер жив, его объекты видны");
        drop(adapter);
    }
    assert_eq!(context.live_objects(), baseline);
    assert_no_validation_errors(&context);
}

/// Печатающий бенч: «состав кадра × каденция фона → к/с → мс/кадр → медианы проходов». Порогов нет.
#[test]
fn saturation_bench_prints() {
    let _lock = serialize_gpu();
    let Some(context) = gpu_context_or_skip() else { return };
    let config = SaturationConfig {
        frames_in_flight: FramesInFlight::new(SLOTS).unwrap(),
        warmup: Duration::from_millis(300),
        measure: Duration::from_millis(1000),
        timeline_step: Duration::from_millis(500),
        fence_timeout: Duration::from_secs(20),
    };
    let rows = [
        (FrameComposition::CompositorOnly, BackdropCadence::NewFrameEveryFrame),
        (FrameComposition::UiOnly, BackdropCadence::NewFrameEveryFrame),
        (FrameComposition::UiOnly, BackdropCadence::SameFrame),
        (FrameComposition::CompositorThenUi, BackdropCadence::NewFrameEveryFrame),
        (FrameComposition::CompositorThenUi, BackdropCadence::SameFrame),
    ];
    println!("кадр 1080x2412 A2B10G10R10, 8 стёкол, 6 подписей, 6 иконок, фон 2048x1536; слотов {SLOTS}");
    println!(
        "{:<15} {:<11} {:>9} {:>9}  медианы проходов, мс",
        "состав", "фон", "к/с", "мс/кадр"
    );
    for (composition, cadence) in rows {
        let mut adapter =
            UiFrameSaturation::new(&context, adapter_config(composition, cadence), typical_scene(&context)).unwrap();
        let mut bench = SaturationBench::new(&context, config).unwrap();
        let report = bench.run(&mut adapter, &CancelToken::new()).unwrap();
        let Throughput::Measured {
            frames_per_second,
            frame_cost,
        } = report.throughput
        else {
            panic!("в окне замера нет кадров: {report:?}");
        };
        let passes: String = adapter
            .pass_times()
            .passes
            .iter()
            .map(|pass| format!("{}={:.3}", pass.label, pass.median.as_secs_f64() * 1e3))
            .collect::<Vec<_>>()
            .join(" ");
        println!(
            "{:<15} {:<11} {:>9.1} {:>9.3}  {passes}",
            composition.name(),
            cadence.name(),
            frames_per_second,
            frame_cost.as_secs_f64() * 1e3
        );
    }
    assert_no_validation_errors(&context);
}
