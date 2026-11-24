//! Синтетическая «сцена» RAW10 для стенда насыщения: детерминированный кадр без оглядки на камеру.
//!
//! Зачем не константа: ветвления шейдеров (пересветы, края, тени) должны идти как в жизни, иначе цифра
//! цены оптимистична. Одно зерно — один и тот же кадр (для сверки «через стенд» и «обычным путём»).
//! Формат — как буфер камеры: RAW10, четыре пикселя в пяти байтах, шаг строки в байтах
//! (для 4096 пикселей — 5120 байт, без паддинга).

use camera_contract::geometry::PixelSize;
use gpu_vulkan::raw_mosaic_preview::reference::pack_raw10;

use super::error::SaturationWorkError;
use super::scene_probe::{probe_cell_light, probe_channel_of_sample};

/// Чёрный уровень RAW10 основной камеры (код АЦП): паспорт потока OnePlus 11, `real-main-lamp-*` в `isp-model`.
pub const SENSOR_BLACK_CODE: u16 = 64;
/// Белый уровень RAW10: 2^10 − 1.
pub const SENSOR_WHITE_CODE: u16 = 1023;
/// Байт, которым заполняется паддинг строки: если шейдер его прочтёт, картинка это покажет (как в тестах ISP).
const PADDING_FILL_BYTE: u8 = 0xA5;
/// Сторона блока «шашечек» (края) сцены, пиксели.
const EDGE_BLOCK_PX: u32 = 64;
/// Доля шума от размаха сенсора (≈ ±1,2 % — порядок шума при средней освещённости).
const NOISE_AMPLITUDE: f64 = 0.012;
/// Умолчание серого кадра: код, при котором свет ≈ 0,2 размаха (около среднего серого после тона).
pub const DEFAULT_GRAY_CODE: u16 = 256;

/// Что показывает синтетический кадр.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SceneContent {
    /// Градиенты + края + шум по зерну: ветвления как в обычной сцене.
    Natural {
        /// Зерно шума: один и тот же кадр при одном зерне.
        seed: u32,
    },
    /// Сцена-пробник: рампа от чёрного до пересвета, круги оттенков (тёмные, средние, яркие), шахматка. Проходит ВЕСЬ
    /// диапазон входа ISP — для доказательства «разница ≤ 1 разряда» (см. `scene_probe`). Без шума и зерна.
    Probe,
    /// Один и тот же код во всех отсчётах (серый кадр): худший случай конкуренции `atomicAdd` в приборах.
    UniformGray {
        /// Код RAW10 всех отсчётов, `SENSOR_BLACK_CODE..=SENSOR_WHITE_CODE`.
        code: u16,
    },
}

impl Default for SceneContent {
    /// Естественная сцена с зерном 1.
    fn default() -> Self {
        Self::Natural { seed: 1 }
    }
}

/// Готовый кадр RAW10: байты в раскладке буфера камеры и те же отсчёты как числа (для эталонов в тестах).
#[derive(Debug, Clone)]
pub struct Raw10Frame {
    /// Ширина, пиксели.
    pub width: u32,
    /// Высота, пиксели.
    pub height: u32,
    /// Шаг строки, байты.
    pub row_stride_bytes: u32,
    /// Упакованные байты (`height` строк по `row_stride_bytes`).
    pub bytes: Vec<u8>,
    /// Отсчёты построчно (10 бит в `u16`).
    pub samples: Vec<u16>,
}

/// Хэш-шум 0…1 по координатам и зерну (без состояния: кадр не зависит от порядка генерации).
fn hash_unit(x: u32, y: u32, seed: u32) -> f64 {
    let mut state = x.wrapping_mul(0x9E37_79B1) ^ y.wrapping_mul(0x85EB_CA77) ^ seed.wrapping_mul(0xC2B2_AE3D);
    state ^= state >> 15;
    state = state.wrapping_mul(0x2C1B_3C6D);
    state ^= state >> 12;
    state = state.wrapping_mul(0x297A_2D39);
    state ^= state >> 15;
    f64::from(state) / f64::from(u32::MAX)
}

/// Линейный свет пикселя (0 — чёрный, 1 — белый) естественной сцены.
fn natural_light(x: u32, y: u32, size: PixelSize, seed: u32) -> f64 {
    let gradient = 0.05
        + 0.75 * f64::from(x) / f64::from(size.width.max(2) - 1)
        + 0.10 * f64::from(y) / f64::from(size.height.max(2) - 1);
    let edge = if (x / EDGE_BLOCK_PX + y / EDGE_BLOCK_PX).is_multiple_of(2) {
        0.06
    } else {
        -0.06
    };
    // Лёгкий цветной наклон по чётности пикселя: ячейки CFA получают разный свет, как при цветной сцене.
    let tilt = 0.01 * f64::from((x & 1) + (y & 1));
    let noise = NOISE_AMPLITUDE * (hash_unit(x, y, seed) * 2.0 - 1.0);
    (gradient + edge + tilt + noise).clamp(0.0, 1.0)
}

/// Код RAW10 отсчёта `(x, y)` сцены-пробника: свет ячейки, взятый каналом мозаики RGGB этого отсчёта, в коды сенсора
/// (свет выше белого обрезается белым кодом, как у настоящего АЦП).
fn probe_sample_code(x: u32, y: u32, size: PixelSize) -> u16 {
    let light = probe_cell_light(x / 2, y / 2, size)[probe_channel_of_sample(x, y)];
    let span = f64::from(SENSOR_WHITE_CODE - SENSOR_BLACK_CODE);
    (f64::from(SENSOR_BLACK_CODE) + light.clamp(0.0, 1.0) * span).round() as u16
}

/// Проверяет размер кадра RAW10 (до выделения памяти).
pub(crate) fn check_raw10_size(size: PixelSize) -> Result<(), SaturationWorkError> {
    if size.width == 0 || size.height == 0 {
        return Err(SaturationWorkError::invalid_config(format!(
            "размер кадра {}x{}: нулевая сторона",
            size.width, size.height
        )));
    }
    if !size.width.is_multiple_of(4) || !size.height.is_multiple_of(2) {
        return Err(SaturationWorkError::invalid_config(format!(
            "размер кадра {}x{}: RAW10 упаковывает по 4 пикселя в строке, а ячейка CFA 2x2 — ширина кратна 4, высота кратна 2",
            size.width, size.height
        )));
    }
    Ok(())
}

/// Строит кадр RAW10 `size` с содержимым `content` и паддингом строки `row_padding_bytes`.
pub fn synthesize_raw10(
    size: PixelSize,
    content: SceneContent,
    row_padding_bytes: u32,
) -> Result<Raw10Frame, SaturationWorkError> {
    check_raw10_size(size)?;
    if let SceneContent::UniformGray { code } = content
        && !(SENSOR_BLACK_CODE..=SENSOR_WHITE_CODE).contains(&code)
    {
        return Err(SaturationWorkError::invalid_config(format!(
            "код серого кадра {code} вне {SENSOR_BLACK_CODE}..={SENSOR_WHITE_CODE}"
        )));
    }
    let span = f64::from(SENSOR_WHITE_CODE - SENSOR_BLACK_CODE);
    let samples: Vec<u16> = (0..size.height)
        .flat_map(|y| (0..size.width).map(move |x| (x, y)))
        .map(|(x, y)| match content {
            SceneContent::UniformGray { code } => code,
            SceneContent::Probe => probe_sample_code(x, y, size),
            SceneContent::Natural { seed } => {
                let light = natural_light(x, y, size, seed);
                (f64::from(SENSOR_BLACK_CODE) + light * span).round() as u16
            }
        })
        .collect();
    let row_stride_bytes = size.width / 4 * 5 + row_padding_bytes;
    let bytes = pack_raw10(&samples, size.width, row_stride_bytes, PADDING_FILL_BYTE);
    Ok(Raw10Frame {
        width: size.width,
        height: size.height,
        row_stride_bytes,
        bytes,
        samples,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_gives_same_frame_and_other_seed_differs() {
        let size = PixelSize::new(64, 32);
        let first = synthesize_raw10(size, SceneContent::Natural { seed: 5 }, 0).unwrap();
        let again = synthesize_raw10(size, SceneContent::Natural { seed: 5 }, 0).unwrap();
        let other = synthesize_raw10(size, SceneContent::Natural { seed: 6 }, 0).unwrap();
        assert_eq!(first.samples, again.samples);
        assert_ne!(first.samples, other.samples);
    }

    #[test]
    fn probe_scene_spans_the_whole_sensor_range_with_clipped_whites() {
        let frame = synthesize_raw10(PixelSize::new(256, 192), SceneContent::Probe, 0).unwrap();
        assert_eq!(
            frame.samples.iter().copied().min(),
            Some(SENSOR_BLACK_CODE),
            "есть чёрный"
        );
        assert_eq!(
            frame.samples.iter().copied().max(),
            Some(SENSOR_WHITE_CODE),
            "есть пересвет"
        );
        let clipped = frame
            .samples
            .iter()
            .filter(|&&sample| sample == SENSOR_WHITE_CODE)
            .count();
        let dark = frame
            .samples
            .iter()
            .filter(|&&sample| sample < SENSOR_BLACK_CODE + 8)
            .count();
        assert!(clipped > 200 && dark > 200, "пересвет {clipped}, тёмные коды {dark}");
        let again = synthesize_raw10(PixelSize::new(256, 192), SceneContent::Probe, 0).unwrap();
        assert_eq!(frame.samples, again.samples, "детерминирован");
    }

    #[test]
    fn natural_scene_is_not_flat_and_stays_in_sensor_range() {
        let frame = synthesize_raw10(PixelSize::new(256, 128), SceneContent::default(), 0).unwrap();
        let (min, max) = frame
            .samples
            .iter()
            .fold((u16::MAX, 0), |(lo, hi), &s| (lo.min(s), hi.max(s)));
        assert!(min >= SENSOR_BLACK_CODE && max <= SENSOR_WHITE_CODE);
        assert!(max - min > 300, "сцена должна быть разнообразной: {min}..{max}");
    }

    #[test]
    fn gray_scene_is_constant_and_raw10_stride_matches_camera_layout() {
        let frame = synthesize_raw10(PixelSize::new(4096, 8), SceneContent::UniformGray { code: 300 }, 0).unwrap();
        assert!(frame.samples.iter().all(|&s| s == 300));
        assert_eq!(frame.row_stride_bytes, 5120);
        assert_eq!(frame.bytes.len(), 5120 * 8);
        let padded = synthesize_raw10(PixelSize::new(64, 2), SceneContent::UniformGray { code: 300 }, 16).unwrap();
        assert_eq!(padded.row_stride_bytes, 80 + 16);
    }

    #[test]
    fn bad_sizes_and_codes_are_typed_errors() {
        for size in [PixelSize::new(0, 8), PixelSize::new(10, 8), PixelSize::new(64, 3)] {
            assert!(matches!(
                synthesize_raw10(size, SceneContent::default(), 0),
                Err(SaturationWorkError::InvalidConfig { .. })
            ));
        }
        assert!(matches!(
            synthesize_raw10(PixelSize::new(64, 8), SceneContent::UniformGray { code: 5000 }, 0),
            Err(SaturationWorkError::InvalidConfig { .. })
        ));
    }
}
