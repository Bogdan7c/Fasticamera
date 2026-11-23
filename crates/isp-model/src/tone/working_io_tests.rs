//! Тесты общего шага рабочего пространства: матрицы, вход (обрезка нулём / мягкое сжатие), выход (сжатие,
//! обрезка 0…1), различимые ошибки и выгрузка чисел для GPU.

use super::*;
use crate::color::working_space::{WORKING_PRIMARIES, working_to_output, xyz_d50_to_working};
use crate::gamut::GamutCompression;

fn clip_settings() -> WorkingIoSettings {
    WorkingIoSettings {
        input_gamut: WorkingInputGamut::ClipToZero,
        output_gamut_compression: GamutCompressionSettings::default(),
    }
}

fn soft_settings() -> WorkingIoSettings {
    WorkingIoSettings {
        input_gamut: WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION),
        ..clip_settings()
    }
}

fn resolved(settings: &WorkingIoSettings, gamut: DisplayGamut) -> WorkingSpaceIo {
    WorkingSpaceIo::resolve(settings, gamut).unwrap()
}

#[test]
fn matrices_are_exactly_the_f32_narrowing_of_the_f64_working_space_matrices() {
    // Бит в бит с тем, что раньше считала AgxBase: те же функции, то же единственное сужение.
    for gamut in [DisplayGamut::Srgb, DisplayGamut::DisplayP3] {
        let io = resolved(&clip_settings(), gamut);
        let parameters = io.parameters();
        assert_eq!(
            parameters.xyz_d50_to_working,
            xyz_d50_to_working().unwrap().to_f32().rows
        );
        assert_eq!(
            parameters.working_to_output,
            working_to_output(gamut).unwrap().to_f32().rows
        );
        assert_eq!(io.output_gamut(), gamut);
    }
}

#[test]
fn clip_to_zero_drops_negatives_and_keeps_the_rest_untouched() {
    let io = resolved(&clip_settings(), DisplayGamut::Srgb);
    assert_eq!(io.apply_input([-0.3, 0.25, 4.0]), [0.0, 0.25, 4.0]);
    assert!(io.parameters().input_compression.is_none());
}

#[test]
fn soft_compression_pulls_out_of_gamut_in_and_leaves_grey_alone() {
    let io = resolved(&soft_settings(), DisplayGamut::Srgb);
    let reference = GamutCompression::resolve(&WorkingInputGamut::DEFAULT_COMPRESSION).unwrap();
    let outside = [-0.05_f32, 0.3, 0.5];
    // Результат — ровно сжатие охвата, а не обрезка нулём (иначе `-0.05` стал бы `0`).
    assert_eq!(io.apply_input(outside), reference.compress_pixel(outside));
    assert!(
        io.apply_input(outside)[0] > 0.0,
        "сжатие оставляет остаток света, обрезка дала бы 0"
    );
    assert_eq!(io.apply_input([0.2; 3]), [0.2; 3]);
    assert!(io.parameters().input_compression.is_some());
}

#[test]
fn to_output_maps_white_to_white_and_clamps_into_zero_one() {
    let io = resolved(&clip_settings(), DisplayGamut::Srgb);
    let white = io.to_output([1.0; 3]);
    for channel in white {
        assert!((channel - 1.0).abs() < 1.0e-5, "{white:?}");
    }
    // Насыщенный цвет Rec. 2020 за охватом sRGB: после сжатия и обрезки каждый канал в [0, 1].
    let vivid = io.to_output([0.0, 1.0, 0.0]);
    assert!(vivid.iter().all(|channel| (0.0..=1.0).contains(channel)), "{vivid:?}");
    // Свет выше белого обрезается до 1: вывод никогда не выходит за [0, 1].
    assert!(io.to_output([8.0; 3]).iter().all(|channel| *channel <= 1.0));
}

#[test]
fn to_output_is_the_matrix_then_compression_then_clamp() {
    let io = resolved(&clip_settings(), DisplayGamut::DisplayP3);
    let compression = GamutCompression::resolve(&GamutCompressionSettings::default()).unwrap();
    let working = [0.1_f32, 0.8, 0.2];
    let expected = compression
        .compress_pixel(io.working_to_output_colors(working))
        .map(|channel| channel.clamp(0.0, 1.0));
    assert_eq!(io.to_output(working), expected);
    // Матрица W → выход переводит основные цвета Rec. 2020: серый остаётся серым.
    let grey = io.working_to_output_colors([0.3; 3]);
    assert!(grey.iter().all(|channel| (channel - 0.3).abs() < 1.0e-5), "{grey:?}");
}

#[test]
fn invalid_compression_is_reported_with_the_side_it_belongs_to() {
    let broken = GamutCompressionSettings {
        threshold: [1.5; 3],
        ..GamutCompressionSettings::default()
    };
    let input = WorkingIoSettings {
        input_gamut: WorkingInputGamut::SoftCompress(broken),
        ..clip_settings()
    };
    assert!(matches!(
        WorkingSpaceIo::resolve(&input, DisplayGamut::Srgb),
        Err(InvalidWorkingIo::InputGamut(_))
    ));
    let output = WorkingIoSettings {
        output_gamut_compression: broken,
        ..clip_settings()
    };
    assert!(matches!(
        WorkingSpaceIo::resolve(&output, DisplayGamut::Srgb),
        Err(InvalidWorkingIo::OutputGamut(_))
    ));
}

#[test]
fn input_gamut_survives_json_round_trip_in_the_config_spelling() {
    // Формат конфига не изменился при переносе типа: snake_case, структурный вариант.
    let json = serde_json::to_string(&WorkingInputGamut::ClipToZero).unwrap();
    assert_eq!(json, "\"clip_to_zero\"");
    let soft = WorkingInputGamut::SoftCompress(WorkingInputGamut::DEFAULT_COMPRESSION);
    let round: WorkingInputGamut = serde_json::from_str(&serde_json::to_string(&soft).unwrap()).unwrap();
    assert_eq!(round, soft);
    // Рабочее пространство — Rec. 2020, как договорено решением 176.
    assert_eq!(WORKING_PRIMARIES, crate::color::chromaticity::Primaries::REC_2020);
}
