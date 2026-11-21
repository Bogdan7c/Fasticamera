//! Выбор рычага частоты кадров для режима потока: как просить 60 к/с на конкретной камере.
//!
//! Правила повторяют то, что зонд сессии 00 получил на телефоне (00 C2), и фиксируют то, чего
//! зонд не доказал, явными решениями:
//!
//! 1. **Частота в пределах публичного потолка** (на OnePlus 11 это 30 к/с: `min_frame_duration`
//!    RAW-потока 33,3 мс) — публичный диапазон AE `[fps, fps]`, если камера объявляет подходящий
//!    диапазон; нет подходящего — `HalDefault` (HAL сам выбирает, мы ничего не обещаем).
//! 2. **Выше потолка, вырезка 16:9** (основная 16:9@60) — публичный `CONTROL_AE_TARGET_FPS_RANGE
//!    = [fps, fps]`: HAL уходит в родной режим сенсора 4096×2304@60. Диапазон `[60,60]` не объявлен
//!    камерой, но принимается — это факт 00 (попытка 2). **Только если вендорская таблица режимов
//!    сенсора** ([`crate::sensor_modes`]) предлагает режим размера вырезки (ширина × ширина·9/16)
//!    на этой частоте или выше.
//! 3. **Выше потолка, полный кадр 4:3**:
//!    * камера из списка проверенных и оба вендорских тега найдены → вендорское переопределение
//!      длительности кадра (основная 4:3@60, попытка 4); диапазон AE при этом **не ставится**,
//!      иначе HAL уходит в 16:9 (попытка 4б). Таблица режимов тут не нужна: у основной в ней нет
//!      4096×3072@60, а вендорский рычаг работает;
//!    * камера из списка, но теги не найдены (другая прошивка) → `FrameRateNotAchievable`;
//!      молча подменить AE-диапазоном нельзя — вышел бы другой режим сенсора, чем просили;
//!    * камера вне списка (ширик, фронт) → публичный AE `[fps, fps]` **только если таблица режимов**
//!      предлагает режим размера буфера на этой частоте. У ширика есть 4000×3000@60 (попытка 16),
//!      у фронта 60 к/с нет вовсе: HAL берёт полноразмерный режим, буфер получается мал, и
//!      провайдер камеры падает (`SIGABRT` в `BindImportedBuffer`).
//! 4. Частота выше `max_public_range_frame_rate` без вендорского рычага — `FrameRateNotAchievable`.
//!
//! **Решение (сессия 13, приёмка на телефоне):** AE-ветки выше публичного потолка без подтверждения
//! таблицей недопустимы. Таблицы нет на прошивке или она непригодна (нечитаема/битая) — тоже
//! `FrameRateNotAchievable`: цена ошибочной догадки — падение провайдера камеры, цена отказа —
//! движок откатывается на запасные 30 к/с (`retry_with_fallback`) и переключение объектива завершается.
//!
//! Автооткат 4:3@60 → 30 при сбое (решение 23) — не здесь: это политика сессии 05.

use camera_contract::capabilities::{CameraCapabilities, FpsRange};
use camera_contract::stream::{FrameRateMechanism, ReadoutAspect, ResolvedStream, TargetFrameRate};
use camera_core::backend::UnsupportedStream;

use camera_contract::geometry::PixelSize;

use crate::config::AndroidCameraConfig;
use crate::sensor_modes::{NativeModeSupport, SensorModeAvailability};
use crate::stream_passport::valid_area;
use crate::vendor::{FrameDurationOverrideSupport, VendorCapabilities};

/// Выбирает рычаг частоты или объясняет, почему режим недостижим.
pub fn plan_frame_rate(
    stream: &ResolvedStream,
    capabilities: &CameraCapabilities,
    vendor: &VendorCapabilities,
    sensor_modes: &SensorModeAvailability,
    config: &AndroidCameraConfig,
) -> Result<FrameRateMechanism, UnsupportedStream> {
    let requested = stream.frame_rate;
    let fps = requested.frames_per_second();
    let not_achievable = UnsupportedStream::FrameRateNotAchievable { requested };

    let option = capabilities
        .raw_streams
        .iter()
        .find(|option| option.format == stream.raw_format && option.size == stream.buffer_size)
        .ok_or(UnsupportedStream::FormatNotOffered {
            format: stream.raw_format,
        })?;

    if within_public_ceiling(option.min_frame_duration.as_nanos(), requested) {
        return Ok(public_range_for(&capabilities.ae_fps_ranges, fps)
            .map_or(FrameRateMechanism::HalDefault, |range| {
                FrameRateMechanism::PublicAeTargetRange { range }
            }));
    }

    match stream.readout {
        ReadoutAspect::SixteenByNine => {
            // Размер вырезки считает тот же код, что и паспорт потока (строки 0..rows буфера).
            // Вырезка не помещается в буфер — режима с таким размером быть не может.
            let Ok(readout_area) = valid_area(stream.readout, stream.buffer_size) else {
                return Err(not_achievable);
            };
            ae_range_on_native_mode(readout_area.size, requested, sensor_modes, config).ok_or(not_achievable)
        }
        ReadoutAspect::FourByThree => match vendor.frame_duration_override_support() {
            FrameDurationOverrideSupport::Available => Ok(FrameRateMechanism::VendorSensorFrameDurationOverride {
                frame_duration: requested.frame_duration(),
            }),
            FrameDurationOverrideSupport::TagsMissing => Err(not_achievable),
            FrameDurationOverrideSupport::NotAllowedForCamera => {
                ae_range_on_native_mode(stream.buffer_size, requested, sensor_modes, config).ok_or(not_achievable)
            }
        },
    }
}

/// Допуск сравнения длительностей кадра, нс. HAL объявляет минимальную длительность целым числом
/// наносекунд, а длительность цели ([`TargetFrameRate::frame_duration`]) округляется до ближайшей
/// наносекунды: на 60 к/с цель — 16 666 667 нс, и объявленные HAL 16 666 666 или 16 666 667 нс
/// означают те же 60 к/с. Микросекунда с запасом покрывает такое округление и при этом на порядки
/// меньше разницы между соседними частотами (60 к/с = 16,67 мс, 30 к/с = 33,33 мс).
const FRAME_DURATION_TOLERANCE_NANOS: u64 = 1_000;

/// Запрошенная частота не выше публичного потолка. Нулевая длительность — «HAL не ограничил» —
/// потолка нет, значит и «выше потолка» быть не может.
///
/// Сравниваются **длительности**, а не целые к/с: деление `1e9 / min_frame_duration` с отбрасыванием
/// дробной части превращало 16 666 667 нс в 59 к/с, и честные 60 к/с считались «выше потолка».
fn within_public_ceiling(min_frame_duration_nanos: u64, requested: TargetFrameRate) -> bool {
    if min_frame_duration_nanos == 0 {
        return true;
    }
    let target_nanos = requested.frame_duration().as_nanos();
    min_frame_duration_nanos <= target_nanos.saturating_add(FRAME_DURATION_TOLERANCE_NANOS)
}

/// Лучший объявленный диапазон AE для частоты: сначала точный `[fps, fps]`, затем самый узкий
/// из объемлющих (при равной ширине — с большим нижним краем).
fn public_range_for(ranges: &[FpsRange], fps: u32) -> Option<FpsRange> {
    ranges
        .iter()
        .copied()
        .filter(|range| range.min <= fps && fps <= range.max)
        .min_by_key(|range| (range.max - range.min, std::cmp::Reverse(range.min)))
}

/// Публичный диапазон `[fps, fps]` выше публичного потолка: только если частота не выше настроенного
/// предела попыток **и** таблица режимов сенсора предлагает родной режим размера `readout_size`.
fn ae_range_on_native_mode(
    readout_size: PixelSize,
    requested: TargetFrameRate,
    sensor_modes: &SensorModeAvailability,
    config: &AndroidCameraConfig,
) -> Option<FrameRateMechanism> {
    let fps = requested.frames_per_second();
    if fps > config.max_public_range_frame_rate {
        return None;
    }
    match sensor_modes.native_mode_support(readout_size, requested) {
        NativeModeSupport::Offered => Some(FrameRateMechanism::PublicAeTargetRange {
            range: FpsRange { min: fps, max: fps },
        }),
        NativeModeSupport::NotOffered => None,
    }
}

#[cfg(test)]
#[path = "frame_rate_plan_tests.rs"]
mod tests;
