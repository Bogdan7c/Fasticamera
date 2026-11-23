//! Стенд V4 целиком на кадре 4096×2304 (основная камера 16:9@60): RAW16 → уровни → Menon 2007 → шумодав →
//! выход → приёмник кадра; метки времени по проходам собираются без ожиданий, объекты возвращаются к
//! исходному счёту. Время RADV по проходам печатается (справочно, не проверяется: цифра для телефона
//! снимается отдельно на Adreno).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

#[path = "../../isp-model/tests/support/mod.rs"]
mod isp_support;
mod quality_support;

use ash::vk;
use gpu_vulkan::compute::DescriptorResource;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::error::GpuError;
use gpu_vulkan::frame_sink::{
    FrameSink, FrameSinkDescription, FrameSinkGrant, FrameSinkPublish, FrameSinkStamp, FrameSinkSubmitDependencies,
    OffscreenFrameSink, OffscreenFrameSinkDesc,
};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::{read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use gpu_vulkan::raw_mosaic_preview::{MosaicSource, ValidArea};
use isp_model::demosaic::DemosaicMethod;
use isp_model::image::DisplayGamut;
use isp_model::params::{FrameInputs, IspParams, ProcessingControls};
use isp_support::real_capture::load_real_capture;
use isp_vulkan::quality::{
    HeadroomPass, HeadroomRecordOutcome, HeadroomTimingCollect, HeadroomTimings, HistoryUse, QualityError,
    StreamGeneration, TemporalDenoiseConfig, VideoFrameInput, VideoHeadroomConfig, VideoHeadroomWorkload,
};
use quality_support::{assert_no_validation_errors, extent, gpu_context_or_skip};

const VIDEO_WIDTH: u32 = 4096;
const VIDEO_HEIGHT: u32 = 2304;

/// Параметры `Quality` по калибровке реального кропа основной камеры (BGGR, RAW10-уровни 64/1023).
fn params(demosaic: DemosaicMethod) -> IspParams {
    let capture = load_real_capture(&isp_support::assets_dir(), "real-main-lamp-gray-tile");
    let frame = FrameInputs {
        levels: capture.sensor.static_levels(),
    };
    let controls = ProcessingControls::with_defaults(capture.neutral, demosaic, DisplayGamut::Srgb);
    IspParams::resolve(&capture.sensor, &frame, &controls).unwrap()
}

/// Синтетический RAW: диагональные полосы разной яркости + градиент (не плоский кадр).
fn synthetic_raw(width: u32, height: u32) -> Vec<u16> {
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            // Свет 0,01…0,22 от диапазона сенсора: вокруг среднего серого тона (0,075), не в пересвете.
            let stripe = ((x + y) / 64) % 4;
            let gradient = x * 100 / width;
            (64 + 10 + stripe * 30 + gradient) as u16
        })
        .collect()
}

fn sink(context: &GpuContext, width: u32, height: u32, slots: usize) -> OffscreenFrameSink {
    OffscreenFrameSink::new(
        context,
        &OffscreenFrameSinkDesc {
            extent: extent(width, height),
            format_policy: VideoHeadroomConfig::default().output_format.storage_policy(),
            frame_slots: slots,
            reader: ConsumerStage::Fragment,
        },
    )
    .unwrap()
}

fn full_area(width: u32, height: u32) -> ValidArea {
    ValidArea {
        left: 0,
        top: 0,
        width,
        height,
    }
}

/// Записывает и выполняет кадр; возвращает итог записи.
fn run_frame(
    context: &GpuContext,
    workload: &mut VideoHeadroomWorkload,
    frame_sink: &mut OffscreenFrameSink,
    frame_slot: usize,
    input: &VideoFrameInput<'_>,
) -> HeadroomRecordOutcome {
    let mut outcome = None;
    run_one_shot_blocking(context, |command_buffer| {
        outcome = Some(workload.record(command_buffer, frame_slot, input, frame_sink).map_err(
            |error| match error {
                QualityError::Gpu(gpu) => gpu,
                other => panic!("запись кадра: {other}"),
            },
        )?);
        Ok(())
    })
    .unwrap();
    assert!(frame_sink.submit_dependencies(frame_slot).unwrap().is_empty());
    outcome.unwrap()
}

fn frame_input<'a>(source: &'a MosaicSource, params: &'a IspParams, valid_area: ValidArea) -> VideoFrameInput<'a> {
    VideoFrameInput {
        source,
        params,
        valid_area,
        stream: StreamGeneration(3),
        sequence: 0,
        presentation_time_ns: 0,
    }
}

fn collect_ready(workload: &mut VideoHeadroomWorkload, frame_slot: usize) -> HeadroomTimings {
    match workload.collect(frame_slot).unwrap() {
        HeadroomTimingCollect::Ready(timings) => timings,
        other => panic!("метки слота {frame_slot} после fence должны быть готовы: {other:?}"),
    }
}

#[test]
fn full_resolution_workload_runs_reports_per_pass_times_and_releases_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    {
        let params = params(DemosaicMethod::Quality);
        let raw = SyntheticRaw16::new(
            &context,
            VIDEO_WIDTH,
            VIDEO_HEIGHT,
            &synthetic_raw(VIDEO_WIDTH, VIDEO_HEIGHT),
        )
        .unwrap();
        let source = raw.source();
        let config = VideoHeadroomConfig::default();
        let mut workload = VideoHeadroomWorkload::new(&context, extent(VIDEO_WIDTH, VIDEO_HEIGHT), &config).unwrap();
        let mut frame_sink = sink(&context, VIDEO_WIDTH, VIDEO_HEIGHT, config.frame_slots);
        assert_eq!(workload.collect(0).unwrap(), HeadroomTimingCollect::NothingRecorded);

        let mut per_pass: Vec<(HeadroomPass, Vec<u64>)> =
            HeadroomPass::ORDER.iter().map(|pass| (*pass, Vec::new())).collect();
        let mut totals = Vec::new();
        const FRAMES: u64 = 6;
        for sequence in 0..FRAMES {
            let frame_slot = (sequence % config.frame_slots as u64) as usize;
            let input = VideoFrameInput {
                source: &source,
                params: &params,
                valid_area: full_area(VIDEO_WIDTH, VIDEO_HEIGHT),
                stream: StreamGeneration(1),
                sequence,
                presentation_time_ns: sequence * 16_666_667,
            };
            let outcome = run_frame(&context, &mut workload, &mut frame_sink, frame_slot, &input);
            let expected_history = if sequence == 0 {
                HistoryUse::Restarted
            } else {
                HistoryUse::Blended
            };
            assert_eq!(
                outcome,
                HeadroomRecordOutcome::Recorded {
                    history: Some(expected_history)
                }
            );
            assert!(
                matches!(frame_sink.after_submit(frame_slot).unwrap(), FrameSinkPublish::Published(stamp) if stamp.sequence == sequence)
            );
            let timings = collect_ready(&mut workload, frame_slot);
            // Все 11 проходов (шумодав включён) с положительным временем, сумма не больше полного кадра.
            assert_eq!(timings.passes.len(), HeadroomPass::ORDER.len(), "{timings:?}");
            let sum: u64 = timings.passes.iter().map(|timing| timing.duration_ns).sum();
            assert!(
                timings.passes.iter().all(|timing| timing.duration_ns > 0),
                "{timings:?}"
            );
            // Каждый отрезок округляется до наносекунды отдельно: сумма может превысить полное время на ≤ 1 нс
            // на отрезок.
            let rounding_slack = timings.passes.len() as u64;
            assert!(
                sum <= timings.total_ns + rounding_slack && timings.total_ns > 0,
                "{timings:?}"
            );
            for timing in &timings.passes {
                if let Some((_, samples)) = per_pass.iter_mut().find(|(pass, _)| *pass == timing.pass) {
                    samples.push(timing.duration_ns);
                }
            }
            totals.push(timings.total_ns);
            // Метки слота уже отданы — повторный сбор не возвращает их второй раз как новые.
            assert_eq!(workload.collect(frame_slot).unwrap(), HeadroomTimingCollect::NotReady);
        }
        let device = &context.capabilities().device_name;
        println!("V4 {VIDEO_WIDTH}x{VIDEO_HEIGHT}, {device}, медиана {FRAMES} кадров (validation FULL, справочно):");
        for (pass, samples) in &mut per_pass {
            samples.sort_unstable();
            let median = samples.get(samples.len() / 2).copied().unwrap_or(0);
            println!("  {:<30} {:>8.3} мс", pass.label(), median as f64 / 1e6);
        }
        totals.sort_unstable();
        println!("  {:<30} {:>8.3} мс", "всего", totals[totals.len() / 2] as f64 / 1e6);

        // Кадр дошёл до приёмника и не чёрный/не белый: среднее сигнала экрана в разумных пределах.
        let last_slot = ((FRAMES - 1) % config.frame_slots as u64) as usize;
        let pixels = read_storage_image_rgba(
            frame_sink.image(last_slot).unwrap(),
            frame_sink.last_use(last_slot).unwrap(),
        )
        .unwrap();
        let mean = pixels
            .iter()
            .map(|pixel| (pixel[0] + pixel[1] + pixel[2]) / 3.0)
            .sum::<f32>()
            / pixels.len() as f32;
        let alpha_ok = pixels.iter().all(|pixel| (pixel[3] - 1.0).abs() < 0.01);
        println!("  средний сигнал экрана {mean:.3}");
        assert!(
            mean > 0.1 && mean < 0.95 && alpha_ok,
            "выход подозрителен: среднее {mean}"
        );
        assert_eq!(frame_sink.published_frames(), FRAMES);
    }
    context.wait_idle().unwrap();
    assert_eq!(
        context.live_objects(),
        baseline,
        "объекты стенда не вернулись к исходному счёту"
    );
    assert_no_validation_errors(&context);
}

#[test]
fn disabled_denoise_skips_its_pass_and_invalid_input_records_nothing() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (width, height) = (64, 32);
    let config = VideoHeadroomConfig {
        temporal_denoise: TemporalDenoiseConfig {
            enabled: false,
            ..TemporalDenoiseConfig::default()
        },
        ..VideoHeadroomConfig::default()
    };
    let mut workload = VideoHeadroomWorkload::new(&context, extent(width, height), &config).unwrap();
    assert!(workload.temporal_denoise().is_none());
    let mut frame_sink = sink(&context, width, height, config.frame_slots);
    let raw = SyntheticRaw16::new(&context, width + 2, height + 2, &synthetic_raw(width + 2, height + 2)).unwrap();
    let source: MosaicSource = raw.source();
    let quality = params(DemosaicMethod::Quality);
    let input = |valid_area: ValidArea, params| frame_input(&source, params, valid_area);
    let outcome = run_frame(
        &context,
        &mut workload,
        &mut frame_sink,
        1,
        &input(full_area(width, height), &quality),
    );
    assert_eq!(outcome, HeadroomRecordOutcome::Recorded { history: None });
    let timings = collect_ready(&mut workload, 1);
    let passes: Vec<HeadroomPass> = timings.passes.iter().map(|timing| timing.pass).collect();
    assert!(!passes.contains(&HeadroomPass::TemporalDenoise), "{passes:?}");
    assert_eq!(passes.len(), HeadroomPass::ORDER.len() - 1);
    assert_eq!(passes.last(), Some(&HeadroomPass::Output));

    // Ошибки — до записи команд: слот 0 остаётся пустым, приёмник кадр не начинал.
    let preview = params(DemosaicMethod::Preview);
    let wrong_size = ValidArea {
        width: 32,
        ..full_area(width, height)
    };
    let outside = ValidArea {
        left: 4,
        top: 4,
        ..full_area(width, height)
    };
    run_one_shot_blocking(&context, |command_buffer| {
        let mut check =
            |input: VideoFrameInput<'_>, slot: usize| workload.record(command_buffer, slot, &input, &mut frame_sink);
        assert!(matches!(
            check(input(full_area(width, height), &preview), 0),
            Err(QualityError::UnsupportedParams { .. })
        ));
        assert!(matches!(
            check(input(wrong_size, &quality), 0),
            Err(QualityError::InvalidInput { .. })
        ));
        assert!(matches!(
            check(input(outside, &quality), 0),
            Err(QualityError::InvalidInput { .. })
        ));
        assert!(matches!(
            check(input(full_area(width, height), &quality), 5),
            Err(QualityError::InvalidInput { .. })
        ));
        Ok(())
    })
    .unwrap();
    assert_eq!(workload.collect(0).unwrap(), HeadroomTimingCollect::NothingRecorded);
    assert_eq!(frame_sink.finished_stamp(0), None);

    // Приёмник другого размера — отказ до записи.
    let mut small_sink = sink(&context, 32, 32, 1);
    run_one_shot_blocking(&context, |command_buffer| {
        let error = workload
            .record(
                command_buffer,
                0,
                &input(full_area(width, height), &quality),
                &mut small_sink,
            )
            .unwrap_err();
        assert!(matches!(error, QualityError::InvalidInput { .. }), "{error}");
        Ok(())
    })
    .unwrap();
    assert_no_validation_errors(&context);
}

/// Приёмник-обёртка с дефектом: первый кадр выдаёт цель, непригодную для записи compute (картинку
/// «только чтение»). Всё остальное — честный `OffscreenFrameSink`.
struct BrokenFirstTargetSink {
    inner: OffscreenFrameSink,
    broken_grants_left: u32,
}

impl FrameSink for BrokenFirstTargetSink {
    fn description(&self) -> FrameSinkDescription {
        self.inner.description()
    }

    fn begin_frame(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
    ) -> Result<FrameSinkGrant, GpuError> {
        let grant = self.inner.begin_frame(command_buffer, frame_slot)?;
        match grant {
            FrameSinkGrant::Ready(mut target) if self.broken_grants_left > 0 => {
                self.broken_grants_left -= 1;
                target.storage = DescriptorResource::ReadOnlyImage {
                    view: vk::ImageView::null(),
                };
                Ok(FrameSinkGrant::Ready(target))
            }
            other => Ok(other),
        }
    }

    fn finish_frame(
        &mut self,
        command_buffer: vk::CommandBuffer,
        frame_slot: usize,
        stamp: FrameSinkStamp,
    ) -> Result<(), GpuError> {
        self.inner.finish_frame(command_buffer, frame_slot, stamp)
    }

    fn abort_frame(&mut self, frame_slot: usize) -> Result<(), GpuError> {
        self.inner.abort_frame(frame_slot)
    }

    fn submit_dependencies(&self, frame_slot: usize) -> Result<FrameSinkSubmitDependencies, GpuError> {
        self.inner.submit_dependencies(frame_slot)
    }

    fn after_submit(&mut self, frame_slot: usize) -> Result<FrameSinkPublish, GpuError> {
        self.inner.after_submit(frame_slot)
    }
}

/// Сбой, который можно обнаружить только после `begin_frame` (негодная цель приёмника): кадр отменён,
/// ни одного прохода не записано, приёмник не застрял «в кадре» и следующий кадр проходит целиком.
#[test]
fn bad_sink_target_aborts_the_frame_and_the_sink_stays_usable() {
    let Some(context) = gpu_context_or_skip() else { return };
    let (width, height) = (64, 32);
    let config = VideoHeadroomConfig::default();
    let mut workload = VideoHeadroomWorkload::new(&context, extent(width, height), &config).unwrap();
    let mut frame_sink = BrokenFirstTargetSink {
        inner: sink(&context, width, height, config.frame_slots),
        broken_grants_left: 1,
    };
    let raw = SyntheticRaw16::new(&context, width, height, &synthetic_raw(width, height)).unwrap();
    let source: MosaicSource = raw.source();
    let quality = params(DemosaicMethod::Quality);
    let input = frame_input(&source, &quality, full_area(width, height));

    // Буфер с отменённым кадром не отправляется: замыкание возвращает ошибку.
    let failed = run_one_shot_blocking(&context, |command_buffer| {
        let error = workload.record(command_buffer, 0, &input, &mut frame_sink).unwrap_err();
        assert!(matches!(error, QualityError::InvalidInput { .. }), "{error}");
        Err(GpuError::InvalidState {
            what: "кадр отменён — буфер выбрасывается".to_owned(),
        })
    });
    assert!(failed.is_err());
    assert_eq!(
        workload.collect(0).unwrap(),
        HeadroomTimingCollect::NothingRecorded,
        "проходов не было"
    );
    assert_eq!(frame_sink.inner.finished_stamp(0), None);
    assert_eq!(frame_sink.after_submit(0).unwrap(), FrameSinkPublish::NothingToPublish);

    // Тот же слот, тот же приёмник: кадр проходит и публикуется.
    run_one_shot_blocking(&context, |command_buffer| {
        let outcome = workload
            .record(command_buffer, 0, &input, &mut frame_sink)
            .map_err(|error| GpuError::InvalidState {
                what: format!("второй кадр: {error}"),
            })?;
        assert!(matches!(outcome, HeadroomRecordOutcome::Recorded { .. }), "{outcome:?}");
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        frame_sink.after_submit(0).unwrap(),
        FrameSinkPublish::Published(_)
    ));
    assert!(matches!(workload.collect(0).unwrap(), HeadroomTimingCollect::Ready(_)));
    assert_no_validation_errors(&context);
}

#[test]
fn invalid_config_is_rejected_before_gpu_objects() {
    let Some(context) = gpu_context_or_skip() else { return };
    let baseline = context.live_objects();
    let config = VideoHeadroomConfig {
        frame_slots: 0,
        ..VideoHeadroomConfig::default()
    };
    assert!(matches!(
        VideoHeadroomWorkload::new(&context, extent(64, 32), &config),
        Err(QualityError::InvalidConfig { .. })
    ));
    assert!(matches!(
        VideoHeadroomWorkload::new(&context, extent(63, 32), &VideoHeadroomConfig::default()),
        Err(QualityError::InvalidInput { .. })
    ));
    assert_eq!(context.live_objects(), baseline);
}
