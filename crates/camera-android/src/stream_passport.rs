//! Сборка постоянной части паспорта RAW-кадров ([`StreamPassport`]) для сессии камеры.
//!
//! Паспорт собирается один раз на сессию из трёх источников, которые бэкенд знает на момент
//! `configure_session`: режим потока ([`ResolvedStream`]), возможности камеры
//! ([`CameraCapabilities`]) и выбранный рычаг частоты ([`FrameRateMechanism`]). Модуль нейтральный
//! (без NDK) и проверяется на ПК по образцам OnePlus 11.
//!
//! # Полезная область буфера
//! * 4:3 — данные занимают весь буфер.
//! * 16:9 — данные занимают строки `0..ширина×9/16` от самого верха (на основной камере 4096×3072
//!   это строки 0–2303, ниже нули, измерено в сессии 00, C2). **Ширик в 16:9 не измерялся**:
//!   для него действует то же правило как допущение, и ISP обязан сверить его первой же съёмкой.
//!
//! # Где полезная область лежит на сенсоре (`sensor_area`)
//! * 4:3 — на всей активной области сенсора.
//! * 16:9 — центральная полоса на всю ширину: высота = ширина × 9/16 в пикселях активной области,
//!   по центру по вертикали (верх округляется вниз до чётного — фаза CFA). Измерено на основной
//!   камере: отношение RAW 16:9 к RAW 4:3 постоянно 1,01–1,02 по всему полю, то есть полоса —
//!   именно центральная вырезка, а не масштабированный полный кадр. Буфер может быть меньше
//!   активной области (биннинг) — полоса остаётся в пикселях активной области, а соответствие
//!   буфера и сенсора получается масштабом (`isp-model`).
//!
//! # Калибровка виньетирования
//! Калибровки приходят снаружи (из профиля устройства, не из Kotlin-конфига) набором
//! [`LensShadingCalibrations`]; паспорт получает ту, что подходит камере и её активной области.
//! Несовпадение области — не молчаливая подгонка, а `NotCalibrated(ActiveArrayMismatch)` и warn.
//!
//! # Мозаика
//! Публичный RAW10/RAW16 — это уже биннинг до Bayer (родная мозаика сенсора — QCFA, но она описана
//! отдельно в `RawSensorInfo::native_mosaic` и в буфер не попадает), поэтому вид мозаики буфера —
//! всегда `Bayer`, а раскладка берётся из характеристик камеры.

use camera_contract::capabilities::{CameraCapabilities, RawSupport};
use camera_contract::frame_passport::StreamPassport;
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::ids::CameraId;
use camera_contract::lens_shading::{
    InvalidSensorArea, LensShadingCalibrationState, LensShadingCalibrations, NotCalibratedReason, ValidAreaOnSensor,
};
use camera_contract::raw_format::{MosaicKind, MosaicPattern};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, ResolvedStream};

/// Числитель и знаменатель соотношения сторон 16:9: высота = ширина × 9 / 16.
const WIDESCREEN_HEIGHT_NUMERATOR: u64 = 9;
const WIDESCREEN_HEIGHT_DENOMINATOR: u64 = 16;

/// Почему паспорт собрать нельзя.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PassportError {
    /// У камеры нет RAW: уровни сенсора и раскладку CFA взять неоткуда.
    #[error("камера не поддерживает RAW: паспорт кадра собрать нельзя")]
    RawNotSupported,
    /// Полоса 16:9 не помещается в буфер (буфер шире, чем выше в пропорции 16:9).
    #[error("полоса 16:9 высотой {required_height} строк не помещается в буфер {buffer_width}×{buffer_height}")]
    WidescreenDoesNotFit {
        buffer_width: u32,
        buffer_height: u32,
        required_height: u64,
    },
    /// Полоса 16:9 не помещается в активную область сенсора.
    #[error("полоса 16:9 не помещается в активную область сенсора: {0}")]
    SensorArea(#[from] InvalidSensorArea),
}

/// Собирает паспорт потока для сконфигурированной сессии.
///
/// `calibrations` — наши калибровки виньетирования устройства; `LensShadingCalibrations::none()`
/// допустим там, где нужна только геометрия (экран камеры): паспорт тогда `NotCalibrated`.
pub fn build_stream_passport(
    stream: &ResolvedStream,
    capabilities: &CameraCapabilities,
    mechanism: FrameRateMechanism,
    calibrations: &LensShadingCalibrations,
) -> Result<StreamPassport, PassportError> {
    let RawSupport::Supported(raw) = &capabilities.raw else {
        return Err(PassportError::RawNotSupported);
    };
    let valid = valid_area(stream.readout, stream.buffer_size)?;
    let sensor_area = sensor_area(stream.readout, capabilities.active_array.size)?;
    Ok(StreamPassport {
        camera: stream.camera.clone(),
        raw_format: stream.raw_format,
        buffer_size: stream.buffer_size,
        readout: stream.readout,
        valid_area: valid,
        sensor_area,
        mosaic: MosaicPattern {
            kind: MosaicKind::Bayer,
            layout: raw.cfa,
        },
        static_levels: raw.levels,
        vendor_processing: capabilities.vendor_processing,
        lens_shading: lens_shading_state(calibrations, &stream.camera, capabilities.active_array.size),
        sensor_orientation: capabilities.sensor_orientation,
        frame_rate: stream.frame_rate,
        frame_rate_mechanism: mechanism,
        // Матрицы цвета идут в паспорт как есть: ISP видоискателя (07) видит только паспорт.
        color_calibration: raw.color_calibration.clone(),
    })
}

/// Состояние калибровки виньетирования потока; несовпадение активной области — предупреждение в лог.
fn lens_shading_state(
    calibrations: &LensShadingCalibrations,
    camera: &CameraId,
    active_array: PixelSize,
) -> LensShadingCalibrationState {
    let state = calibrations.state_for(camera, active_array);
    if let LensShadingCalibrationState::NotCalibrated(reason @ NotCalibratedReason::ActiveArrayMismatch { .. }) = &state
    {
        tracing::warn!(
            target: "fasticamera::camera_android::stream_passport",
            %camera,
            ?reason,
            "калибровка виньетирования снята для другой активной области: коррекция выключена"
        );
    }
    state
}

/// Где полезная область потока лежит на активной области сенсора (см. доку модуля).
pub(crate) fn sensor_area(readout: ReadoutAspect, active_array: PixelSize) -> Result<ValidAreaOnSensor, PassportError> {
    match readout {
        ReadoutAspect::FourByThree => Ok(ValidAreaOnSensor::whole_active_array(active_array)),
        ReadoutAspect::SixteenByNine => {
            let band_height =
                u64::from(active_array.width) * WIDESCREEN_HEIGHT_NUMERATOR / WIDESCREEN_HEIGHT_DENOMINATOR;
            // Полоса выше активной области — `InvalidSensorArea` ниже; высота в u32 по построению (≤ ширина).
            let band_height = u32::try_from(band_height).unwrap_or(u32::MAX);
            let free_rows = active_array.height.saturating_sub(band_height);
            // Центр по вертикали; верх чётный, иначе фаза CFA строк полосы разойдётся с сенсором.
            let top = (free_rows / 2) & !1;
            Ok(ValidAreaOnSensor::new(
                active_array,
                PixelRect {
                    left: 0,
                    top,
                    size: PixelSize::new(active_array.width, band_height),
                },
            )?)
        }
    }
}

/// Где в буфере лежат настоящие данные (см. доку модуля).
pub(crate) fn valid_area(readout: ReadoutAspect, buffer: PixelSize) -> Result<PixelRect, PassportError> {
    let height = match readout {
        ReadoutAspect::FourByThree => buffer.height,
        ReadoutAspect::SixteenByNine => {
            let required = u64::from(buffer.width) * WIDESCREEN_HEIGHT_NUMERATOR / WIDESCREEN_HEIGHT_DENOMINATOR;
            match u32::try_from(required) {
                Ok(rows) if rows <= buffer.height => rows,
                _ => {
                    return Err(PassportError::WidescreenDoesNotFit {
                        buffer_width: buffer.width,
                        buffer_height: buffer.height,
                        required_height: required,
                    });
                }
            }
        }
    };
    Ok(PixelRect {
        left: 0,
        top: 0,
        size: PixelSize::new(buffer.width, height),
    })
}

#[cfg(test)]
#[path = "stream_passport_tests.rs"]
mod tests;
