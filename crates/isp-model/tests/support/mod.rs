//! Общие помощники интеграционных тестов `isp-model`: чтение фикстур из `test-assets/isp-model/`.
//!
//! Формат фикстуры описан в `test-assets/isp-model/README.md`: манифест `<имя>.json` +
//! отсчёты `<имя>.bin` (little-endian, порядок строк, каналы подряд).

#![allow(dead_code, reason = "каждый тестовый бинарь использует свою часть помощников")]
#![allow(
    clippy::panic,
    reason = "помощник тестов: битая фикстура должна ронять тест с понятным текстом"
)]

pub mod color_chart;
pub mod knob_strength;
pub mod real_capture;

use std::path::PathBuf;

/// Отсчёты фикстуры в исходном типе.
#[derive(Debug, Clone, PartialEq)]
pub enum AssetSamples {
    U16(Vec<u16>),
    F32(Vec<f32>),
}

/// Фикстура: размер, число каналов, отсчёты и весь манифест (для полей конкретного теста).
#[derive(Debug, Clone)]
pub struct TestAsset {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub channels: u32,
    pub samples: AssetSamples,
    pub manifest: serde_json::Value,
}

impl TestAsset {
    pub fn u16_samples(&self) -> &[u16] {
        match &self.samples {
            AssetSamples::U16(samples) => samples,
            AssetSamples::F32(_) => panic!("фикстура {} хранит f32, а тест ждёт u16", self.name),
        }
    }

    pub fn f32_samples(&self) -> &[f32] {
        match &self.samples {
            AssetSamples::F32(samples) => samples,
            AssetSamples::U16(_) => panic!("фикстура {} хранит u16, а тест ждёт f32", self.name),
        }
    }

    /// Трёхканальные отсчёты как пиксели `[R, G, B]`.
    pub fn rgb_pixels(&self) -> Vec<[f32; 3]> {
        assert_eq!(self.channels, 3, "фикстура {} не трёхканальная", self.name);
        self.f32_samples()
            .chunks_exact(3)
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect()
    }

    /// Поле манифеста по пути через точку (`"extra.black_level"`).
    pub fn field(&self, dotted_path: &str) -> &serde_json::Value {
        let mut value = &self.manifest;
        for key in dotted_path.split('.') {
            value = value
                .get(key)
                .unwrap_or_else(|| panic!("в манифесте {} нет поля {dotted_path}", self.name));
        }
        value
    }
}

/// Каталог фикстур: `<корень репозитория>/test-assets/isp-model`.
pub fn assets_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test-assets/isp-model")
}

/// Читает фикстуру по имени (без расширения). Ошибка чтения — паника с понятным текстом:
/// пропавшая фикстура должна валить тест, а не пропускать проверку.
pub fn load_asset(name: &str) -> TestAsset {
    load_asset_from(&assets_dir(), name)
}

/// То же из произвольного каталога (полный кадр для утилиты рендера лежит вне репозитория).
pub fn load_asset_from(dir: &std::path::Path, name: &str) -> TestAsset {
    let manifest_path = dir.join(format!("{name}.json"));
    let manifest_text = std::fs::read_to_string(&manifest_path)
        .unwrap_or_else(|error| panic!("нет манифеста {}: {error}", manifest_path.display()));
    let manifest: serde_json::Value = serde_json::from_str(&manifest_text)
        .unwrap_or_else(|error| panic!("битый манифест {}: {error}", manifest_path.display()));
    let width = manifest_u32(&manifest, "width", name);
    let height = manifest_u32(&manifest, "height", name);
    let channels = manifest_u32(&manifest, "channels", name);
    let data_file = manifest["data_file"]
        .as_str()
        .unwrap_or_else(|| panic!("в {name} нет data_file"));
    let bytes = std::fs::read(dir.join(data_file)).unwrap_or_else(|error| panic!("нет данных {data_file}: {error}"));
    let expected_samples = width as usize * height as usize * channels as usize;
    let samples = match manifest["sample_type"].as_str() {
        Some("u16le") => {
            assert_eq!(
                bytes.len(),
                expected_samples * 2,
                "размер {data_file} не совпал с манифестом"
            );
            AssetSamples::U16(
                bytes
                    .chunks_exact(2)
                    .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
                    .collect(),
            )
        }
        Some("f32le") => {
            assert_eq!(
                bytes.len(),
                expected_samples * 4,
                "размер {data_file} не совпал с манифестом"
            );
            AssetSamples::F32(
                bytes
                    .chunks_exact(4)
                    .map(|quad| f32::from_le_bytes([quad[0], quad[1], quad[2], quad[3]]))
                    .collect(),
            )
        }
        other => panic!("неизвестный sample_type {other:?} в {name}"),
    };
    TestAsset {
        name: name.to_owned(),
        width,
        height,
        channels,
        samples,
        manifest,
    }
}

fn manifest_u32(manifest: &serde_json::Value, key: &str, name: &str) -> u32 {
    let value = manifest[key]
        .as_u64()
        .unwrap_or_else(|| panic!("в {name} нет числового поля {key}"));
    u32::try_from(value).unwrap_or_else(|_| panic!("поле {key} в {name} не помещается в u32"))
}
