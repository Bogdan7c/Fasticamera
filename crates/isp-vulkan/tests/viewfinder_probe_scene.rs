//! Сцена-«пробник» стенда насыщения проходит через настоящий ISP видоискателя и покрывает весь диапазон выхода:
//! чёрные, белые и насыщенные пиксели. Без этого «наибольшая разница ≤ 1 разряда» между двумя шейдерами ничего бы
//! не доказывала. Сравнение картинок двух шейдеров — `gpu_vulkan::saturation::compare_image_bytes`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod lens_shading_support;
mod look_support;
mod viewfinder_support;

use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use isp_model::cfa::BayerPattern;
use isp_model::image::DisplayGamut;
use isp_model::params::IspParams;
use isp_model::tone::agx_settings::AgxSettings;
use isp_model::tone::base::ToneBase;
use isp_model::tone::working_io::WorkingInputGamut;
use isp_vulkan::saturation::{SceneContent, synthesize_raw10};
use isp_vulkan::viewfinder::{IspStageSet, OutputAllocation, ViewfinderGeometry, ViewfinderStorageFormat};
use lens_shading_support::{asymmetric_map, with_map};
use viewfinder_support::{
    ParamsSpec, RawFrame, RunRequest, SourceLayout, assert_no_validation_errors, gpu_context_or_skip, run_viewfinder,
    test_config,
};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 192;

/// Параметры в стиле продукта: AgX, вид «насыщенность и сочность» (как у Natural), карта теней.
fn product_like(soft_input: bool) -> IspParams {
    let input_gamut = if soft_input {
        WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION)
    } else {
        AgxSettings::default().input_gamut
    };
    let spec = ParamsSpec {
        pattern: BayerPattern::RGGB,
        gamut: DisplayGamut::DisplayP3,
        tone_base: ToneBase::Agx(AgxSettings {
            input_gamut,
            ..AgxSettings::default()
        }),
        ..ParamsSpec::default()
    };
    let params = spec.build_with_look(look_support::saturation_vibrance(0.15, 0.15));
    with_map(&params, &asymmetric_map(9, 7))
}

/// Параметры прежней основы без вида и без карты (ветка прежней основы шейдера).
#[allow(dead_code)]
fn legacy_plain() -> IspParams {
    ParamsSpec {
        pattern: BayerPattern::RGGB,
        ..ParamsSpec::default()
    }
    .build()
}

/// Картинка кадра `content` с опытными переключателями `bits`.
fn render(context: &gpu_vulkan::context::GpuContext, params: &IspParams, content: SceneContent) -> Vec<u8> {
    let frame = synthesize_raw10(PixelSize::new(WIDTH, HEIGHT), content, 0).unwrap();
    let raw = RawFrame {
        width: frame.width,
        height: frame.height,
        pattern: BayerPattern::RGGB,
        samples: frame.samples,
    };
    let full = PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(WIDTH, HEIGHT),
    };
    let request = RunRequest {
        layout: SourceLayout::Raw10 { padding_bytes: 0 },
        geometry: ViewfinderGeometry {
            valid_area: full,
            crop: full,
            sensor_orientation: SensorOrientation::Degrees90,
            allocation: OutputAllocation::Exact,
        },
        stages: IspStageSet::ALL,
        ..RunRequest::whole_frame(&raw, params, isp_support_display())
    };
    let config = test_config(ViewfinderStorageFormat::PreferTenBitUnorm);
    let run = run_viewfinder(context, &config, &request);
    // 10-бит выход читается как f32: переводим в коды так же, как их пишет шейдер (round(v · 1023)), для сравнения.
    run.pixels
        .iter()
        .flat_map(|pixel| {
            let code = |value: f32| (value * 1023.0).round() as u32;
            let word = code(pixel[0]) | (code(pixel[1]) << 10) | (code(pixel[2]) << 20) | (3 << 30);
            word.to_le_bytes()
        })
        .collect()
}

fn isp_support_display() -> gpu_vulkan::swapchain::DisplayColorSpace {
    gpu_vulkan::swapchain::DisplayColorSpace::DisplayP3
}

#[test]
fn probe_scene_reaches_black_white_and_saturated_colors_in_the_output() {
    let Some(context) = gpu_context_or_skip() else { return };
    let picture = render(&context, &product_like(false), SceneContent::Probe);
    let pixels: Vec<[u32; 3]> = picture
        .chunks_exact(4)
        .map(|bytes| {
            let word = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            [word & 0x3FF, (word >> 10) & 0x3FF, (word >> 20) & 0x3FF]
        })
        .collect();
    assert!(
        pixels.iter().any(|p| p.iter().all(|&c| c < 12)),
        "в выходе нет почти чёрных пикселей"
    );
    assert!(
        pixels.iter().any(|p| p.iter().all(|&c| c > 1000)),
        "в выходе нет почти белых пикселей"
    );
    let saturated = pixels
        .iter()
        .filter(|p| p.iter().max().unwrap() - p.iter().min().unwrap() > 400)
        .count();
    assert!(saturated > 500, "насыщенных пикселей мало: {saturated}");
}

#[test]
fn probe_covers_more_of_the_output_range_than_the_natural_scene() {
    let Some(context) = gpu_context_or_skip() else { return };
    let params = product_like(false);
    let spread = |content| {
        let picture = render(&context, &params, content);
        let green: Vec<u32> = picture
            .chunks_exact(4)
            .map(|bytes| (u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) >> 10) & 0x3FF)
            .collect();
        green.iter().max().unwrap() - green.iter().min().unwrap()
    };
    let (probe, natural) = (spread(SceneContent::Probe), spread(SceneContent::default()));
    assert!(probe > 1000, "пробник занимает почти весь диапазон 10-бит: {probe}");
    assert!(
        probe > natural + 200,
        "пробник шире естественной сцены: {probe} против {natural}"
    );
    assert_no_validation_errors(&context);
}
