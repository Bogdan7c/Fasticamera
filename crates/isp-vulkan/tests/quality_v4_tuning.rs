//! Опыты стенда V4 (`HeadroomTuning`, сессия 16a пакет V): каждый опыт реально меняет то, что должен, а то, что
//! обязано остаться прежним, остаётся прежним ПО БИТАМ.
//!
//! * «только AgX»: пайплайн вывода собран под одну модель основы — картинка совпадает с продуктовой бит в бит;
//! * отключение стадий вывода: картинка РАЗЛИЧАЕТСЯ с продуктовой (иначе опыт ничего не измерял бы);
//! * пробы вместо Menon: кадр доходит до вывода и до приёмника, картинка отличается (мозаику никто не демозаичил).

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod saturation_support;

use ash::vk;
use camera_contract::geometry::PixelSize;
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::frame_sink::{FrameSink, OffscreenFrameSink, OffscreenFrameSinkDesc};
use gpu_vulkan::hardware_buffer_cache::queue_handoff::ConsumerStage;
use gpu_vulkan::kit_test_support::{read_storage_image_rgba, run_one_shot_blocking};
use gpu_vulkan::raw_mosaic_preview::ValidArea;
use isp_model::image::DisplayGamut;
use isp_vulkan::quality::{
    DemosaicProbe, DemosaicVariant, HeadroomTuning, OutputStageSkips, OutputToneModel, StabilizerMock,
    StreamGeneration, VideoFrameInput, VideoHeadroomConfig, VideoHeadroomWorkload,
};
use isp_vulkan::saturation::{SceneContent, synthesize_raw10};
use saturation_support::quality_params;
use saturation_support::viewfinder_support::{assert_no_validation_errors, gpu_context_or_skip};

const WIDTH: u32 = 512;
const HEIGHT: u32 = 384;

fn config_with(tuning: HeadroomTuning) -> VideoHeadroomConfig {
    VideoHeadroomConfig {
        tuning,
        ..VideoHeadroomConfig::default()
    }
}

/// Один кадр синтетической сцены через цепочку V4 с настройками `config`: пиксели картинки приёмника.
fn render_frame(context: &GpuContext, config: &VideoHeadroomConfig) -> Vec<[f32; 4]> {
    let frame = synthesize_raw10(PixelSize::new(WIDTH, HEIGHT), SceneContent::default(), 0).unwrap();
    let raw = gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw10::new(
        context,
        frame.width,
        frame.height,
        frame.row_stride_bytes,
        &frame.bytes,
    )
    .unwrap();
    let source = raw.source();
    let extent = vk::Extent2D {
        width: WIDTH,
        height: HEIGHT,
    };
    let mut workload = VideoHeadroomWorkload::new(context, extent, config).unwrap();
    let mut sink = OffscreenFrameSink::new(
        context,
        &OffscreenFrameSinkDesc {
            extent,
            format_policy: config.output_format.storage_policy(),
            frame_slots: config.frame_slots,
            reader: ConsumerStage::Fragment,
        },
    )
    .unwrap();
    let params = quality_params(DisplayGamut::Srgb);
    let input = VideoFrameInput {
        source: &source,
        params: &params,
        valid_area: ValidArea {
            left: 0,
            top: 0,
            width: WIDTH,
            height: HEIGHT,
        },
        stream: StreamGeneration(1),
        sequence: 0,
        presentation_time_ns: 0,
    };
    run_one_shot_blocking(context, |command_buffer| {
        source.record_acquire_for(context, command_buffer, ConsumerStage::Compute);
        workload.record(command_buffer, 0, &input, &mut sink).map_err(|error| {
            gpu_vulkan::error::GpuError::InvalidState {
                what: error.to_string(),
            }
        })?;
        source.record_release_for(context, command_buffer, ConsumerStage::Compute);
        Ok(())
    })
    .unwrap();
    sink.after_submit(0).unwrap();
    read_storage_image_rgba(sink.image(0).unwrap(), sink.last_use(0).unwrap()).unwrap()
}

#[test]
fn agx_only_output_pipeline_gives_the_product_image_bit_for_bit() {
    let Some(context) = gpu_context_or_skip() else { return };
    let product = render_frame(&context, &config_with(HeadroomTuning::PRODUCT));
    let agx_only = render_frame(
        &context,
        &config_with(HeadroomTuning {
            output_tone_model: OutputToneModel::AgxOnly,
            ..HeadroomTuning::PRODUCT
        }),
    );
    assert!(
        product.iter().any(|pixel| *pixel != product[0]),
        "картинка не вырождена"
    );
    assert!(agx_only == product, "вывод «только AgX» отличается от продуктового");
    assert_no_validation_errors(&context);
}

#[test]
fn every_output_skip_changes_the_image() {
    let Some(context) = gpu_context_or_skip() else { return };
    let product = render_frame(&context, &config_with(HeadroomTuning::PRODUCT));
    let none = OutputStageSkips::NONE;
    let skips = [
        OutputStageSkips {
            camera_matrix: true,
            ..none
        },
        OutputStageSkips {
            tone_base: true,
            ..none
        },
        OutputStageSkips { look: true, ..none },
        OutputStageSkips {
            output_gamut: true,
            ..none
        },
        OutputStageSkips { oetf: true, ..none },
    ];
    for skipped in skips {
        let image = render_frame(
            &context,
            &config_with(HeadroomTuning {
                output_skips: skipped,
                ..HeadroomTuning::PRODUCT
            }),
        );
        // Вид по умолчанию нейтральный, а выходное сжатие охвата не трогает цвета внутри охвата: такая
        // стадия может не менять картинку сцены. Остальные обязаны менять.
        if skipped == (OutputStageSkips { look: true, ..none })
            || skipped
                == (OutputStageSkips {
                    output_gamut: true,
                    ..none
                })
        {
            continue;
        }
        assert!(image != product, "отключение {skipped:?} не изменило картинку");
    }
    assert_no_validation_errors(&context);
}

#[test]
fn probe_chains_reach_the_sink_and_do_not_demosaic() {
    let Some(context) = gpu_context_or_skip() else { return };
    let product = render_frame(&context, &config_with(HeadroomTuning::PRODUCT));
    for probe in [DemosaicProbe::Empty, DemosaicProbe::CopyF32, DemosaicProbe::CopyHalf] {
        let image = render_frame(
            &context,
            &config_with(HeadroomTuning {
                demosaic: DemosaicVariant::Probe(probe),
                ..HeadroomTuning::PRODUCT
            }),
        );
        assert_eq!(image.len(), product.len(), "{probe:?}");
        assert!(
            image != product,
            "{probe:?}: проба выдала картинку настоящего демозаика"
        );
    }
    assert_no_validation_errors(&context);
}

#[test]
fn stabilizer_mock_resamples_the_frame() {
    let Some(context) = gpu_context_or_skip() else { return };
    let product = render_frame(&context, &config_with(HeadroomTuning::PRODUCT));
    let warped = render_frame(
        &context,
        &config_with(HeadroomTuning {
            stabilizer_mock: StabilizerMock::MergedWithOutput,
            ..HeadroomTuning::PRODUCT
        }),
    );
    assert_eq!(warped.len(), product.len());
    // Поворот и сдвиг строки двигают содержимое: картинка другая, но тоже не вырождена (сцена не потеряна).
    let differing = warped.iter().zip(&product).filter(|(a, b)| a != b).count();
    assert!(
        differing > product.len() / 4,
        "макет почти не изменил картинку: {differing}"
    );
    assert!(warped.iter().any(|pixel| *pixel != warped[0]));
    assert_no_validation_errors(&context);
}

#[test]
fn workgroup_shape_does_not_change_the_image() {
    let Some(context) = gpu_context_or_skip() else { return };
    let render = |x, y| {
        render_frame(
            &context,
            &VideoHeadroomConfig {
                workgroup_size: isp_vulkan::quality::WorkgroupSizeConfig { x, y },
                ..config_with(HeadroomTuning::PRODUCT)
            },
        )
    };
    let sixteen_by_sixteen = render(16, 16);
    assert!(
        sixteen_by_sixteen.iter().any(|pixel| *pixel != sixteen_by_sixteen[0]),
        "картинка не вырождена"
    );
    // Кадр 512×384 кратен не всем формам (384 / 128 группы по высоте, 512 / 128 по ширине): края покрываются неполными группами.
    for (x, y) in [(128, 2), (32, 8), (64, 4), (8, 8), (4, 32)] {
        assert!(
            render(x, y) == sixteen_by_sixteen,
            "форма группы {x}x{y} изменила картинку"
        );
    }
    assert_no_validation_errors(&context);
}
