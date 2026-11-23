//! Собирает НАШ DNG из эталонного кадра зонда (`~/fasticamera-captures/c7-reference/captures/<run>/`):
//! `*.stream.raw16` (весь буфер, 16 бит little-endian) + `*.passport.json` (паспорт зонда, раздел `stream_frame`).
//! Нужен для сверки с платформенным `DngCreator` (`tools/dng-check/compare_with_dngcreator.py`).
//!
//! Запуск: `cargo run -p photo-output --example dng_from_capture -- <каталог_прогона> <выход.dng>
//! [--section stream_frame|still_frame] [--raw16 <файл.raw16>] [--lens-shading <lens-shading.json>]`
//!
//! `--section` выбирает, чей паспорт читать: `stream_frame` (по умолчанию, кадр потока, которому принадлежит
//! `*.stream.raw16`) или `still_frame` (кадр, из которого платформа сделала свой `*.dng`; нужен, чтобы
//! собрать DNG с метаданными платформенного, подложив его же пиксели).
//!
//! `--raw16` подменяет отсчёты (для сверки на снятом приложением плоском поле); `--lens-shading` кладёт в DNG
//! `OpcodeList2` с картой виньетирования камеры из профиля устройства (`tools/dng-check/lens_shading_e2e.py`).
//!
//! Допущения примера: нейтраль берётся из паспорта (`neutral_color_point` — подсказка HAL, у зонда нашего
//! AWB нет), кадрирование — весь кадр, поворот 0, цифровая подтяжка 0, время — время изменения файла (UTC).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "пример для разработчика, не продуктовый код"
)]

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, Matrix3x3, Rational, ReferenceIlluminant, SensorLevels, VendorProcessingFlags,
};
use camera_contract::capture_result::{CaptureResultMeta, FrameNumber, SensorTimestamp};
use camera_contract::frame_passport::{
    CaptureColorHints, FrameCaptureDetails, NoiseModelCoefficients, NoiseProfile, StreamPassport,
};
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::ids::CameraId;
use camera_contract::lens_shading::LensShadingGainGrid;
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use camera_contract::three_a::{RequestOrigin, ThreeAResultState};
use photo_output::dng::{DngInputs, write_dng};
use photo_output::exif::{CameraDescription, CaptureMoment, LensDescription};
use serde_json::Value;
use shell_contract::clock::MonotonicNanos;
use still_contract::frame_choice::FrameChoice;
use still_contract::lens_shading::{AppliedLensShading, LensShadingNotAppliedReason};
use still_contract::plan::{PrimaryFormat, RawOutput, StillId, StillOutputs};
use still_contract::shot::{AsShotNeutral, DigitalGainEv, OwnedPixels, RawSamples16, StillFrameFacts, StillRotation};

/// Знаменатели, в которых HAL отдаёт числа матриц: двоичные (фронт: n/128) и десятичные (основная: n/10000).
/// JSON зонда хранит уже поделённое double; берём первый знаменатель, дающий то же число без потерь, чтобы
/// пример не вносил своей ошибки квантования в сверку.
const MATRIX_DENOMINATORS: [i32; 7] = [128, 256, 1024, 10_000, 65_536, 100_000, 1_000_000];
/// Допуск «знаменатель подошёл»: double из JSON отличается от дроби только округлением печати.
const MATRIX_EXACT_TOLERANCE: f64 = 1e-9;

fn main() {
    let options = CommandLine::parse(std::env::args().skip(1));
    let run_dir = options.run_dir;
    let output = options.output;

    let raw_path = single_file_with_suffix(&run_dir, ".stream.raw16");
    let passport_path = single_file_with_suffix(&run_dir, ".passport.json");
    let passport: Value = serde_json::from_slice(&std::fs::read(&passport_path).unwrap()).unwrap();
    let section = options.section;
    let frame = &passport[section.as_str()];

    let file_name = raw_path.file_name().unwrap().to_string_lossy().into_owned();
    // Имена зонда: `<id камеры>_<сцена>.stream.raw16`.
    let camera_id = file_name.split('_').next().unwrap().to_owned();

    let (array_width, array_height) = parse_size(frame["sensor"]["pixel_array"].as_str().unwrap());
    // `--raw16` подменяет отсчёты кадра потока (например, плоское поле, снятое приложением): паспорт остаётся прогона.
    let raw_bytes = std::fs::read(options.raw16_override.as_ref().unwrap_or(&raw_path)).unwrap();
    // Размер буфера: обычно равен `pixel_array`; у фронтальной камеры RAW идёт с биннингом 2×2
    // (`raw_binning_factor_used`), и буфер вдвое меньше по каждой стороне.
    let (width, height) = if raw_bytes.len() == (array_width * array_height * 2) as usize {
        (array_width, array_height)
    } else if raw_bytes.len() == (array_width / 2 * (array_height / 2) * 2) as usize {
        (array_width / 2, array_height / 2)
    } else {
        panic!(
            "размер {} байт не подходит ни к {array_width}x{array_height}, ни к половине",
            raw_bytes.len()
        );
    };
    let size = PixelSize::new(width, height);
    let raw = RawSamples16::new(size, Arc::new(OwnedPixels(raw_bytes))).expect("размер raw16 не совпал с pixel_array");

    let mut facts = facts_from_passport(frame, &camera_id, size);
    if let Some(profile_path) = &options.lens_shading_profile {
        // Описываем в DNG ту же поправку, которую применил бы снимок приложения (карта = калибровка камеры
        // над полезной областью; здесь полезная область — вся активная, значит пересчёта нет).
        facts.lens_shading =
            AppliedLensShading::Applied(Arc::new(read_lens_shading_grid(profile_path, &camera_id, size)));
    }
    let modified = std::fs::metadata(&raw_path).unwrap().modified().unwrap();
    let utc_millis = i64::try_from(
        modified
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO)
            .as_millis(),
    )
    .unwrap();
    let lens = LensDescription {
        focal_length_mm: first_f32(&frame["sensor"]["focal_lengths"]),
        f_number: first_f32(&frame["sensor"]["apertures"]),
    };
    let camera = CameraDescription {
        make: "OnePlus".to_owned(),
        model: "CPH2447".to_owned(),
        // Как у DngCreator на этом телефоне: удобнее сравнивать тег в тег.
        unique_camera_model: "CPH2447-OnePlus-OnePlus".to_owned(),
        software: "Fasticamera dng_from_capture".to_owned(),
        lens: Some(lens),
    };
    let moment = CaptureMoment {
        utc_millis,
        utc_offset_minutes: 0,
    };

    let mut writer = BufWriter::new(File::create(&output).unwrap());
    write_dng(
        &DngInputs {
            facts: &facts,
            raw: &raw,
            camera: &camera,
            captured_at: &moment,
        },
        &mut writer,
    )
    .unwrap();
    println!("записан {} ({}×{})", output.display(), width, height);
}

/// Разобранная командная строка примера.
struct CommandLine {
    run_dir: PathBuf,
    output: PathBuf,
    /// Чей паспорт читать (`stream_frame` / `still_frame`).
    section: String,
    /// Файл `raw16` вместо `*.stream.raw16` прогона.
    raw16_override: Option<PathBuf>,
    /// Профиль калибровки виньетирования (`device-profiles/<устройство>/lens-shading.json`).
    lens_shading_profile: Option<PathBuf>,
}

impl CommandLine {
    fn parse(mut args: impl Iterator<Item = String>) -> Self {
        let run_dir = PathBuf::from(args.next().expect("первый аргумент — каталог прогона"));
        let output = PathBuf::from(args.next().expect("второй аргумент — выходной .dng"));
        let mut parsed = Self {
            run_dir,
            output,
            section: "stream_frame".to_owned(),
            raw16_override: None,
            lens_shading_profile: None,
        };
        while let Some(flag) = args.next() {
            let value = args.next().unwrap_or_else(|| panic!("у флага {flag} нет значения"));
            match flag.as_str() {
                "--section" => parsed.section = value,
                "--raw16" => parsed.raw16_override = Some(PathBuf::from(value)),
                "--lens-shading" => parsed.lens_shading_profile = Some(PathBuf::from(value)),
                other => panic!("неизвестный флаг {other}"),
            }
        }
        parsed
    }
}

/// Сетка усилений камеры `camera_id` из профиля калибровки; размер активной области обязан совпасть с кадром.
fn read_lens_shading_grid(profile_path: &Path, camera_id: &str, size: PixelSize) -> LensShadingGainGrid {
    let profile: Value = serde_json::from_slice(&std::fs::read(profile_path).unwrap()).unwrap();
    let camera = profile["cameras"]
        .as_array()
        .unwrap()
        .iter()
        .find(|camera| camera["camera_id"].as_str() == Some(camera_id))
        .unwrap_or_else(|| panic!("в профиле нет камеры {camera_id}"));
    let array = &camera["active_array"];
    assert_eq!(
        (array["width"].as_u64().unwrap(), array["height"].as_u64().unwrap()),
        (u64::from(size.width), u64::from(size.height)),
        "калибровка снята для другой активной области: нужен пересчёт, пример его не делает"
    );
    let grid = &camera["grid"];
    let points = grid["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|point| {
            let gains: [f32; 4] = numbers(point)
                .into_iter()
                .map(|gain| gain as f32)
                .collect::<Vec<_>>()
                .try_into()
                .unwrap();
            gains
        })
        .collect();
    LensShadingGainGrid::new(
        grid["columns"].as_u64().unwrap() as u32,
        grid["rows"].as_u64().unwrap() as u32,
        points,
    )
    .unwrap()
}

fn single_file_with_suffix(dir: &Path, suffix: &str) -> PathBuf {
    let mut found = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.file_name().unwrap().to_string_lossy().ends_with(suffix))
        .collect::<Vec<_>>();
    assert_eq!(found.len(), 1, "в {} нужен ровно один файл *{suffix}", dir.display());
    found.remove(0)
}

fn parse_size(text: &str) -> (u32, u32) {
    let (width, height) = text.split_once('x').expect("размер вида 4096x3072");
    (width.parse().unwrap(), height.parse().unwrap())
}

fn first_f32(value: &Value) -> f32 {
    value
        .as_array()
        .and_then(|items| items.first())
        .and_then(Value::as_f64)
        .unwrap() as f32
}

fn numbers(value: &Value) -> Vec<f64> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item.as_f64().unwrap())
        .collect()
}

fn matrix(value: &Value) -> Matrix3x3 {
    let values: [f64; 9] = numbers(value).try_into().expect("в матрице 9 чисел");
    Matrix3x3(values.map(|number| {
        let denominator = MATRIX_DENOMINATORS
            .into_iter()
            .find(|candidate| {
                let fraction = (number * f64::from(*candidate)).round() / f64::from(*candidate);
                (fraction - number).abs() < MATRIX_EXACT_TOLERANCE
            })
            .unwrap_or(1_000_000);
        Rational {
            numerator: (number * f64::from(denominator)).round() as i32,
            denominator,
        }
    }))
}

fn cfa_layout(code: i64) -> CfaLayout {
    // `SENSOR_INFO_COLOR_FILTER_ARRANGEMENT`: 0 RGGB, 1 GRBG, 2 GBRG, 3 BGGR.
    match code {
        0 => CfaLayout::Rggb,
        1 => CfaLayout::Grbg,
        2 => CfaLayout::Gbrg,
        3 => CfaLayout::Bggr,
        other => panic!("раскладка CFA {other} не Bayer"),
    }
}

fn facts_from_passport(frame: &Value, camera_id: &str, size: PixelSize) -> StillFrameFacts {
    let sensor = &frame["sensor"];
    let color = &frame["color"];
    let dynamic = &frame["frame"];
    let area = PixelRect { left: 0, top: 0, size };
    let black_level_pattern: [u32; 4] = numbers(&sensor["black_level_pattern"])
        .into_iter()
        .map(|level| level as u32)
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    let calibration = ["1", "2"]
        .iter()
        .map(|suffix| ColorCalibrationSet {
            illuminant: ReferenceIlluminant::from_exif_code(
                color[format!("reference_illuminant{suffix}").as_str()]
                    .as_u64()
                    .unwrap() as u8,
            ),
            color_matrix: matrix(&color[format!("color_matrix{suffix}").as_str()]),
            forward_matrix: matrix(&color[format!("forward_matrix{suffix}").as_str()]),
            calibration_transform: matrix(&color[format!("calibration{suffix}").as_str()]),
        })
        .collect();
    let stream = StreamPassport {
        camera: CameraId::new(camera_id).unwrap(),
        raw_format: RawFormat::Raw16,
        buffer_size: size,
        readout: ReadoutAspect::FourByThree,
        valid_area: area,
        sensor_area: camera_contract::lens_shading::ValidAreaOnSensor::whole_active_array(size),
        lens_shading: camera_contract::lens_shading::LensShadingCalibrationState::NotCalibrated(
            camera_contract::lens_shading::NotCalibratedReason::NoCalibrationForCamera,
        ),
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: cfa_layout(sensor["cfa"].as_i64().unwrap()),
        },
        static_levels: SensorLevels {
            black_level_pattern,
            white_level: sensor["white_level"].as_u64().unwrap() as u32,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        sensor_orientation: SensorOrientation::Degrees90,
        frame_rate: TargetFrameRate::new(30).unwrap(),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: calibration,
    };

    let neutral: [f64; 3] = numbers(&dynamic["neutral_color_point"])
        .try_into()
        .expect("3 числа нейтрали");
    let noise_profile = dynamic["noise_profile"].as_array().map(|pairs| {
        let coefficients: Vec<NoiseModelCoefficients> = pairs
            .iter()
            .map(|pair| {
                let pair = numbers(pair);
                NoiseModelCoefficients {
                    scale: pair[0],
                    offset: pair[1],
                }
            })
            .collect();
        NoiseProfile(coefficients.try_into().expect("4 пары шума"))
    });
    let dynamic_black_level = dynamic["dynamic_black_level"].as_array().map(|_| {
        let levels: [f32; 4] = numbers(&dynamic["dynamic_black_level"])
            .into_iter()
            .map(|level| level as f32)
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        levels
    });
    let timestamp = SensorTimestamp::from_nanos(dynamic["timestamp_ns"].as_i64().unwrap());
    StillFrameFacts {
        id: StillId(1),
        outputs: StillOutputs {
            primary: PrimaryFormat::Jpeg,
            raw: RawOutput::Dng,
        },
        stream: Arc::new(stream),
        sensor_timestamp: timestamp,
        capture: FrameCaptureDetails {
            result: CaptureResultMeta {
                frame_number: FrameNumber(0),
                sensor_timestamp: Some(timestamp),
                exposure_time: dynamic["exposure_ns"].as_u64().map(Duration::from_nanos),
                sensitivity_iso: dynamic["iso"].as_u64().map(|iso| iso as u32),
                reported_frame_duration: None,
                rolling_shutter_skew: None,
                three_a: ThreeAResultState::unreported(RequestOrigin::Repeating),
            },
            color: CaptureColorHints {
                dynamic_black_level,
                dynamic_white_level: dynamic["dynamic_white_level"].as_u64().map(|level| level as u32),
                noise_profile,
                ..CaptureColorHints::default()
            },
        },
        choice: FrameChoice::ShownAtPress {
            sensor_timestamp: timestamp,
            shown_at: MonotonicNanos::from_nanos(0),
        },
        crop: area,
        rotation: StillRotation::None,
        neutral: AsShotNeutral(neutral),
        digital_gain: DigitalGainEv(0.0),
        lens_shading: AppliedLensShading::NotApplied(LensShadingNotAppliedReason::NotCalibrated),
    }
}
