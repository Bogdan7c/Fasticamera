//! Запись DNG: каждый тег сверяется с ожидаемым значением из паспорта OnePlus 11 и фактов кадра;
//! отсчёты в файле — бит-в-бит входу; негодный вход отвергается с объяснением.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "тесты")]

mod support;

use camera_contract::capabilities::SensorLevels;
use camera_contract::frame_passport::{NoiseModelCoefficients, NoiseProfile};
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern};
use photo_output::dng::{DngError, DngInputs, DngWriteOptions, write_dng, write_dng_with_options};
use still_contract::shot::{RawSamples16, StillFrameFacts, StillRotation};
use support::still_fixture::{
    camera_description, capture_moment, main_camera_calibration, main_camera_stream, still_facts, synthetic_raw,
};
use support::tiff_reader::{Order, ParsedIfd, ParsedTiff};

const RAW_SIZE: PixelSize = PixelSize::new(16, 12);

fn area_at(left: u32, top: u32) -> PixelRect {
    PixelRect {
        left,
        top,
        size: RAW_SIZE,
    }
}

/// Кадрирование: внутри полезной области, отступ (2, 4), размер 10×6.
fn crop_inside(area: PixelRect) -> PixelRect {
    PixelRect {
        left: area.left + 2,
        top: area.top + 4,
        size: PixelSize::new(10, 6),
    }
}

fn facts_at(left: u32, top: u32) -> StillFrameFacts {
    let area = area_at(left, top);
    still_facts(main_camera_stream(area), crop_inside(area))
}

fn dng_bytes_with(facts: &StillFrameFacts, raw: &RawSamples16, options: DngWriteOptions) -> Result<Vec<u8>, DngError> {
    let camera = camera_description();
    let moment = capture_moment();
    let mut bytes = Vec::new();
    write_dng_with_options(
        &DngInputs {
            facts,
            raw,
            camera: &camera,
            captured_at: &moment,
        },
        options,
        &mut bytes,
    )?;
    Ok(bytes)
}

fn dng_bytes(facts: &StillFrameFacts) -> Vec<u8> {
    dng_bytes_with(facts, &synthetic_raw(RAW_SIZE), DngWriteOptions::default()).unwrap()
}

fn invalid_input_message(result: Result<Vec<u8>, DngError>) -> String {
    match result {
        Err(DngError::InvalidInput(message)) => message,
        other => panic!("ожидали InvalidInput, получили {other:?}"),
    }
}

fn ifd0(bytes: &[u8]) -> ParsedIfd {
    ParsedTiff::open(bytes).first_ifd()
}

fn cfa_pattern(facts: &StillFrameFacts) -> Vec<u8> {
    ifd0(&dng_bytes(facts)).entry(33422).bytes().to_vec()
}

#[test]
fn ifd0_carries_the_whole_expected_tag_set_with_values_from_passport_and_facts() {
    let facts = facts_at(0, 0);
    let bytes = dng_bytes(&facts);
    let tiff = ParsedTiff::open(&bytes);
    assert_eq!(
        tiff.order,
        Order::Little,
        "DNG — little-endian, как отсчёты кадра и `DngCreator`"
    );
    let ifd = tiff.first_ifd();

    // Полный набор тегов: ничего лишнего (без миниатюры, без SubIFD) и ничего не потеряно.
    assert_eq!(
        ifd.tags(),
        vec![
            254, 256, 257, 258, 259, 262, 271, 272, 273, 274, 277, 278, 279, 284, 305, 306, 33421, 33422, 34665, 50706,
            50707, 50708, 50710, 50711, 50713, 50714, 50717, 50719, 50720, 50721, 50722, 50723, 50724, 50728, 50730,
            50778, 50779, 50964, 50965
        ]
    );
    assert_eq!(ifd.entry(254).longs(), vec![0], "NewSubFileType: основное изображение");
    assert_eq!(ifd.entry(256).longs(), vec![16]);
    assert_eq!(ifd.entry(257).longs(), vec![12]);
    assert_eq!(ifd.entry(258).shorts(), vec![16]);
    assert_eq!(ifd.entry(259).shorts(), vec![1]);
    assert_eq!(ifd.entry(262).shorts(), vec![32803], "CFA");
    assert_eq!(ifd.entry(277).shorts(), vec![1]);
    assert_eq!(ifd.entry(284).shorts(), vec![1]);
    assert_eq!(ifd.entry(271).ascii(), "OnePlus");
    assert_eq!(ifd.entry(272).ascii(), "CPH2447");
    assert_eq!(ifd.entry(305).ascii(), "Fasticamera 0.1");
    assert_eq!(ifd.entry(306).ascii(), "2026:10:25 20:48:05");
    assert_eq!(ifd.entry(274).shorts(), vec![6], "Clockwise90 → Orientation 6");

    assert_eq!(ifd.entry(50706).bytes(), &[1, 4, 0, 0], "DNGVersion");
    assert_eq!(ifd.entry(50707).bytes(), &[1, 1, 0, 0], "DNGBackwardVersion");
    assert_eq!(ifd.entry(50708).ascii(), "CPH2447-OnePlus-OnePlus camera 2");
    assert_eq!(ifd.entry(33421).shorts(), vec![2, 2]);
    assert_eq!(ifd.entry(50710).bytes(), &[0, 1, 2], "CFAPlaneColor: R, G, B");
    assert_eq!(ifd.entry(50711).shorts(), vec![1], "CFALayout: прямоугольная");
    assert_eq!(ifd.entry(50713).shorts(), vec![2, 2]);
    assert_eq!(
        ifd.entry(50714).rational_values(),
        vec![64.0; 4],
        "статический чёрный фикстуры"
    );
    assert_eq!(ifd.entry(50717).longs(), vec![1023]);
    // Кадрирование относительно полезной области: отступ (2, 4), размер 10×6.
    assert_eq!(ifd.entry(50719).longs(), vec![2, 4]);
    assert_eq!(ifd.entry(50720).longs(), vec![10, 6]);

    let calibration = main_camera_calibration();
    for (index, set) in calibration.iter().enumerate() {
        let tags = if index == 0 {
            (50721, 50723, 50964, 50778)
        } else {
            (50722, 50724, 50965, 50779)
        };
        let pairs = |matrix: &camera_contract::capabilities::Matrix3x3| {
            matrix
                .0
                .iter()
                .map(|r| (r.numerator, r.denominator))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ifd.entry(tags.0).srationals(),
            pairs(&set.color_matrix),
            "ColorMatrix{}",
            index + 1
        );
        assert_eq!(
            ifd.entry(tags.1).srationals(),
            pairs(&set.calibration_transform),
            "CameraCalibration{}",
            index + 1
        );
        assert_eq!(
            ifd.entry(tags.2).srationals(),
            pairs(&set.forward_matrix),
            "ForwardMatrix{}",
            index + 1
        );
        assert_eq!(ifd.entry(tags.3).shorts(), vec![u16::from(set.illuminant.exif_code())]);
    }
    assert_eq!(ifd.entry(50778).shorts(), vec![21], "иллюминант 1: D65");
    assert_eq!(ifd.entry(50779).shorts(), vec![17], "иллюминант 2: лампа A");

    // AsShotNeutral — НАША нейтраль из фактов; BaselineExposure — подтяжка +0,25 EV.
    let neutral = ifd.entry(50728).rational_values();
    for (written, expected) in neutral.iter().zip(facts.neutral.0) {
        assert!((written - expected).abs() < 1e-6, "{written} против {expected}");
    }
    assert_eq!(ifd.entry(50730).srational_values(), vec![0.25]);
    assert!(
        ifd.maybe(51041).is_none(),
        "NoiseProfile не пишется, пока HAL профиль не дал"
    );
}

#[test]
fn exif_ifd_inside_dng_has_capture_tags() {
    let bytes = dng_bytes(&facts_at(0, 0));
    let tiff = ParsedTiff::open(&bytes);
    let ifd = tiff.first_ifd();
    let exif = tiff.sub_ifd(ifd.entry(34665));
    assert_eq!(exif.entry(33434).rationals(), vec![(22_450_523, 1_000_000_000)]);
    assert_eq!(exif.entry(34855).shorts(), vec![249]);
    assert_eq!(exif.entry(33437).rationals(), vec![(1800, 1000)]);
    assert_eq!(exif.entry(37386).rationals(), vec![(5590, 1000)]);
    assert_eq!(exif.entry(36867).ascii(), "2026:10:25 20:48:05");
    assert_eq!(exif.entry(37521).ascii(), "250");
    assert_eq!(exif.entry(36881).ascii(), "+03:00");
}

#[test]
fn cfa_pattern_follows_parity_of_the_valid_area_origin() {
    // Буфер BGGR: B G / G R; коды DNG: R=0, G=1, B=2.
    assert_eq!(cfa_pattern(&facts_at(0, 0)), vec![2, 1, 1, 0], "чётный старт: BGGR");
    assert_eq!(cfa_pattern(&facts_at(1, 0)), vec![1, 2, 0, 1], "нечётный столбец: GBRG");
    assert_eq!(cfa_pattern(&facts_at(0, 1)), vec![1, 0, 2, 1], "нечётная строка: GRBG");
    assert_eq!(cfa_pattern(&facts_at(1, 1)), vec![0, 1, 1, 2], "нечётные оба: RGGB");
    // Чётный сдвиг (на 2 пикселя) узор не меняет.
    assert_eq!(cfa_pattern(&facts_at(2, 4)), vec![2, 1, 1, 0]);
}

#[test]
fn all_four_layouts_map_to_their_dng_codes() {
    for (layout, expected) in [
        (CfaLayout::Rggb, vec![0u8, 1, 1, 2]),
        (CfaLayout::Grbg, vec![1, 0, 2, 1]),
        (CfaLayout::Gbrg, vec![1, 2, 0, 1]),
        (CfaLayout::Bggr, vec![2, 1, 1, 0]),
    ] {
        let mut facts = facts_at(0, 0);
        let mut stream = (*facts.stream).clone();
        stream.mosaic.layout = layout;
        facts.stream = stream.into();
        assert_eq!(cfa_pattern(&facts), expected, "{layout:?}");
    }
}

fn with_levels(facts: &mut StillFrameFacts, pattern: [u32; 4], white: u32) {
    let mut stream = (*facts.stream).clone();
    stream.static_levels = SensorLevels {
        black_level_pattern: pattern,
        white_level: white,
    };
    facts.stream = stream.into();
}

#[test]
fn black_level_comes_from_static_pattern_without_dynamic_and_is_shifted_with_the_cfa() {
    let mut facts = facts_at(1, 0);
    with_levels(&mut facts, [60, 61, 62, 63], 1023);
    // Нечётный столбец: позиции 2×2 записанного изображения = (1, 0, 3, 2) позиций буфера.
    assert_eq!(
        ifd0(&dng_bytes(&facts)).entry(50714).rational_values(),
        vec![61.0, 60.0, 63.0, 62.0]
    );

    let mut facts = facts_at(0, 0);
    with_levels(&mut facts, [60, 61, 62, 63], 1023);
    assert_eq!(
        ifd0(&dng_bytes(&facts)).entry(50714).rational_values(),
        vec![60.0, 61.0, 62.0, 63.0]
    );
}

#[test]
fn dynamic_black_and_white_levels_win_over_static_ones() {
    let mut facts = facts_at(0, 1);
    with_levels(&mut facts, [60, 61, 62, 63], 1023);
    facts.capture.color.dynamic_black_level = Some([64.25, 65.5, 66.0, 67.75]);
    facts.capture.color.dynamic_white_level = Some(1000);
    let ifd = ifd0(&dng_bytes(&facts));
    // Нечётная строка: позиции (2, 3, 0, 1).
    assert_eq!(ifd.entry(50714).rational_values(), vec![66.0, 67.75, 64.25, 65.5]);
    assert_eq!(ifd.entry(50717).longs(), vec![1000]);
}

#[test]
fn zero_dynamic_white_level_falls_back_to_static() {
    let mut facts = facts_at(0, 0);
    facts.capture.color.dynamic_white_level = Some(0);
    assert_eq!(ifd0(&dng_bytes(&facts)).entry(50717).longs(), vec![1023]);
}

#[test]
fn samples_are_written_bit_for_bit_as_strips_for_any_strip_size() {
    let facts = facts_at(0, 0);
    let raw = synthetic_raw(RAW_SIZE);
    // 16 отсчётов × 2 байта = 32 байта в строке: 100 байт → 3 строки в полосе (12 строк → 4 полосы),
    // 1 байт → по строке на полосу, огромное значение → одна полоса.
    for (target, expected_strips, expected_rows) in [(100usize, 4usize, 3u32), (1, 12, 1), (1 << 30, 1, 12)] {
        let bytes = dng_bytes_with(
            &facts,
            &raw,
            DngWriteOptions {
                target_strip_bytes: target,
            },
        )
        .unwrap();
        let tiff = ParsedTiff::open(&bytes);
        let ifd = tiff.first_ifd();
        assert_eq!(ifd.entry(273).count as usize, expected_strips, "target {target}");
        assert_eq!(ifd.entry(278).longs(), vec![expected_rows]);
        assert_eq!(
            tiff.strip_data(&ifd),
            raw.le_bytes(),
            "отсчёты изменились при target {target}"
        );
    }
    // И с настройками по умолчанию.
    let bytes = dng_bytes(&facts);
    let tiff = ParsedTiff::open(&bytes);
    assert_eq!(tiff.strip_data(&tiff.first_ifd()), raw.le_bytes());
}

#[test]
fn rotation_is_written_as_orientation() {
    for (rotation, expected) in [
        (StillRotation::None, 1),
        (StillRotation::Clockwise90, 6),
        (StillRotation::Clockwise180, 3),
        (StillRotation::Clockwise270, 8),
    ] {
        let mut facts = facts_at(0, 0);
        facts.rotation = rotation;
        assert_eq!(ifd0(&dng_bytes(&facts)).entry(274).shorts(), vec![expected]);
    }
}

#[test]
fn noise_profile_is_written_per_color_plane_with_greens_averaged() {
    let mut facts = facts_at(0, 0);
    let pair = |scale: f64, offset: f64| NoiseModelCoefficients { scale, offset };
    // Позиции буфера BGGR: [B, G, G, R].
    facts.capture.color.noise_profile = Some(NoiseProfile([
        pair(0.4, 4e-7),
        pair(0.2, 2e-7),
        pair(0.3, 3e-7),
        pair(0.1, 1e-7),
    ]));
    let written = ifd0(&dng_bytes(&facts)).entry(51041).doubles();
    // Порядок плоскостей — как у CFAPlaneColor: R, G, B.
    let expected = [0.1, 1e-7, 0.25, 2.5e-7, 0.4, 4e-7];
    assert_eq!(written.len(), 6);
    for (got, want) in written.iter().zip(expected) {
        assert!((got - want).abs() < 1e-12, "{written:?} против {expected:?}");
    }
}

#[test]
fn invalid_noise_profile_is_dropped_but_the_dng_is_still_written() {
    let mut facts = facts_at(0, 0);
    facts.capture.color.noise_profile = Some(NoiseProfile(
        [NoiseModelCoefficients {
            scale: -1.0,
            offset: 0.0,
        }; 4],
    ));
    assert!(ifd0(&dng_bytes(&facts)).maybe(51041).is_none());
}

#[test]
fn single_calibration_set_writes_only_first_illuminant_tags() {
    let mut facts = facts_at(0, 0);
    let mut stream = (*facts.stream).clone();
    stream.color_calibration.truncate(1);
    facts.stream = stream.into();
    let ifd = ifd0(&dng_bytes(&facts));
    for present in [50721, 50723, 50964, 50778] {
        assert!(ifd.maybe(present).is_some(), "нет тега {present}");
    }
    for absent in [50722, 50724, 50965, 50779] {
        assert!(ifd.maybe(absent).is_none(), "лишний тег {absent}");
    }
}

#[test]
fn wrong_sample_size_is_rejected_and_explained() {
    let facts = facts_at(0, 0);
    let raw = synthetic_raw(PixelSize::new(16, 10));
    let message = invalid_input_message(dng_bytes_with(&facts, &raw, DngWriteOptions::default()));
    assert!(message.contains("полезной области"), "{message}");
}

#[test]
fn missing_color_calibration_is_rejected() {
    let mut facts = facts_at(0, 0);
    let mut stream = (*facts.stream).clone();
    stream.color_calibration.clear();
    facts.stream = stream.into();
    let message = invalid_input_message(dng_bytes_with(
        &facts,
        &synthetic_raw(RAW_SIZE),
        DngWriteOptions::default(),
    ));
    assert!(message.contains("калибровки"), "{message}");
}

#[test]
fn non_bayer_mosaic_is_rejected() {
    for mosaic in [
        MosaicPattern {
            kind: MosaicKind::QuadBayer,
            layout: CfaLayout::Bggr,
        },
        MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Mono,
        },
    ] {
        let mut facts = facts_at(0, 0);
        let mut stream = (*facts.stream).clone();
        stream.mosaic = mosaic;
        facts.stream = stream.into();
        let message = invalid_input_message(dng_bytes_with(
            &facts,
            &synthetic_raw(RAW_SIZE),
            DngWriteOptions::default(),
        ));
        assert!(message.contains("Bayer"), "{message}");
    }
}

#[test]
fn crop_outside_valid_area_and_bad_levels_are_rejected() {
    let raw = synthetic_raw(RAW_SIZE);

    let mut facts = facts_at(4, 4);
    facts.crop = PixelRect {
        left: 0, // левее полезной области
        top: 4,
        size: PixelSize::new(8, 8),
    };
    let message = invalid_input_message(dng_bytes_with(&facts, &raw, DngWriteOptions::default()));
    assert!(message.contains("кадрирование"), "{message}");

    let mut facts = facts_at(0, 0);
    with_levels(&mut facts, [64, 64, 64, 2000], 1023);
    let message = invalid_input_message(dng_bytes_with(&facts, &raw, DngWriteOptions::default()));
    assert!(message.contains("белого"), "{message}");

    let mut facts = facts_at(0, 0);
    facts.neutral.0[1] = 0.0;
    let message = invalid_input_message(dng_bytes_with(&facts, &raw, DngWriteOptions::default()));
    assert!(message.contains("нейтраль"), "{message}");
}

#[test]
fn valid_area_beyond_the_buffer_is_rejected() {
    let mut facts = facts_at(0, 0);
    let mut stream = (*facts.stream).clone();
    stream.valid_area = area_at(4090, 0); // 4090 + 16 > 4096
    facts.crop = crop_inside(stream.valid_area);
    facts.stream = stream.into();
    let message = invalid_input_message(dng_bytes_with(
        &facts,
        &synthetic_raw(RAW_SIZE),
        DngWriteOptions::default(),
    ));
    assert!(message.contains("буфер"), "{message}");
}

#[test]
fn rejected_input_writes_nothing_and_sink_errors_surface_as_io() {
    let facts = facts_at(0, 0);
    let camera = camera_description();
    let moment = capture_moment();
    let raw = synthetic_raw(PixelSize::new(16, 10));
    let mut out = Vec::new();
    let inputs = DngInputs {
        facts: &facts,
        raw: &raw,
        camera: &camera,
        captured_at: &moment,
    };
    assert!(write_dng(&inputs, &mut out).is_err());
    assert!(out.is_empty(), "отказ до записи не оставляет огрызка файла");

    struct FailingSink;
    impl std::io::Write for FailingSink {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("диск полон"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let raw_ok = synthetic_raw(RAW_SIZE);
    let inputs = DngInputs { raw: &raw_ok, ..inputs };
    assert!(matches!(write_dng(&inputs, &mut FailingSink), Err(DngError::Io(_))));
}
