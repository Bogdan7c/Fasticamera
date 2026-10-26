//! Параметры smoke-сценариев камеры (`camera.scenarios`): шаги `camera-stream`, пороги, дедлайны.
//!
//! Всё, что в сценариях похоже на число или список (какая камера, какая частота, сколько ждать,
//! какой минимум кадров в секунду считается «60»), живёт здесь, а не в коде сценариев: сценарий
//! только исполняет таблицу. Значения по умолчанию взяты из фактов сессии 00
//! (`mem:device/oneplus11-camera-facts`, `user/foundation/results/00.md`) и решения 21 (оба формата RAW).
//!
//! Типы настроек (`RawFormatSetting`, `ReadoutSetting`, `LensRoleSetting`) повторяют словарь
//! `camera-contract`, потому что тот сериализуется, но не разбирается из JSON: правила конфига
//! (`deny_unknown_fields`, snake_case) не должны протекать в нейтральный словарь.

use camera_contract::control::CaptureIntent;
use camera_contract::ids::{CameraId, CameraTarget, EmptyCameraId, LensRole};
use camera_contract::raw_format::RawFormat;
use camera_contract::stream::{InvalidFrameRate, ReadoutAspect, StreamMode, TargetFrameRate};
use serde::{Deserialize, Serialize};

/// Формат RAW в конфиге.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RawFormatSetting {
    Raw10,
    Raw16,
}

impl RawFormatSetting {
    /// Имя формата для отчётов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            RawFormatSetting::Raw10 => "raw10",
            RawFormatSetting::Raw16 => "raw16",
        }
    }

    pub fn to_contract(self) -> RawFormat {
        match self {
            RawFormatSetting::Raw10 => RawFormat::Raw10,
            RawFormatSetting::Raw16 => RawFormat::Raw16,
        }
    }
}

/// Какую часть сенсора читать, в конфиге.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadoutSetting {
    FourByThree,
    SixteenByNine,
}

impl ReadoutSetting {
    pub fn to_contract(self) -> ReadoutAspect {
        match self {
            ReadoutSetting::FourByThree => ReadoutAspect::FourByThree,
            ReadoutSetting::SixteenByNine => ReadoutAspect::SixteenByNine,
        }
    }
}

/// Роль объектива в конфиге.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LensRoleSetting {
    UltraWide,
    Wide,
    Tele,
    Front,
}

impl LensRoleSetting {
    pub fn to_contract(self) -> LensRole {
        match self {
            LensRoleSetting::UltraWide => LensRole::UltraWide,
            LensRoleSetting::Wide => LensRole::Wide,
            LensRoleSetting::Tele => LensRole::Tele,
            LensRoleSetting::Front => LensRole::Front,
        }
    }
}

/// Каким рычагом частоты шаг ожидает достичь цели. `Any` — рычаг не проверяется
/// (30 к/с достигаются по умолчанию HAL, и это тоже нормально).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExpectedLever {
    Any,
    /// Публичный диапазон AE (`public_ae_target_range`).
    PublicAeTargetRange,
    /// Вендорское переопределение длительности кадра (`vendor_sensor_frame_duration_override`).
    VendorSensorFrameDurationOverride,
}

impl ExpectedLever {
    /// Имя рычага для отчётов (совпадает с serde); `None` — рычаг не проверяется.
    pub fn name(self) -> Option<&'static str> {
        match self {
            ExpectedLever::Any => None,
            ExpectedLever::PublicAeTargetRange => Some("public_ae_target_range"),
            ExpectedLever::VendorSensorFrameDurationOverride => Some("vendor_sensor_frame_duration_override"),
        }
    }
}

/// Присылает ли HAL камеры финальные результаты кадров (`onCaptureCompleted`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureResultsExpectation {
    /// Обычный случай (физические камеры): финальный результат на каждый кадр, частота судится по меткам сенсора.
    #[default]
    Required,
    /// Факт телефона (сессия 04): HAL логической камеры 0 шлёт только частичные результаты
    /// (`onCaptureProgressed`), финальных нет; Java-зонд сессии 00 подтверждает (`img_fps` 30, `res_fps` null).
    /// Частота судится по RAW-кадрам, а отсутствие финальных результатов — сторож факта: если они
    /// появятся (прошивка), шаг падает, чтобы пересмотреть ожидание.
    NotSentByHal,
}

impl CaptureResultsExpectation {
    pub fn is_required(&self) -> bool {
        matches!(self, CaptureResultsExpectation::Required)
    }
}

/// Один шаг `camera-stream`: что открыть, какой режим просить, что считать успехом.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamStepConfig {
    /// ID камеры Camera2 (`"0"` — логическая, `"2"` — основная, ...).
    pub camera_id: String,
    pub raw_format: RawFormatSetting,
    pub readout: ReadoutSetting,
    /// Целевая частота, к/с.
    pub frame_rate: u32,
    pub expected_lever: ExpectedLever,
    /// Минимальная измеренная по меткам сенсора частота, к/с. Ниже цели на запас: средняя частота
    /// за секунды замера не бывает ровно 60,00 (джиттер, первые кадры).
    pub min_measured_fps: f64,
    /// Ждём ли финальных результатов кадров от HAL (по умолчанию да).
    #[serde(default)]
    pub capture_results: CaptureResultsExpectation,
}

/// Режим и цель шага в виде словаря контракта.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedStep {
    pub target: CameraTarget,
    pub mode: StreamMode,
}

impl StreamStepConfig {
    /// Шаг в виде команд движку: цель открытия и режим потока.
    pub fn resolve(&self) -> Result<ResolvedStep, CameraScenarioConfigError> {
        let camera = CameraId::new(self.camera_id.as_str())?;
        let frame_rate = TargetFrameRate::new(self.frame_rate)?;
        Ok(ResolvedStep {
            target: CameraTarget::Camera(camera),
            mode: StreamMode {
                raw_format: self.raw_format.to_contract(),
                readout: self.readout.to_contract(),
                frame_rate,
            },
        })
    }

    /// Подпись шага для строки результата, где счёт идёт на байты: `2/raw10/16x9@60`.
    pub fn short_label(&self) -> String {
        let readout = match self.readout {
            ReadoutSetting::FourByThree => "4x3",
            ReadoutSetting::SixteenByNine => "16x9",
        };
        format!(
            "{}/{}/{readout}@{}",
            self.camera_id,
            self.raw_format.name(),
            self.frame_rate
        )
    }

    /// Подпись шага для сообщений о нарушениях: `камера 2 raw10 16:9@60`.
    pub fn label(&self) -> String {
        let readout = match self.readout {
            ReadoutSetting::FourByThree => "4:3",
            ReadoutSetting::SixteenByNine => "16:9",
        };
        format!(
            "камера {} {} {readout}@{}",
            self.camera_id,
            self.raw_format.name(),
            self.frame_rate
        )
    }
}

/// Параметры `camera-evict`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct EvictScenarioConfig {
    /// Какую камеру открывать по роли (как делает продукт).
    pub role: LensRoleSetting,
    pub raw_format: RawFormatSetting,
    pub readout: ReadoutSetting,
    /// Частота, к/с: 30 достаточно, вытеснение от частоты не зависит.
    pub frame_rate: u32,
    /// Сколько ждать `Streaming` после открытия, мс.
    pub streaming_timeout_ms: u64,
    /// Сколько ждать события недоступности после стадии `streaming`, мс. Включает запуск системной
    /// камеры скриптом, её открытие и паузу: 30 с с большим запасом.
    pub eviction_timeout_ms: u64,
    /// Сколько ждать восстановления потока после недоступности, мс. Считается от события
    /// недоступности; включает пребывание в фоне и возврат приложения скриптом.
    pub recovery_timeout_ms: u64,
}

impl Default for EvictScenarioConfig {
    fn default() -> Self {
        Self {
            role: LensRoleSetting::Wide,
            raw_format: RawFormatSetting::Raw10,
            readout: ReadoutSetting::FourByThree,
            frame_rate: 30,
            streaming_timeout_ms: 15_000,
            eviction_timeout_ms: 30_000,
            recovery_timeout_ms: 30_000,
        }
    }
}

impl EvictScenarioConfig {
    /// Цель и режим потока сценария вытеснения.
    pub fn resolve(&self) -> Result<ResolvedStep, CameraScenarioConfigError> {
        Ok(ResolvedStep {
            target: CameraTarget::Role(self.role.to_contract()),
            mode: StreamMode {
                raw_format: self.raw_format.to_contract(),
                readout: self.readout.to_contract(),
                frame_rate: TargetFrameRate::new(self.frame_rate)?,
            },
        })
    }
}

/// Для чего сценарии открывают поток: видоискатель.
pub const SCENARIO_CAPTURE_INTENT: CaptureIntent = CaptureIntent::Preview;

/// Параметры всех сценариев камеры.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CameraScenarioConfig {
    /// Сколько `camera-catalog` ждёт прочитанного каталога, мс.
    pub catalog_timeout_ms: u64,
    /// Сколько `camera-stream` ждёт `Streaming` на каждом шаге, мс. Первый запуск сессии на телефоне
    /// занимает секунды (HAL), 15 с — с запасом.
    pub stream_start_timeout_ms: u64,
    /// Сколько `camera-stream` ждёт закрытия камеры между шагами, мс.
    pub stream_close_timeout_ms: u64,
    /// Сколько секунд (в мс) идёт замер на каждом шаге после `Streaming`. 5 с — 300 кадров при 60 к/с.
    pub measure_ms: u64,
    /// Шаги `camera-stream` (по умолчанию — матрица решения 21 и фактов 00).
    pub stream_steps: Vec<StreamStepConfig>,
    pub evict: EvictScenarioConfig,
}

fn step(
    camera_id: &str,
    raw_format: RawFormatSetting,
    readout: ReadoutSetting,
    frame_rate: u32,
    expected_lever: ExpectedLever,
    min_measured_fps: f64,
) -> StreamStepConfig {
    StreamStepConfig {
        camera_id: camera_id.to_owned(),
        raw_format,
        readout,
        frame_rate,
        expected_lever,
        min_measured_fps,
        capture_results: CaptureResultsExpectation::Required,
    }
}

impl Default for CameraScenarioConfig {
    fn default() -> Self {
        use ExpectedLever::{Any, PublicAeTargetRange, VendorSensorFrameDurationOverride};
        use RawFormatSetting::{Raw10, Raw16};
        use ReadoutSetting::{FourByThree, SixteenByNine};
        Self {
            catalog_timeout_ms: 10_000,
            stream_start_timeout_ms: 15_000,
            stream_close_timeout_ms: 5_000,
            measure_ms: 5_000,
            stream_steps: vec![
                // Основная 16:9@60 — публичный AE-диапазон (00: C2).
                step("2", Raw10, SixteenByNine, 60, PublicAeTargetRange, 57.0),
                // Основная 4:3@60 — только вендорское переопределение длительности кадра (решение 23).
                step("2", Raw10, FourByThree, 60, VendorSensorFrameDurationOverride, 57.0),
                // Ширик 4:3@60 — публичный AE-диапазон; вендорские ключи на нём ломают поток (00, попытка 17).
                step("3", Raw10, FourByThree, 60, PublicAeTargetRange, 55.0),
                // Фронталка 30 к/с: рычаг не важен (HAL по умолчанию).
                step("1", Raw10, FourByThree, 30, Any, 28.0),
                // Логическая 0 (камера без роли, видна приложению) 30 к/с. Факт сессии 04: её HAL шлёт только
                // частичные результаты, финальных нет, поэтому частота судится по RAW-кадрам.
                StreamStepConfig {
                    capture_results: CaptureResultsExpectation::NotSentByHal,
                    ..step("0", Raw10, FourByThree, 30, Any, 28.0)
                },
                // Решение 21: оба формата. RAW16 на основной 16:9@60.
                step("2", Raw16, SixteenByNine, 60, PublicAeTargetRange, 57.0),
            ],
            evict: EvictScenarioConfig::default(),
        }
    }
}

/// Неверные параметры сценариев камеры.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CameraScenarioConfigError {
    #[error("{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("stream_steps не может быть пустым: сценарию camera-stream нечего проверять")]
    NoStreamSteps,
    #[error("шаг {index} stream_steps: пустой ID камеры")]
    EmptyCameraId { index: usize },
    #[error("частота: {0}")]
    InvalidFrameRate(#[from] InvalidFrameRate),
    #[error("ID камеры: {0}")]
    InvalidCameraId(#[from] EmptyCameraId),
    #[error(
        "шаг {index} stream_steps: min_measured_fps ({min_measured_fps}) должно быть конечным, больше нуля и не больше цели ({frame_rate})"
    )]
    InvalidMinimumRate {
        index: usize,
        min_measured_fps: f64,
        frame_rate: u32,
    },
}

impl CameraScenarioConfig {
    pub fn validate(&self) -> Result<(), CameraScenarioConfigError> {
        for (field, value) in [
            ("catalog_timeout_ms", self.catalog_timeout_ms),
            ("stream_start_timeout_ms", self.stream_start_timeout_ms),
            ("stream_close_timeout_ms", self.stream_close_timeout_ms),
            ("measure_ms", self.measure_ms),
            ("evict.streaming_timeout_ms", self.evict.streaming_timeout_ms),
            ("evict.eviction_timeout_ms", self.evict.eviction_timeout_ms),
            ("evict.recovery_timeout_ms", self.evict.recovery_timeout_ms),
        ] {
            if value == 0 {
                return Err(CameraScenarioConfigError::Zero { field });
            }
        }
        if self.stream_steps.is_empty() {
            return Err(CameraScenarioConfigError::NoStreamSteps);
        }
        for (index, configured) in self.stream_steps.iter().enumerate() {
            if configured.camera_id.is_empty() {
                return Err(CameraScenarioConfigError::EmptyCameraId { index });
            }
            configured.resolve()?;
            let minimum_ok = configured.min_measured_fps.is_finite()
                && configured.min_measured_fps > 0.0
                && configured.min_measured_fps <= f64::from(configured.frame_rate);
            if !minimum_ok {
                return Err(CameraScenarioConfigError::InvalidMinimumRate {
                    index,
                    min_measured_fps: configured.min_measured_fps,
                    frame_rate: configured.frame_rate,
                });
            }
        }
        self.evict.resolve()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "camera_scenario_config_tests.rs"]
mod tests;
