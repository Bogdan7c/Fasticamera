//! Чистое построение записей запроса Camera2: что положить в повторяющийся запрос и в параметры
//! сессии. Слой NDK только применяет готовый план — вся логика проверяется на ПК.
//!
//! Что берётся из зонда сессии 00 (`CameraSessionRunner`/`StreamExperiment`):
//! * диапазон AE ставится **и в параметры сессии, и в каждый запрос** (это сессионный ключ, по нему
//!   HAL выбирает режим сенсора);
//! * вендорские ключи CamX — **только в запрос** (в таблице 00 попытка 4 шла `req_keys`, без
//!   `sess_keys`); параметров сессии у такого потока нет вовсе;
//! * ручная экспозиция: `AE_MODE=OFF`, выдержка, ISO и длительность кадра равная частоте потока.

use camera_contract::capabilities::FocusCapability;
use camera_contract::control::{CaptureIntent, ExposureControl, FocusControl, ManualControl};
use camera_contract::stream::{FrameRateMechanism, TargetFrameRate};

use crate::entry::EntryValue;
use crate::tags::{self, Tag};
use crate::vendor::{RequestEntryWriter, VendorCapabilities, VendorKey, VendorValue, VendorWriteError};

/// Шаблон запроса Camera2 (`ACameraDevice_request_template`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestTemplate {
    Preview,
    Record,
}

impl RequestTemplate {
    /// Значение для `ACameraDevice_createCaptureRequest`.
    pub fn ndk_value(self) -> u32 {
        match self {
            RequestTemplate::Preview => tags::TEMPLATE_PREVIEW_VALUE,
            RequestTemplate::Record => tags::TEMPLATE_RECORD_VALUE,
        }
    }
}

impl From<CaptureIntent> for RequestTemplate {
    fn from(intent: CaptureIntent) -> Self {
        match intent {
            CaptureIntent::Preview => RequestTemplate::Preview,
            CaptureIntent::VideoRecord => RequestTemplate::Record,
        }
    }
}

/// Одна запись запроса: публичный тег NDK или вендорский ключ (номер выдаст реестр).
#[derive(Debug, Clone, PartialEq)]
pub enum PlannedEntry {
    Public { tag: Tag, value: EntryValue },
    Vendor { key: VendorKey, value: VendorValue },
}

/// План запроса: шаблон и записи.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestPlan {
    pub template: RequestTemplate,
    pub entries: Vec<PlannedEntry>,
}

/// Откуда взялись параметры запроса.
#[derive(Debug, Clone, Copy)]
pub struct RequestInputs<'a> {
    pub intent: CaptureIntent,
    pub controls: &'a ManualControl,
    pub frame_rate: TargetFrameRate,
    pub mechanism: FrameRateMechanism,
    pub focus: FocusCapability,
}

/// Строит повторяющийся запрос.
pub fn plan_repeating_request(inputs: &RequestInputs<'_>) -> RequestPlan {
    let mut entries = frame_rate_entries(inputs.mechanism);
    entries.extend(exposure_entries(inputs.controls.exposure, inputs.frame_rate));
    entries.extend(focus_entries(inputs.intent, inputs.controls.focus, inputs.focus));
    RequestPlan {
        template: inputs.intent.into(),
        entries,
    }
}

/// Параметры сессии: только публичные ключи рычага частоты. У вендорского рычага их нет.
pub fn plan_session_parameters(intent: CaptureIntent, mechanism: FrameRateMechanism) -> Option<RequestPlan> {
    let entries: Vec<PlannedEntry> = frame_rate_entries(mechanism)
        .into_iter()
        .filter(|entry| matches!(entry, PlannedEntry::Public { .. }))
        .collect();
    (!entries.is_empty()).then_some(RequestPlan {
        template: intent.into(),
        entries,
    })
}

/// Записи рычага частоты (`HalDefault` — пусто).
fn frame_rate_entries(mechanism: FrameRateMechanism) -> Vec<PlannedEntry> {
    match mechanism {
        FrameRateMechanism::HalDefault => Vec::new(),
        FrameRateMechanism::PublicAeTargetRange { range } => vec![PlannedEntry::Public {
            tag: tags::AE_TARGET_FPS_RANGE,
            value: EntryValue::Int32s(vec![saturating_i32(range.min), saturating_i32(range.max)]),
        }],
        FrameRateMechanism::VendorSensorFrameDurationOverride { frame_duration } => vec![
            PlannedEntry::Vendor {
                key: VendorKey::DisableFpsLimits,
                value: VendorValue::Byte(1),
            },
            PlannedEntry::Vendor {
                key: VendorKey::OverrideSensorFrameDuration,
                value: VendorValue::Int64(saturating_i64(frame_duration.as_nanos())),
            },
        ],
    }
}

fn exposure_entries(exposure: ExposureControl, frame_rate: TargetFrameRate) -> Vec<PlannedEntry> {
    match exposure {
        // Автомат вендора: шаблон уже включает AE, лишнего не пишем.
        ExposureControl::Auto => Vec::new(),
        ExposureControl::Manual { exposure_time, iso } => {
            let exposure_nanos = saturating_i64(u64::try_from(exposure_time.as_nanos()).unwrap_or(u64::MAX));
            // Кадр не может быть короче выдержки: HAL ограничит сам, но мы просим честно.
            let frame_duration_nanos = saturating_i64(frame_rate.frame_duration().as_nanos()).max(exposure_nanos);
            vec![
                PlannedEntry::Public {
                    tag: tags::AE_MODE,
                    value: EntryValue::Bytes(vec![tags::AE_MODE_OFF]),
                },
                PlannedEntry::Public {
                    tag: tags::SENSOR_EXPOSURE_TIME,
                    value: EntryValue::Int64s(vec![exposure_nanos]),
                },
                PlannedEntry::Public {
                    tag: tags::SENSOR_SENSITIVITY,
                    value: EntryValue::Int32s(vec![saturating_i32(iso)]),
                },
                PlannedEntry::Public {
                    tag: tags::SENSOR_FRAME_DURATION,
                    value: EntryValue::Int64s(vec![frame_duration_nanos]),
                },
            ]
        }
    }
}

fn focus_entries(intent: CaptureIntent, control: FocusControl, capability: FocusCapability) -> Vec<PlannedEntry> {
    // Фиксированный фокус (фронт): режимы AF бессмысленны и могут отвергаться — не пишем ничего.
    if capability == FocusCapability::Fixed {
        return Vec::new();
    }
    match control {
        FocusControl::ContinuousAuto => {
            let mode = match intent {
                CaptureIntent::Preview => tags::AF_MODE_CONTINUOUS_PICTURE,
                CaptureIntent::VideoRecord => tags::AF_MODE_CONTINUOUS_VIDEO,
            };
            vec![PlannedEntry::Public {
                tag: tags::AF_MODE,
                value: EntryValue::Bytes(vec![mode]),
            }]
        }
        FocusControl::Manual { distance_diopters } => vec![
            PlannedEntry::Public {
                tag: tags::AF_MODE,
                value: EntryValue::Bytes(vec![tags::AF_MODE_OFF]),
            },
            PlannedEntry::Public {
                tag: tags::LENS_FOCUS_DISTANCE,
                value: EntryValue::Floats(vec![distance_diopters]),
            },
        ],
    }
}

fn saturating_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn saturating_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

/// Почему план не удалось применить к запросу.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ApplyError {
    #[error(transparent)]
    Vendor(#[from] VendorWriteError),
    #[error("запрос отверг тег {tag}, код {status}")]
    PublicRejected { tag: &'static str, status: i32 },
}

impl ApplyError {
    /// Код `camera_status_t`, если отказ пришёл от платформы; 0 — отказ принят до обращения к ней.
    pub fn platform_status(&self) -> i32 {
        match self {
            ApplyError::PublicRejected { status, .. }
            | ApplyError::Vendor(VendorWriteError::Rejected { status, .. }) => *status,
            ApplyError::Vendor(_) => 0,
        }
    }
}

/// Применяет записи к запросу. На первой ошибке останавливается и возвращает её:
/// частично заполненный запрос вызывающий не отправляет.
pub fn apply_entries(
    entries: &[PlannedEntry],
    vendor: &VendorCapabilities,
    writer: &mut dyn RequestEntryWriter,
) -> Result<(), ApplyError> {
    for entry in entries {
        match entry {
            PlannedEntry::Public { tag, value } => {
                writer
                    .write_entry(tag.code, value)
                    .map_err(|status| ApplyError::PublicRejected { tag: tag.name, status })?;
            }
            PlannedEntry::Vendor { key, value } => vendor.write_value(*key, *value, writer)?,
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "request_plan_tests.rs"]
mod tests;
