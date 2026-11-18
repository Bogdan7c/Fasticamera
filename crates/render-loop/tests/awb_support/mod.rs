//! Общее для тестов AWB на GPU: сцена из фикстуры, мозаика RAW, калибровка цвета, поддельный производитель
//! кадров и импортёр, который отдаёт мозаику по метке сенсора.

#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]
#![allow(dead_code, reason = "каждый тестовый файл использует свою часть помощников")]

use std::collections::HashMap;
use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex, PoisonError};

use camera_contract::capabilities::{
    ColorCalibrationSet, LensShading, Matrix3x3, Rational, ReferenceIlluminant, SensorLevels, VendorProcessingFlags,
};
use camera_contract::fixtures::camera_id;
use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::{PixelRect, PixelSize, SensorOrientation};
use camera_contract::raw_format::{CfaLayout, MosaicKind, MosaicPattern, RawFormat};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};
use gpu_vulkan::context::GpuContext;
use gpu_vulkan::hardware_buffer_cache::ImportCacheCounters;
use gpu_vulkan::raw_mosaic_preview::MosaicSource;
use gpu_vulkan::raw_mosaic_preview::synthetic::SyntheticRaw16;
use raw_frame_stream::{ProducerFrame, RawFrameLease, ReleaseFences};
use render_loop::vulkan_backend::{CameraFrameImporter, CameraImportError, SubmissionSerial};
use shell_contract::hardware_buffer::BorrowedHardwareBuffer;

/// Мозаика 64×48 = 32×24 ячеек 2×2 = ровно одна ячейка на плитку статистики 32×24.
pub const FRAME_SIZE: PixelSize = PixelSize::new(64, 48);
pub const BLACK_LEVEL: u32 = 1024;
pub const WHITE_LEVEL: u32 = 16383;
pub const TILE_COLUMNS: usize = 32;
pub const QUANT_MAX: f64 = 4095.0;

// ---- фикстура сцены ----

pub fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-assets/scene-estimation")
}

/// Камерные значения (доли белого, ДО баланса) каждой плитки сцены фикстуры: `[r, g, b]` по 768 плиткам.
/// Пересвеченная плитка фикстуры (маркер 0xFFFF) становится полностью белой (ячейка вне порога пересвета).
pub fn fixture_tiles(scene: &str) -> Vec<[f64; 3]> {
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(assets_dir().join("awb-scenes.json")).expect("манифест"))
            .expect("манифест — JSON");
    let index = manifest["extra"]["scenes"]
        .as_array()
        .expect("сцены")
        .iter()
        .position(|candidate| candidate["name"] == scene)
        .unwrap_or_else(|| panic!("в фикстуре нет сцены {scene}"));
    let bytes = std::fs::read(assets_dir().join("awb-scenes.bin")).expect("отсчёты");
    let samples: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let level = |value: u16| {
        if value == 0xFFFF {
            1.0
        } else {
            f64::from(value) / QUANT_MAX
        }
    };
    samples[index * 768 * 3..(index + 1) * 768 * 3]
        .chunks_exact(3)
        .map(|rgb| [level(rgb[0]), level(rgb[1]), level(rgb[2])])
        .collect()
}

/// Отсчёты RAW16 BGGR: ячейка `(cx, cy)` — плитка `cy · 32 + cx` (позиции 2×2: B, G / G, R).
pub fn mosaic_from_tiles(tiles: &[[f64; 3]]) -> Vec<u16> {
    let span = f64::from(WHITE_LEVEL - BLACK_LEVEL);
    let raw = |level: f64| u16::try_from(BLACK_LEVEL + (level * span).round() as u32).expect("RAW16");
    let (width, height) = (FRAME_SIZE.width as usize, FRAME_SIZE.height as usize);
    (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            let [red, green, blue] = tiles[(y / 2) * TILE_COLUMNS + x / 2];
            match (x % 2, y % 2) {
                (0, 0) => raw(blue),
                (1, 1) => raw(red),
                _ => raw(green),
            }
        })
        .collect()
}

/// Рациональная матрица 3×3: знаменатель 2^20 хранит десятичные значения HAL с ошибкой < 1e-6.
pub fn matrix(rows: &serde_json::Value) -> Matrix3x3 {
    const DENOMINATOR: i32 = 1 << 20;
    let numbers: Vec<f64> = rows
        .as_array()
        .expect("матрица — массив строк")
        .iter()
        .flat_map(|row| {
            row.as_array()
                .expect("строка")
                .iter()
                .map(|value| value.as_f64().expect("число"))
        })
        .collect();
    Matrix3x3(std::array::from_fn(|index| Rational {
        numerator: (numbers[index] * f64::from(DENOMINATOR)).round() as i32,
        denominator: DENOMINATOR,
    }))
}

pub fn diagonal(values: &serde_json::Value) -> Matrix3x3 {
    let d: Vec<f64> = values
        .as_array()
        .expect("диагональ")
        .iter()
        .map(|v| v.as_f64().expect("число"))
        .collect();
    matrix(&serde_json::json!([
        [d[0], 0.0, 0.0],
        [0.0, d[1], 0.0],
        [0.0, 0.0, d[2]]
    ]))
}

/// Калибровка цвета OnePlus 11 — те же числа, по которым генератор фикстуры считал отклик камеры (манифест).
/// Образец `main_wide_camera()` несёт единичные матрицы и для цвета не годится.
pub fn calibration() -> Vec<ColorCalibrationSet> {
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(assets_dir().join("awb-scenes.json")).expect("манифест"))
            .expect("манифест — JSON");
    let calibration = &manifest["extra"]["calibration"];
    let forward = matrix(&serde_json::json!([
        [0.4375, 0.3828125, 0.140625],
        [0.21875, 0.71875, 0.0625],
        [0.015625, 0.09375, 0.7109375]
    ]));
    vec![
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::D65,
            color_matrix: matrix(&calibration["color_matrix_d65"]),
            forward_matrix: forward,
            calibration_transform: diagonal(&calibration["camera_calibration_d65_diagonal"]),
        },
        ColorCalibrationSet {
            illuminant: ReferenceIlluminant::StandardA,
            color_matrix: matrix(&calibration["color_matrix_standard_a"]),
            forward_matrix: forward,
            calibration_transform: diagonal(&calibration["camera_calibration_standard_a_diagonal"]),
        },
    ]
}

pub fn stream_passport() -> StreamPassport {
    StreamPassport {
        camera: camera_id("2"),
        raw_format: RawFormat::Raw16,
        buffer_size: FRAME_SIZE,
        readout: ReadoutAspect::FourByThree,
        valid_area: PixelRect {
            left: 0,
            top: 0,
            size: FRAME_SIZE,
        },
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: CfaLayout::Bggr,
        },
        static_levels: SensorLevels {
            black_level_pattern: [BLACK_LEVEL; 4],
            white_level: WHITE_LEVEL,
        },
        vendor_processing: VendorProcessingFlags {
            lens_shading: LensShading::AppliedByVendor,
        },
        // Без поворота: пиксель изображения (x, y) — плитка (x, y), серую плитку легко найти.
        sensor_orientation: SensorOrientation::Degrees0,
        frame_rate: TargetFrameRate::new(60).expect("60 к/с"),
        frame_rate_mechanism: FrameRateMechanism::HalDefault,
        color_calibration: calibration(),
    }
}

// ---- поддельный производитель и импортёр ----

#[derive(Default)]
pub struct Returned(pub Mutex<Vec<i64>>);

pub struct FakeFrame {
    pub sensor_ts: i64,
    pub storage: Box<u8>,
    pub returned: Arc<Returned>,
}

// SAFETY: указатель из `hardware_buffer` указывает на `storage`, живой до `return_to_producer`.
unsafe impl ProducerFrame for FakeFrame {
    fn hardware_buffer(&self) -> BorrowedHardwareBuffer<'_> {
        let pointer = NonNull::from(&*self.storage).cast::<c_void>();
        // SAFETY: `storage` живёт, пока жив `self`; заимствование ограничено `&self`.
        unsafe { BorrowedHardwareBuffer::from_raw(pointer) }
    }

    fn return_to_producer(self: Box<Self>, _release: ReleaseFences) {
        self.returned
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(self.sensor_ts);
    }
}

/// Импортёр: кадр с меткой сенсора `ts` — мозаика `scenes[ts]`.
pub struct SceneImporter {
    pub context: GpuContext,
    pub scenes: HashMap<i64, Arc<Vec<u16>>>,
    pub entries: HashMap<i64, (SyntheticRaw16, u64)>,
    pub counters: ImportCacheCounters,
}

impl CameraFrameImporter for SceneImporter {
    fn import_frame(
        &mut self,
        lease: &RawFrameLease,
        submission: SubmissionSerial,
    ) -> Result<MosaicSource, CameraImportError> {
        let sensor_ts = lease.passport().sensor_timestamp.as_nanos();
        if let Some((source, last_use)) = self.entries.get_mut(&sensor_ts) {
            *last_use = submission.0;
            self.counters.hits += 1;
            return Ok(source.source());
        }
        self.counters.misses += 1;
        let texels = Arc::clone(
            self.scenes
                .get(&sensor_ts)
                .ok_or_else(|| CameraImportError::Platform(format!("неизвестный кадр {sensor_ts}")))?,
        );
        let source = SyntheticRaw16::new(&self.context, FRAME_SIZE.width, FRAME_SIZE.height, &texels)?;
        let mosaic = source.source();
        self.entries.insert(sensor_ts, (source, submission.0));
        Ok(mosaic)
    }

    fn retire_completed(&mut self, completed: SubmissionSerial) {
        self.entries.retain(|_, (_, last_use)| *last_use > completed.0);
    }

    fn release_all_imports_gpu_idle(&mut self) {
        self.entries.clear();
    }

    fn imports_alive(&self) -> usize {
        self.entries.len()
    }

    fn cache_counters(&self) -> ImportCacheCounters {
        self.counters
    }
}
