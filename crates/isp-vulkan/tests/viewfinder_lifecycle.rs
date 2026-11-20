//! Жизненный цикл `ViewfinderIsp`: слоты и `collect` (никогда не ждёт), отложенное уничтожение
//! пересозданного изображения и вытесненных вариантов пайплайна, ошибки до записи команд, подложка,
//! возврат живых объектов Vulkan к исходному числу.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod viewfinder_support;

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use gpu_vulkan::backdrop::{BackdropFrameNumber, SubmissionSerial};
use gpu_vulkan::gpu_timing::TimestampCollect;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::swapchain::DisplayColorSpace;
use isp_vulkan::viewfinder::{
    IspStage, IspStageSet, OutputAllocation, ViewfinderError, ViewfinderFrame, ViewfinderGeometry, ViewfinderIsp,
    ViewfinderStorageFormat,
};
use viewfinder_support::{
    GpuSource, ParamsSpec, RawFrame, RunRequest, assert_no_validation_errors, gpu_context_or_skip, record_and_submit,
    test_config,
};

#[test]
fn slots_retirement_and_live_objects_follow_the_contract() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let spec = ParamsSpec::default();
        let params = spec.build();
        let raw = RawFrame::from_linear(32, 24, spec.pattern, spec.levels(), |x, _, channel| {
            f64::from(x) / 40.0 * spec.neutral[channel]
        });
        let gpu_source = GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
        let source = gpu_source.source();
        let mut config = test_config(ViewfinderStorageFormat::HalfFloatOnly);
        config.pipeline_variant_capacity = 2;
        let mut isp = ViewfinderIsp::new(&context, &config).unwrap();
        let full = RunRequest::whole_frame(&raw, &params, spec.display());

        // До первой записи: слот пуст, изображения и подложки нет.
        let empty = isp.collect(0).unwrap();
        assert_eq!(empty.isp_gpu_time, TimestampCollect::NothingRecorded);
        assert_eq!(empty.awb_statistics, None);
        assert!(isp.image().is_none());
        assert!(isp.backdrop_source(SubmissionSerial(1)).is_none());

        // Запись 0, слот 0, вариант A.
        record_and_submit(&context, &mut isp, &full, &source, 0, BackdropFrameNumber(10));
        let first = isp.collect(0).unwrap();
        assert!(matches!(first.isp_gpu_time, TimestampCollect::Ready(ref spans) if spans.total_ns > 0));
        assert_eq!(first.awb_statistics.as_ref().unwrap().totals().used_cells, 16 * 12);
        // Повторный сбор того же слота: статистика уже отдана, метки — те же старые → NotReady.
        let again = isp.collect(0).unwrap();
        assert_eq!(again.awb_statistics, None);
        assert_eq!(again.isp_gpu_time, TimestampCollect::NotReady);

        let backdrop = isp.backdrop_source(SubmissionSerial(1)).unwrap();
        assert_eq!(backdrop.frame(), BackdropFrameNumber(10));
        assert_eq!(backdrop.color_space(), DisplayColorSpace::DisplayP3);
        assert_eq!(backdrop.view(), isp.image().unwrap().sampled_view());
        assert_eq!(backdrop.extent().width, 16);

        // Запись 1, слот 1, вариант B; запись 2, слот 0, вариант C — кэш на 2, A вытеснен в отложенное.
        let without_tone = RunRequest {
            stages: IspStageSet::ALL.without(IspStage::Tone),
            ..full
        };
        record_and_submit(&context, &mut isp, &without_tone, &source, 1, BackdropFrameNumber(11));
        let without_oetf = RunRequest {
            stages: IspStageSet::ALL.without(IspStage::Oetf),
            ..full
        };
        record_and_submit(&context, &mut isp, &without_oetf, &source, 0, BackdropFrameNumber(12));
        assert_eq!(isp.cached_pipeline_variants(), 2);
        assert_eq!(isp.retired_objects(), 1, "вытесненный вариант ждёт fence");
        // Слот 1: его последняя запись (1) раньше вытеснения (2) — fence не доказывает, что A свободен.
        isp.collect(1).unwrap();
        assert_eq!(isp.retired_objects(), 1);
        isp.collect(0).unwrap();
        assert_eq!(isp.retired_objects(), 0, "fence записи 2 увиден — вариант уничтожен");

        // Смена кадрирования: изображение пересоздаётся, старое ждёт fence.
        let old_view = isp.image().unwrap().sampled_view();
        let cropped = RunRequest {
            geometry: ViewfinderGeometry {
                valid_area: raw.full_rect(),
                crop: PixelRect {
                    left: 4,
                    top: 4,
                    size: PixelSize::new(16, 12),
                },
                sensor_orientation: SensorOrientation::Degrees0,
                allocation: OutputAllocation::Exact,
            },
            ..full
        };
        record_and_submit(&context, &mut isp, &cropped, &source, 1, BackdropFrameNumber(13));
        assert_ne!(isp.image().unwrap().sampled_view(), old_view);
        assert_eq!(isp.image().unwrap().extent().width, 8);
        // Ждут fence двое: старое изображение и вариант B (вытеснен, когда вернулся вариант A — «все стадии»).
        assert_eq!(
            isp.retired_objects(),
            2,
            "старое изображение и вытесненный вариант ждут fence"
        );
        assert_eq!(isp.cached_pipeline_variants(), 2);
        isp.collect(1).unwrap();
        assert_eq!(isp.retired_objects(), 0);
    }
    assert_eq!(context.live_objects(), baseline, "все объекты ISP освобождены");
    assert_no_validation_errors(&context);
}

#[test]
fn collect_never_blocks_before_the_gpu_ran_the_slot() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let raw = RawFrame::from_linear(16, 16, spec.pattern, spec.levels(), |_, _, channel| {
        0.3 * spec.neutral[channel]
    });
    let gpu_source = GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
    let source = gpu_source.source();
    let mut isp = ViewfinderIsp::new(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly)).unwrap();
    let full = RunRequest::whole_frame(&raw, &params, spec.display());
    run_one_shot_blocking(&context, |command_buffer| {
        source.record_acquire_for(&context, command_buffer, ConsumerStage::Compute);
        isp.record(
            command_buffer,
            &ViewfinderFrame {
                source: &source,
                params: &params,
                geometry: full.geometry,
                display_color_space: full.display,
                stages: IspStageSet::ALL.without(IspStage::AwbStatistics),
                frame: BackdropFrameNumber(1),
                frame_slot: 0,
                reader: ConsumerStage::Fragment,
            },
        )
        .unwrap();
        // Команды ещё даже не отправлены: метки не готовы, и вызов обязан вернуться сразу.
        let started = std::time::Instant::now();
        let report = isp.collect(0).unwrap();
        assert_eq!(report.isp_gpu_time, TimestampCollect::NotReady);
        assert!(
            started.elapsed() < std::time::Duration::from_millis(100),
            "collect не ждёт GPU"
        );
        source.record_release_for(&context, command_buffer, ConsumerStage::Compute);
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        isp.collect(0).unwrap().isp_gpu_time,
        TimestampCollect::Ready(_)
    ));
    assert_no_validation_errors(&context);
}

/// Проверка вида ошибки.
type ErrorCheck = fn(&ViewfinderError) -> bool;

#[test]
fn invalid_frames_are_rejected_before_any_command_is_recorded() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let raw = RawFrame::from_linear(16, 16, spec.pattern, spec.levels(), |_, _, _| 0.3);
    let gpu_source = GpuSource::new(&context, &raw, viewfinder_support::SourceLayout::Raw16);
    let source = gpu_source.source();
    let mut isp = ViewfinderIsp::new(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly)).unwrap();
    let valid = ViewfinderFrame {
        source: &source,
        params: &params,
        geometry: RunRequest::whole_frame(&raw, &params, spec.display()).geometry,
        display_color_space: DisplayColorSpace::DisplayP3,
        stages: IspStageSet::ALL,
        frame: BackdropFrameNumber(1),
        frame_slot: 0,
        reader: ConsumerStage::Fragment,
    };
    let outside = ViewfinderFrame {
        geometry: ViewfinderGeometry {
            crop: PixelRect {
                left: 0,
                top: 0,
                size: PixelSize::new(16, 18),
            },
            ..valid.geometry
        },
        ..valid
    };
    // Сетка AWB 4×3 мельче 2×2 ячеек кадрирования — статистика не поместится.
    let tiny_crop = ViewfinderFrame {
        geometry: ViewfinderGeometry {
            crop: PixelRect {
                left: 0,
                top: 0,
                size: PixelSize::new(4, 4),
            },
            ..valid.geometry
        },
        ..valid
    };
    let cases: [(&str, ViewfinderFrame<'_>, ErrorCheck); 4] = [
        (
            "кадрирование за полезной областью",
            outside,
            |error| matches!(error, ViewfinderError::InvalidGeometry { .. }),
        ),
        (
            "гамма экрана не та",
            ViewfinderFrame {
                display_color_space: DisplayColorSpace::Srgb,
                ..valid
            },
            |error| matches!(error, ViewfinderError::GamutMismatch { .. }),
        ),
        (
            "слот вне кольца",
            ViewfinderFrame { frame_slot: 5, ..valid },
            |error| matches!(error, ViewfinderError::FrameSlotOutOfRange { slot: 5, slots: 2 }),
        ),
        ("статистика не помещается", tiny_crop, |error| {
            matches!(error, ViewfinderError::AwbStatistics(_))
        }),
    ];
    for (what, frame, expected) in cases {
        run_one_shot_blocking(&context, |command_buffer| {
            let error = isp.record(command_buffer, &frame).unwrap_err();
            assert!(expected(&error), "{what}: {error:?}");
            Ok(())
        })
        .unwrap();
        assert!(isp.image().is_none(), "{what}: изображение не создано");
        assert_eq!(isp.cached_pipeline_variants(), 0, "{what}: пайплайн не создан");
    }
    // Без статистики тот же маленький кадр проходит.
    run_one_shot_blocking(&context, |command_buffer| {
        let frame = ViewfinderFrame {
            stages: IspStageSet::ALL.without(IspStage::AwbStatistics),
            ..tiny_crop
        };
        source.record_acquire_for(&context, command_buffer, ConsumerStage::Compute);
        isp.record(command_buffer, &frame).unwrap();
        Ok(())
    })
    .unwrap();
    assert_eq!(isp.image().unwrap().extent().width, 2);
    assert_no_validation_errors(&context);
}

/// Удержание кадра (сессия 13): `detach_image` отдаёт текущую картинку целиком (с содержимым) и ISP продолжает
/// на НОВОЙ картинке, не трогая удержанную; удержанная читается как была; после `retire_detached_image` она
/// освобождается только когда завершится следующая запись ISP, а в конце все объекты GPU возвращаются.
#[test]
fn detached_image_keeps_its_pixels_while_the_isp_moves_on_and_is_retired_by_a_later_record() {
    use gpu_vulkan::kit_test_support::read_storage_image_rgba;
    use gpu_vulkan::live_objects::LiveObjectKind;

    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let spec = ParamsSpec::default();
        let params = spec.build();
        // Два разных кадра: градиент по x и по y, чтобы удержанные и новые пиксели различались.
        let by_x = RawFrame::from_linear(32, 24, spec.pattern, spec.levels(), |x, _, channel| {
            f64::from(x) / 40.0 * spec.neutral[channel]
        });
        let by_y = RawFrame::from_linear(32, 24, spec.pattern, spec.levels(), |_, y, channel| {
            f64::from(y) / 30.0 * spec.neutral[channel]
        });
        let first_source = GpuSource::new(&context, &by_x, viewfinder_support::SourceLayout::Raw16);
        let second_source = GpuSource::new(&context, &by_y, viewfinder_support::SourceLayout::Raw16);
        let mut isp = ViewfinderIsp::new(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly)).unwrap();
        let first = RunRequest::whole_frame(&by_x, &params, spec.display());
        record_and_submit(
            &context,
            &mut isp,
            &first,
            &first_source.source(),
            0,
            BackdropFrameNumber(1),
        );
        isp.collect(0).unwrap();
        let before = {
            let image = isp.image().unwrap();
            read_storage_image_rgba(image.storage_image(), image.current_use()).unwrap()
        };

        // Отцепили: у ISP картинки больше нет, у нас — прежняя целиком.
        let held = isp.detach_image().expect("картинка была");
        assert!(isp.image().is_none(), "ISP остался без картинки до следующего кадра");
        assert!(isp.detach_image().is_none(), "отцепить второй раз нечего");
        assert_eq!(held.content_extent(), held.extent(), "Exact: содержимое = картинка");
        let images_with_held = context.live_objects().count(LiveObjectKind::Image);

        // ISP пишет следующий кадр в НОВУЮ картинку; удержанная не меняется ни на байт.
        let second = RunRequest::whole_frame(&by_y, &params, spec.display());
        record_and_submit(
            &context,
            &mut isp,
            &second,
            &second_source.source(),
            0,
            BackdropFrameNumber(2),
        );
        isp.collect(0).unwrap();
        assert_eq!(
            context.live_objects().count(LiveObjectKind::Image),
            images_with_held + 1
        );
        let after = read_storage_image_rgba(held.storage_image(), held.current_use()).unwrap();
        assert_eq!(after, before, "удержанная картинка не тронута записью следующего кадра");
        let live = {
            let image = isp.image().unwrap();
            read_storage_image_rgba(image.storage_image(), image.current_use()).unwrap()
        };
        assert_ne!(live, before, "новый кадр лежит в новой картинке");

        // Возврат на уничтожение: не раньше следующей записи ISP.
        isp.retire_detached_image(held);
        assert_eq!(isp.retired_objects(), 1, "ждёт завершения следующей записи");
        assert_eq!(
            context.live_objects().count(LiveObjectKind::Image),
            images_with_held + 1
        );
        record_and_submit(
            &context,
            &mut isp,
            &second,
            &second_source.source(),
            0,
            BackdropFrameNumber(3),
        );
        isp.collect(0).unwrap();
        assert_eq!(
            isp.retired_objects(),
            0,
            "после завершения записи удержанная картинка освобождена"
        );
        assert_eq!(context.live_objects().count(LiveObjectKind::Image), images_with_held);
    }
    assert_eq!(context.live_objects(), baseline, "объекты GPU вернулись к исходным");
    assert_no_validation_errors(&context);
}
