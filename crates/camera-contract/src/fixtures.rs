//! Образцы камер OnePlus 11 (CPH2447) по фактам сессии 00 — для тестов `camera-core`,
//! `camera-android` и приложения. Числа взяты из `user/foundation/results/00.md`
//! и `mem:device/oneplus11-camera-facts`; где 00 не мерила (оптика), значения правдоподобные
//! и помечены. Продуктовый код эти образцы не видит (фича `fixtures`).

use std::time::Duration;

use crate::capabilities::{
    CameraCapabilities, CameraDescriptor, ColorCalibrationSet, ExposureTimeRange, FocusCalibration, FocusCapability,
    FpsRange, FrameDuration, HardwareLevel, IsoRange, LensShading, ManualSensorRanges, Matrix3x3, Optics,
    PhysicalSizeMm, Rational, RawSensorInfo, RawStreamOption, RawSupport, ReferenceIlluminant, RequestCapability,
    SensorLevels, VendorProcessingFlags,
};
use crate::geometry::{PixelRect, PixelSize, SensorOrientation};
use crate::ids::{CameraId, CameraTopology, LensFacing};
use crate::raw_format::{CfaLayout, MosaicKind, MosaicPattern, NativeMosaic, RawFormat};
use crate::three_a::{
    AfModeSupport, AntibandingModeSupport, ExposureCompensationSupport, LockSupport, MaxRegions, ThreeACapabilities,
};

/// Публичный потолок OnePlus 11: 33,3 мс на кадр (30 к/с) для всех RAW (00 C1).
const PUBLIC_MIN_FRAME_DURATION: FrameDuration = FrameDuration::from_nanos(33_333_333);

/// ID камеры из строки-литерала теста.
#[expect(
    clippy::expect_used,
    reason = "только тестовые образцы: пустой литерал — ошибка самого теста"
)]
pub fn camera_id(id: &str) -> CameraId {
    CameraId::new(id).expect("ID образца камеры непустой")
}

fn identity_matrix() -> Matrix3x3 {
    let one = Rational {
        numerator: 1,
        denominator: 1,
    };
    let zero = Rational {
        numerator: 0,
        denominator: 1,
    };
    Matrix3x3([one, zero, zero, zero, one, zero, zero, zero, one])
}

fn raw_info() -> RawSensorInfo {
    RawSensorInfo {
        levels: SensorLevels {
            black_level_pattern: [64; 4],
            white_level: 1023,
        },
        cfa: CfaLayout::Bggr,
        native_mosaic: NativeMosaic::Known(MosaicKind::QuadBayer),
        color_calibration: vec![
            ColorCalibrationSet {
                illuminant: ReferenceIlluminant::D65,
                color_matrix: identity_matrix(),
                forward_matrix: identity_matrix(),
                calibration_transform: identity_matrix(),
            },
            ColorCalibrationSet {
                illuminant: ReferenceIlluminant::StandardA,
                color_matrix: identity_matrix(),
                forward_matrix: identity_matrix(),
                calibration_transform: identity_matrix(),
            },
        ],
    }
}

fn raw_streams(size: PixelSize) -> Vec<RawStreamOption> {
    let mosaic = MosaicPattern {
        kind: MosaicKind::Bayer,
        layout: CfaLayout::Bggr,
    };
    [RawFormat::Raw10, RawFormat::Raw16]
        .into_iter()
        .map(|format| RawStreamOption {
            format,
            size,
            mosaic,
            min_frame_duration: PUBLIC_MIN_FRAME_DURATION,
        })
        .collect()
}

fn manual_sensor() -> ManualSensorRanges {
    ManualSensorRanges::Supported {
        iso: IsoRange { min: 100, max: 6400 },
        exposure_time: ExposureTimeRange {
            shortest: Duration::from_micros(10),
            longest: Duration::from_secs(32),
        },
        max_frame_duration: FrameDuration::from_nanos(32_000_000_000),
    }
}

fn capabilities(
    facing: LensFacing,
    topology: CameraTopology,
    raw_size: PixelSize,
    focus: FocusCapability,
    optics: Optics,
) -> CameraCapabilities {
    CameraCapabilities {
        facing,
        topology,
        hardware_level: HardwareLevel::Level3,
        request_capabilities: vec![
            RequestCapability::BackwardCompatible,
            RequestCapability::ManualSensor,
            RequestCapability::Raw,
            RequestCapability::ReadSensorSettings,
        ],
        pixel_array: raw_size,
        active_array: PixelRect {
            left: 0,
            top: 0,
            size: raw_size,
        },
        // Типичная ориентация для Android: задние сенсоры 90°, фронт 270°. Сверяется со
        // smoke `camera-catalog` (dumpsys `android.sensor.orientation`).
        sensor_orientation: match facing {
            LensFacing::Front => SensorOrientation::Degrees270,
            LensFacing::Back | LensFacing::External => SensorOrientation::Degrees90,
        },
        raw_streams: raw_streams(raw_size),
        ae_fps_ranges: vec![FpsRange { min: 15, max: 30 }, FpsRange { min: 30, max: 30 }],
        manual_sensor: manual_sensor(),
        raw: RawSupport::Supported(raw_info()),
        three_a: three_a_for(facing),
        focus,
        optics,
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
    }
}

/// Автоматика 3A по `dumpsys media.camera` (сессия 08): задние камеры — компенсация
/// −18…18 по 1/6 EV, по одному региону AE и AF (AWB без регионов), режимы AF 0–4,
/// блокировка AE, антибандинг 0–3; фронт — только `AF_MODE_OFF` (фиксированный фокус).
fn three_a_for(facing: LensFacing) -> ThreeACapabilities {
    let af_modes = match facing {
        LensFacing::Front => vec![AfModeSupport::Off],
        LensFacing::Back | LensFacing::External => vec![
            AfModeSupport::Off,
            AfModeSupport::Auto,
            AfModeSupport::Macro,
            AfModeSupport::ContinuousVideo,
            AfModeSupport::ContinuousPicture,
        ],
    };
    ThreeACapabilities {
        exposure_compensation: ExposureCompensationSupport::Supported {
            min_steps: -18,
            max_steps: 18,
            step_ev: Rational {
                numerator: 1,
                denominator: 6,
            },
        },
        max_regions: MaxRegions {
            auto_exposure: 1,
            auto_white_balance: 0,
            auto_focus: u32::from(facing != LensFacing::Front),
        },
        af_modes,
        ae_lock: LockSupport::Available,
        antibanding_modes: vec![
            AntibandingModeSupport::Off,
            AntibandingModeSupport::Hz50,
            AntibandingModeSupport::Hz60,
            AntibandingModeSupport::Auto,
        ],
    }
}

fn adjustable_focus() -> FocusCapability {
    FocusCapability::Adjustable {
        min_focus_distance_diopters: 10.0,
        hyperfocal_distance_diopters: Some(0.3),
        calibration: FocusCalibration::Approximate,
    }
}

/// Основная камера 2 (IMX890): RAW 4096×3072; оптика правдоподобная (угол ~ 82°).
pub fn main_wide_camera() -> CameraDescriptor {
    CameraDescriptor {
        id: camera_id("2"),
        capabilities: capabilities(
            LensFacing::Back,
            CameraTopology::Physical,
            PixelSize::new(4096, 3072),
            adjustable_focus(),
            Optics {
                apertures_f_number: vec![1.8],
                focal_lengths_mm: vec![5.6],
                sensor_physical_size: PhysicalSizeMm {
                    width_mm: 9.8,
                    height_mm: 7.4,
                },
            },
        ),
    }
}

/// Ширик 3 (IMX581): RAW 4000×3000; оптика правдоподобная (угол ~ 115°).
pub fn ultra_wide_camera() -> CameraDescriptor {
    CameraDescriptor {
        id: camera_id("3"),
        capabilities: capabilities(
            LensFacing::Back,
            CameraTopology::Physical,
            PixelSize::new(4000, 3000),
            adjustable_focus(),
            Optics {
                apertures_f_number: vec![2.2],
                focal_lengths_mm: vec![2.6],
                sensor_physical_size: PhysicalSizeMm {
                    width_mm: 8.0,
                    height_mm: 6.0,
                },
            },
        ),
    }
}

/// Фронт 1 (S5K3P9): RAW 2304×1728, фокус фиксированный.
pub fn front_camera() -> CameraDescriptor {
    CameraDescriptor {
        id: camera_id("1"),
        capabilities: capabilities(
            LensFacing::Front,
            CameraTopology::Physical,
            PixelSize::new(2304, 1728),
            FocusCapability::Fixed,
            Optics {
                apertures_f_number: vec![2.4],
                focal_lengths_mm: vec![3.3],
                sensor_physical_size: PhysicalSizeMm {
                    width_mm: 4.6,
                    height_mm: 3.5,
                },
            },
        ),
    }
}

/// Логическая камера 0 = основная 2 + ширик 3.
pub fn logical_back_camera() -> CameraDescriptor {
    let mut descriptor = main_wide_camera();
    descriptor.id = camera_id("0");
    descriptor.capabilities.topology = CameraTopology::Logical {
        physical_ids: vec![camera_id("2"), camera_id("3")],
    };
    descriptor
        .capabilities
        .request_capabilities
        .push(RequestCapability::LogicalMultiCamera);
    descriptor
}

/// Все камеры, видимые приложению на OnePlus 11 (0–3; 4 и 5 закрыты `SYSTEM_CAMERA`).
pub fn oneplus11_visible_cameras() -> Vec<CameraDescriptor> {
    vec![
        logical_back_camera(),
        front_camera(),
        main_wide_camera(),
        ultra_wide_camera(),
    ]
}
