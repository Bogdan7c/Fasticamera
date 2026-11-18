//! Паспорт RAW-кадра: всё, что ISP и потребители должны знать о кадре, кроме самих пикселей.
//!
//! Паспорт состоит из двух частей (сессия 05, V2 README):
//! * [`StreamPassport`] — постоянная часть потока: формат, размер буфера, полезная область,
//!   мозаика, уровни сенсора из характеристик, вендорская обработка, ориентация, частота.
//!   Одна на всю сессию камеры, кадры делят её через `Arc`;
//! * [`FrameCaptureMeta`] — покадровая часть из результата запроса (время, экспозиция, ISO,
//!   перекос rolling shutter, динамический black level, подсказка баланса белого).
//!
//! Главный инвариант: покадровые метаданные принадлежат **именно этому** кадру. Пара
//! «кадр ↔ результат» собирается только по точному совпадению `SENSOR_TIMESTAMP`; если пары нет,
//! это честное [`FrameCaptureMeta::Missing`] с причиной (решение 42), а не метаданные соседа.

use std::sync::Arc;

use serde::Serialize;

use crate::capabilities::{ColorCalibrationSet, Rational, SensorLevels, VendorProcessingFlags};
use crate::capture_result::{CaptureResultMeta, SensorTimestamp};
use crate::geometry::{PixelRect, PixelSize, SensorOrientation};
use crate::ids::CameraId;
use crate::raw_format::{MosaicPattern, RawFormat};
use crate::stream::{FrameRateMechanism, ReadoutAspect, TargetFrameRate};

/// Постоянная часть паспорта: одинакова для всех кадров одной сконфигурированной сессии.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StreamPassport {
    pub camera: CameraId,
    pub raw_format: RawFormat,
    /// Размер RAW-буфера в пикселях (для 16:9 основной камеры — всё равно 4096×3072).
    pub buffer_size: PixelSize,
    pub readout: ReadoutAspect,
    /// Где в буфере настоящие данные. 16:9@60 основной: строки 0–2303 из 3072, ниже нули
    /// (00 C2) — ISP обязан читать только эту область.
    pub valid_area: PixelRect,
    pub mosaic: MosaicPattern,
    /// Статические уровни из характеристик; динамические (если HAL отдаёт) — в покадровой части.
    pub static_levels: SensorLevels,
    pub vendor_processing: VendorProcessingFlags,
    pub sensor_orientation: SensorOrientation,
    /// Частота, которая реально сконфигурирована (после возможного отката 4:3@60 → 30).
    pub frame_rate: TargetFrameRate,
    pub frame_rate_mechanism: FrameRateMechanism,
    /// Цветовая калибровка этой камеры (DNG-матрицы на 1–2 иллюминанта) из характеристик.
    ///
    /// Живёт в паспорте (сессия 07), потому что ISP видоискателя видит только кадр с паспортом,
    /// а не каталог камер: без матриц он не может перевести цвет сенсора в цвет экрана.
    /// Пустой список — HAL калибровку не отдал; ISP обязан честно отказаться от цвета, а не
    /// подставить чужие матрицы.
    pub color_calibration: Vec<ColorCalibrationSet>,
}

/// Порядковый номер кадра внутри нашего потока (0, 1, 2…), который присваивает лизинг.
///
/// Не путать с `FrameNumber` из Camera2: тот есть только у кадров с парой и может прыгать
/// между сессиями. Этот — непрерывный счёт принятых буферов, по нему видно пропуски у нас.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct FrameSequence(pub u64);

/// Коэффициенты баланса белого, которые применил бы HAL (`ACAMERA_COLOR_CORRECTION_GAINS`),
/// по каналам в порядке `[R, G_even, G_odd, B]`. Только подсказка: баланс белого наш (решение 9).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ColorCorrectionGains(pub [f32; 4]);

/// Цветовые подсказки из результата запроса. `None` — HAL ключ не отдал.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct CaptureColorHints {
    /// `ACAMERA_SENSOR_DYNAMIC_BLACK_LEVEL` по каналам квадрата 2×2 (на OnePlus 11 не отдаётся, 00 C3).
    pub dynamic_black_level: Option<[f32; 4]>,
    /// `ACAMERA_SENSOR_DYNAMIC_WHITE_LEVEL`.
    pub dynamic_white_level: Option<u32>,
    pub color_gains: Option<ColorCorrectionGains>,
    /// `ACAMERA_SENSOR_NEUTRAL_COLOR_POINT`: нейтральный цвет в пространстве сенсора (для DNG).
    pub neutral_color_point: Option<[Rational; 3]>,
}

/// Полные покадровые метаданные: то, что движок знает о результате, плюс цветовые подсказки.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct FrameCaptureDetails {
    pub result: CaptureResultMeta,
    pub color: CaptureColorHints,
}

/// Почему у кадра нет покадровых метаданных.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MissingCaptureMeta {
    /// Результат с такой меткой не пришёл за время ожидания пары (потерян или камера их не шлёт —
    /// логическая камера 0, факт 04).
    PairingTimedOut,
    /// Поток остановили, пока кадр ждал пару.
    StreamStopped,
}

/// Покадровая часть паспорта.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum FrameCaptureMeta {
    /// Результат с той же меткой сенсора, что у буфера.
    Paired(FrameCaptureDetails),
    /// Метаданных нет; чужие не подставлены (решение 42).
    Missing { reason: MissingCaptureMeta },
}

impl FrameCaptureMeta {
    /// Метаданные кадра, если пара собрана.
    pub fn details(&self) -> Option<&FrameCaptureDetails> {
        match self {
            Self::Paired(details) => Some(details),
            Self::Missing { .. } => None,
        }
    }
}

/// Паспорт одного RAW-кадра.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RawFramePassport {
    /// Общая для потока часть. В сериализацию кадра не входит: в отчётах она пишется один раз
    /// на поток, а не повторяется у каждого кадра.
    #[serde(skip)]
    pub stream: Arc<StreamPassport>,
    pub sequence: FrameSequence,
    /// Время начала экспозиции из буфера (`AImage_getTimestamp` = `SENSOR_TIMESTAMP`, шкала BOOTTIME).
    pub sensor_timestamp: SensorTimestamp,
    pub capture: FrameCaptureMeta,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capture_result::FrameNumber;

    fn result_at(nanos: i64) -> CaptureResultMeta {
        CaptureResultMeta {
            frame_number: FrameNumber(3),
            sensor_timestamp: Some(SensorTimestamp::from_nanos(nanos)),
            exposure_time: None,
            sensitivity_iso: Some(400),
            reported_frame_duration: None,
            rolling_shutter_skew: None,
            three_a: crate::three_a::ThreeAResultState::unreported(crate::three_a::RequestOrigin::Repeating),
        }
    }

    #[test]
    fn details_are_present_only_for_a_paired_frame() {
        let paired = FrameCaptureMeta::Paired(FrameCaptureDetails {
            result: result_at(42),
            color: CaptureColorHints::default(),
        });
        assert_eq!(
            paired.details().map(|details| details.result.frame_number),
            Some(FrameNumber(3))
        );

        let missing = FrameCaptureMeta::Missing {
            reason: MissingCaptureMeta::PairingTimedOut,
        };
        assert_eq!(missing.details(), None);
    }

    #[test]
    fn missing_meta_serializes_with_its_reason() {
        let missing = FrameCaptureMeta::Missing {
            reason: MissingCaptureMeta::StreamStopped,
        };
        let json = serde_json::to_value(missing).expect("сериализация");
        assert_eq!(
            json,
            serde_json::json!({"state": "missing", "reason": "stream_stopped"})
        );
    }
}
