//! Контракт кадра `ViewfinderIsp` в потоке «как в продукте»: новый кадр камеры на каждый кадр в кольце
//! слотов (`collect` освобождает наборы дескрипторов слота), WAR-барьер перезаписи дожидается чтений
//! подложки на ЛЮБОЙ шейдерной стадии, подложка после пересоздания изображения сообщает, что устарела.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod viewfinder_support;

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use gpu_vulkan::backdrop::{BackdropFrameNumber, SubmissionSerial};
use gpu_vulkan::compute::{BindingKind, BindingWrite, ComputePipeline, ComputePipelineDesc, LocalSize, WorkgroupCount};
use gpu_vulkan::error::GpuError;
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::host_buffer::{HostBuffer, HostBufferUsage};
use gpu_vulkan::kit_test_support::run_one_shot_blocking;
use gpu_vulkan::shaders::blobs;
use isp_vulkan::viewfinder::{
    IspStage, IspStageSet, OutputAllocation, ViewfinderError, ViewfinderFrame, ViewfinderGeometry, ViewfinderIsp,
    ViewfinderStorageFormat,
};
use viewfinder_support::{
    GpuSource, ParamsSpec, RawFrame, RunRequest, SourceLayout, assert_no_validation_errors, gpu_context_or_skip,
    readback, record_and_submit, test_config,
};

const WIDTH: u32 = 32;
const HEIGHT: u32 = 24;

/// Плоский серый кадр яркости `level` (доля диапазона сенсора).
fn flat_frame(spec: &ParamsSpec, level: f64) -> RawFrame {
    RawFrame::from_linear(WIDTH, HEIGHT, spec.pattern, spec.levels(), |_, _, channel| {
        level * spec.neutral[channel]
    })
}

/// Регрессия (P7): в продукте каждый кадр камеры — НОВЫЙ импорт (другой буфер/картинка). Слоты кольца
/// переиспользуются; `collect(slot)` после fence обязан освободить набор дескрипторов слота у каждого
/// варианта пайплайна, иначе третья запись (слот 0 второй раз) с новым источником — `InvalidState`.
#[test]
fn every_frame_with_a_new_camera_source_is_processed_across_reused_slots() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let mut isp = ViewfinderIsp::new(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly)).unwrap();
    let mut previous_luma: Option<f32> = None;
    for frame_index in 0..6_u32 {
        // Яркость растёт от кадра к кадру: по выходу видно, что обработан именно новый источник.
        let raw = flat_frame(&spec, 0.04 + 0.03 * f64::from(frame_index));
        for layout in [SourceLayout::Raw16, SourceLayout::Raw10 { padding_bytes: 8 }] {
            let gpu_source = GpuSource::new(&context, &raw, layout);
            let source = gpu_source.source();
            let request = RunRequest {
                layout,
                ..RunRequest::whole_frame(&raw, &params, spec.display())
            };
            let slot = (frame_index % 2) as usize;
            record_and_submit(
                &context,
                &mut isp,
                &request,
                &source,
                slot,
                BackdropFrameNumber(u64::from(frame_index)),
            );
            // `readback` зовёт `collect(slot)` — как поток рендера после fence слота.
            let run = readback(&mut isp, slot);
            let luma = run.pixel(WIDTH / 4, HEIGHT / 4)[1];
            if layout == SourceLayout::Raw16
                && let Some(previous) = previous_luma
            {
                assert!(
                    luma > previous,
                    "кадр {frame_index}: {luma} не ярче {previous} — источник не тот"
                );
            }
            if layout == SourceLayout::Raw16 {
                previous_luma = Some(luma);
            }
        }
    }
    assert_no_validation_errors(&context);
}

/// Без `collect` между записями одного слота смена источника — честная ошибка до записи команд, а не
/// обновление набора, привязанного в командном буфере.
#[test]
fn re_recording_a_slot_without_collect_is_rejected_before_any_command() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let raw = flat_frame(&spec, 0.1);
    let first = GpuSource::new(&context, &raw, SourceLayout::Raw16);
    let second = GpuSource::new(&context, &raw, SourceLayout::Raw16);
    let mut isp = ViewfinderIsp::new(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly)).unwrap();
    let request = RunRequest::whole_frame(&raw, &params, spec.display());
    record_and_submit(&context, &mut isp, &request, &first.source(), 0, BackdropFrameNumber(1));
    let second_source = second.source();
    run_one_shot_blocking(&context, |command_buffer| {
        let error = isp
            .record(
                command_buffer,
                &ViewfinderFrame {
                    source: &second_source,
                    params: &params,
                    geometry: request.geometry,
                    display_color_space: request.display,
                    stages: request.stages,
                    frame: BackdropFrameNumber(2),
                    frame_slot: 0,
                    reader: ConsumerStage::Fragment,
                },
            )
            .unwrap_err();
        assert!(
            matches!(error, ViewfinderError::Gpu(GpuError::InvalidState { .. })),
            "{error:?}"
        );
        Ok(())
    })
    .unwrap();
    // После collect тот же слот с новым источником записывается.
    isp.collect(0).unwrap();
    record_and_submit(&context, &mut isp, &request, &second_source, 0, BackdropFrameNumber(2));
    assert_no_validation_errors(&context);
}

/// Потребитель подложки (стекло) читает изображение в COMPUTE, хотя кадр записан для читателя
/// `Fragment`. Следующая запись ISP обязана дождаться и этого чтения (WAR), иначе sync-валидация
/// сообщает `SYNC-HAZARD-WRITE-AFTER-READ`.
#[test]
fn next_write_waits_for_backdrop_reads_on_any_shader_stage() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let raw = flat_frame(&spec, 0.1);
    let gpu_source = GpuSource::new(&context, &raw, SourceLayout::Raw16);
    let source = gpu_source.source();
    let mut isp = ViewfinderIsp::new(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly)).unwrap();
    // Без статистики AWB: её барьеры (обнуление буфера) случайно связали бы compute-чтение стекла с
    // записью ISP и скрыли бы дефект барьера изображения.
    let request = RunRequest {
        stages: IspStageSet::ALL.without(IspStage::AwbStatistics),
        ..RunRequest::whole_frame(&raw, &params, spec.display())
    };
    // Первая запись создаёт изображение (вне проверяемого буфера), подложку берём из неё.
    record_and_submit(&context, &mut isp, &request, &source, 0, BackdropFrameNumber(1));
    isp.collect(0).unwrap();
    let image = isp.image().unwrap();
    let (width, height) = (image.extent().width, image.extent().height);

    let mut glass_reader = ComputePipeline::new(
        &context,
        &ComputePipelineDesc {
            label: "test-glass-reader",
            spirv: blobs::KIT_TEST_SAMPLE_COMP,
            bindings: &[BindingKind::CombinedImageSampler, BindingKind::ReadWriteStorageBuffer],
            push_constant_bytes: 8,
            specialization: &[],
            frame_slots: 1,
        },
    )
    .unwrap();
    let sampled = HostBuffer::new(
        &context,
        u64::from(width * height) * 16,
        HostBufferUsage::ReadWriteStorage,
    )
    .unwrap();
    glass_reader
        .bind(
            0,
            &[
                BindingWrite {
                    binding: 0,
                    resource: image.storage_image().read_only_binding(),
                },
                BindingWrite {
                    binding: 1,
                    resource: sampled.binding(),
                },
            ],
        )
        .unwrap();
    let push: Vec<u8> = [width, height].iter().flat_map(|word| word.to_ne_bytes()).collect();
    let frame = |slot: usize, number: u64| ViewfinderFrame {
        source: &source,
        params: &params,
        geometry: request.geometry,
        display_color_space: request.display,
        stages: request.stages,
        frame: BackdropFrameNumber(number),
        frame_slot: slot,
        reader: ConsumerStage::Fragment,
    };
    run_one_shot_blocking(&context, |command_buffer| {
        source.record_acquire_for(&context, command_buffer, ConsumerStage::Compute);
        isp.record(command_buffer, &frame(1, 2)).unwrap();
        // Стекло читает подложку в compute (чужая для ISP стадия).
        let backdrop = isp.backdrop_source(SubmissionSerial(1)).unwrap();
        backdrop.record_read_barrier(command_buffer, ConsumerStage::Compute);
        glass_reader.record_dispatch(
            command_buffer,
            0,
            &push,
            WorkgroupCount::covering(width, height, LocalSize::new(8, 8)?),
        )?;
        // Следующий кадр ISP перезаписывает изображение в том же буфере.
        isp.record(command_buffer, &frame(0, 3)).unwrap();
        source.record_release_for(&context, command_buffer, ConsumerStage::Compute);
        Ok(())
    })
    .unwrap();
    assert_no_validation_errors(&context);
}

/// После пересоздания изображения (смена размера) прежняя подложка устарела: производитель так и
/// отвечает, а свежая подложка актуальна.
#[test]
fn backdrop_becomes_stale_after_the_image_is_recreated() {
    let Some(context) = gpu_context_or_skip() else { return };
    let spec = ParamsSpec::default();
    let params = spec.build();
    let raw = flat_frame(&spec, 0.1);
    let gpu_source = GpuSource::new(&context, &raw, SourceLayout::Raw16);
    let source = gpu_source.source();
    let mut isp = ViewfinderIsp::new(&context, &test_config(ViewfinderStorageFormat::HalfFloatOnly)).unwrap();
    let full = RunRequest::whole_frame(&raw, &params, spec.display());
    record_and_submit(&context, &mut isp, &full, &source, 0, BackdropFrameNumber(1));
    isp.collect(0).unwrap();
    let old_backdrop = isp.backdrop_source(SubmissionSerial(1)).unwrap();
    assert!(isp.is_backdrop_current(&old_backdrop));

    // Тот же размер — то же изображение: подложка остаётся актуальной.
    record_and_submit(&context, &mut isp, &full, &source, 1, BackdropFrameNumber(2));
    isp.collect(1).unwrap();
    assert!(isp.is_backdrop_current(&old_backdrop));

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
    record_and_submit(&context, &mut isp, &cropped, &source, 0, BackdropFrameNumber(3));
    assert!(
        !isp.is_backdrop_current(&old_backdrop),
        "изображение пересоздано — старая подложка устарела"
    );
    let fresh = isp.backdrop_source(SubmissionSerial(3)).unwrap();
    assert!(isp.is_backdrop_current(&fresh));
    isp.collect(0).unwrap();
    assert!(!isp.is_backdrop_current(&old_backdrop));
    assert_no_validation_errors(&context);
}
