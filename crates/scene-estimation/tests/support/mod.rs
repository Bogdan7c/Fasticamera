//! Общие помощники интеграционных тестов: загрузка фикстуры сцен и профиль цвета OnePlus 11.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]
#![allow(dead_code, reason = "каждый тестовый файл использует свою часть помощников")]

use std::path::PathBuf;

use camera_contract::capabilities::{ColorCalibrationSet, Matrix3x3, Rational, ReferenceIlluminant};
use isp_model::color::dng::DngColorProfile;
use isp_model::statistics::{AwbStatistics, AwbTile, TileGrid};
use serde_json::Value;

/// Сцена фикстуры.
pub struct Scene {
    pub name: String,
    pub kind: String,
    pub light: String,
    /// Ожидаемый свет: CCT, К.
    pub expected_cct: f64,
    /// Ожидаемый свет: Duv.
    pub expected_duv: f64,
    pub statistics: AwbStatistics,
}

fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-assets/scene-estimation")
}

/// Рациональная матрица 3×3: знаменатель 2^20 хранит десятичные значения HAL с ошибкой < 1e-6.
fn matrix(values: &Value) -> Matrix3x3 {
    const DENOMINATOR: i32 = 1 << 20;
    let numbers: Vec<f64> = values
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

fn diagonal(values: &Value) -> Matrix3x3 {
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

/// Профиль цвета из калибровки в манифесте (те же числа, что у генератора).
pub fn camera_profile(manifest: &Value) -> DngColorProfile {
    let calibration = &manifest["extra"]["calibration"];
    let forward = matrix(&serde_json::json!([
        [0.4375, 0.3828125, 0.140625],
        [0.21875, 0.71875, 0.0625],
        [0.015625, 0.09375, 0.7109375]
    ]));
    let sets = vec![
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
    ];
    DngColorProfile::from_calibration_sets(&sets).expect("профиль цвета")
}

/// Манифест и сцены фикстуры `awb-scenes`.
pub fn load_scenes() -> (Value, Vec<Scene>) {
    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(assets_dir().join("awb-scenes.json")).expect("манифест"))
            .expect("манифест — JSON");
    let bytes = std::fs::read(assets_dir().join("awb-scenes.bin")).expect("отсчёты");
    let samples: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    let extra = &manifest["extra"];
    let grid = TileGrid {
        columns: u32::try_from(extra["grid"]["columns"].as_u64().expect("столбцы")).expect("u32"),
        rows: u32::try_from(extra["grid"]["rows"].as_u64().expect("строки")).expect("u32"),
    };
    let cells = u32::try_from(extra["cells_per_tile"].as_u64().expect("ячеек")).expect("u32");
    let tiles_per_scene = grid.tile_count() as usize;
    let scenes = extra["scenes"]
        .as_array()
        .expect("сцены")
        .iter()
        .enumerate()
        .map(|(index, scene)| {
            let base = index * tiles_per_scene * 3;
            let tiles = (0..tiles_per_scene)
                .map(|tile| {
                    let q = &samples[base + tile * 3..base + tile * 3 + 3];
                    if q.iter().all(|&value| value == 0xFFFF) {
                        return AwbTile {
                            sum_r: 0,
                            sum_g: 0,
                            sum_b: 0,
                            used_cells: 0,
                            saturated_cells: cells,
                        };
                    }
                    let [q_r, q_g, q_b] = [q[0], q[1], q[2]].map(u32::from);
                    AwbTile {
                        sum_r: q_r * cells,
                        sum_g: 2 * q_g * cells,
                        sum_b: q_b * cells,
                        used_cells: cells,
                        saturated_cells: 0,
                    }
                })
                .collect();
            Scene {
                name: scene["name"].as_str().expect("имя").to_owned(),
                kind: scene["kind"].as_str().expect("вид").to_owned(),
                light: scene["light"].as_str().expect("свет").to_owned(),
                expected_cct: scene["expected"]["cct"].as_f64().expect("cct"),
                expected_duv: scene["expected"]["duv"].as_f64().expect("duv"),
                statistics: AwbStatistics::from_tiles(grid, tiles).expect("плитки по сетке"),
            }
        })
        .collect();
    (manifest, scenes)
}
