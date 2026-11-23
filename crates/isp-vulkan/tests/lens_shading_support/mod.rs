//! Общие помощники тестов коррекции виньетирования на GPU: несимметричная карта усилений и подмена
//! решения стадии в готовых `IspParams`. Не зависит от конкретного конвейера (видоискатель, V4, снимок).

#![allow(dead_code, reason = "каждый тестовый бинарь использует свою часть помощников")]
#![allow(
    clippy::unwrap_used,
    reason = "помощник тестов: падение и есть способ сообщить о провале"
)]

use std::sync::Arc;

use isp_model::lens_shading::{LensShadingGainMap, LensShadingSkipReason, LensShadingStep};
use isp_model::params::IspParams;

/// Карта `columns × rows` с НЕСИММЕТРИЧНЫМИ усилениями: у четырёх каналов разный размах и разная форма по
/// осям (квадратичная по X, линейная по Y, перекрёстный член), поэтому перепутанные каналы, оси, строки
/// или неверные веса билинейной смеси дают заметно другую картинку. Все усиления ≥ 1 (как у настоящей
/// калибровки), максимум — около 2,35 в «синем» канале правого нижнего угла.
pub fn asymmetric_map(columns: u32, rows: u32) -> Arc<LensShadingGainMap> {
    let mut points = Vec::new();
    for row in 0..rows {
        for column in 0..columns {
            let horizontal = column as f32 / (columns - 1) as f32;
            let vertical = row as f32 / (rows - 1) as f32;
            let mut gains = [1.0_f32; 4];
            for (channel, gain) in gains.iter_mut().enumerate() {
                let strength = 0.9 * (channel as f32 + 1.0) / 4.0;
                *gain = 1.0 + strength * (0.7 * horizontal * horizontal + 0.3 * vertical + 0.5 * horizontal * vertical);
            }
            points.push(gains);
        }
    }
    Arc::new(LensShadingGainMap::new(columns, rows, points).unwrap())
}

/// Карта из единичных усилений: коррекция включена, но ничего не меняет (умножение на 1,0 точно).
pub fn identity_map(columns: u32, rows: u32) -> Arc<LensShadingGainMap> {
    Arc::new(LensShadingGainMap::new(columns, rows, vec![[1.0; 4]; (columns * rows) as usize]).unwrap())
}

/// Те же параметры с картой `map` в стадии виньетирования.
pub fn with_map(params: &IspParams, map: &Arc<LensShadingGainMap>) -> IspParams {
    IspParams {
        lens_shading: LensShadingStep::Apply(Arc::clone(map)),
        ..params.clone()
    }
}

/// Те же параметры с явным пропуском стадии виньетирования.
pub fn skipped(params: &IspParams) -> IspParams {
    IspParams {
        lens_shading: LensShadingStep::Skip(LensShadingSkipReason::NotCalibrated),
        ..params.clone()
    }
}

/// Размер активной области основной камеры «2» OnePlus 11 (калибровка снята над ней).
pub const ACTIVE_ARRAY: camera_contract::geometry::PixelSize = camera_contract::geometry::PixelSize::new(4096, 3072);

/// Настоящая калибровка камеры «2» из `device-profiles/oneplus-cph2447/lens-shading.json` (минимальный
/// разбор без проверок приложения: битый или пропавший профиль роняет тест с понятным текстом).
pub fn real_calibration_of_camera_2() -> camera_contract::lens_shading::LensShadingCalibration {
    use camera_contract::fixtures::camera_id;
    use camera_contract::geometry::PixelSize;
    use camera_contract::lens_shading::{LensShadingCalibration, LensShadingGainGrid};

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../device-profiles/oneplus-cph2447/lens-shading.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("нет профиля {}: {error}", path.display()));
    let profile: serde_json::Value = serde_json::from_str(&text).expect("профиль — JSON");
    let camera = profile["cameras"]
        .as_array()
        .expect("cameras")
        .iter()
        .find(|camera| camera["camera_id"] == "2")
        .expect("в профиле есть камера 2");
    let grid = &camera["grid"];
    let points: Vec<[f32; 4]> = grid["points"]
        .as_array()
        .expect("points")
        .iter()
        .map(|point| {
            let values: Vec<f32> = point
                .as_array()
                .expect("точка")
                .iter()
                .map(|value| value.as_f64().expect("число") as f32)
                .collect();
            [values[0], values[1], values[2], values[3]]
        })
        .collect();
    LensShadingCalibration {
        camera: camera_id("2"),
        active_array: PixelSize::new(
            camera["active_array"]["width"].as_u64().expect("width") as u32,
            camera["active_array"]["height"].as_u64().expect("height") as u32,
        ),
        grid: LensShadingGainGrid::new(
            grid["columns"].as_u64().expect("columns") as u32,
            grid["rows"].as_u64().expect("rows") as u32,
            points,
        )
        .expect("сетка профиля валидна"),
    }
}
