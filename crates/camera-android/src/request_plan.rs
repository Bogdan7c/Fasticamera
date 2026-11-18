//! Чистое построение записей запроса Camera2: что положить в повторяющийся запрос и в параметры
//! сессии. Слой NDK только применяет готовый план — вся логика проверяется на ПК.
//!
//! Что берётся из зонда сессии 00 (`CameraSessionRunner`/`StreamExperiment`):
//! * диапазон AE ставится **и в параметры сессии, и в каждый запрос** (это сессионный ключ, по нему
//!   HAL выбирает режим сенсора);
//! * вендорские ключи CamX — **только в запрос** (в таблице 00 попытка 4 шла `req_keys`, без
//!   `sess_keys`); параметров сессии у такого потока нет вовсе;
//! * ручная экспозиция: `AE_MODE=OFF`, выдержка, ISO и длительность кадра не короче частоты потока.
//!
//! С сессии 08 управляющую часть (экспозиция, фокус, регионы, триггер) задаёт `RequestControls`
//! движка; её перевод в теги живёт в `request_controls_plan`, а здесь — то, что общее для **любого**
//! запроса: шаблон, рычаг частоты кадров, вендорские ключи потока.

use camera_contract::capabilities::CameraCapabilities;
use camera_contract::control::CaptureIntent;
use camera_contract::stream::{FrameRateMechanism, TargetFrameRate};
use camera_contract::three_a::{ExposureBracket, MainsFrequency};
use camera_core::request_controls::{AfTrigger, ExposurePlan, RequestControls};

use crate::entry::EntryValue;
use crate::request_controls_plan::{ControlsContext, PlanNote, controls_entries};
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
    pub controls: &'a RequestControls,
    /// Разовое действие AF: ставится ровно в один запрос (триггер тапа); у остальных `None`.
    pub trigger: Option<AfTrigger>,
    pub frame_rate: TargetFrameRate,
    pub mechanism: FrameRateMechanism,
    /// Возможности камеры (фокус, регионы, режимы 3A, активная область).
    pub camera: &'a CameraCapabilities,
    pub mains: MainsFrequency,
}

/// Готовый запрос и то, что из желаемого записать не удалось.
#[derive(Debug, Clone, PartialEq)]
pub struct PlannedRequest {
    pub plan: RequestPlan,
    pub notes: Vec<PlanNote>,
}

/// Строит один запрос любого вида (повторяющийся, триггер): частота кадров и вендорские ключи
/// потока идут **первыми и всегда** — ключи CamX «липкие» в HAL (зонд 08), а разовый запрос без
/// них сменил бы режим сенсора; затем управляющая часть из плана движка.
pub fn plan_request(inputs: &RequestInputs<'_>) -> PlannedRequest {
    let mut entries = frame_rate_entries(inputs.mechanism);
    let controls = controls_entries(
        &ControlsContext {
            intent: inputs.intent,
            camera: inputs.camera,
            mains: inputs.mains,
            frame_rate: inputs.frame_rate,
        },
        inputs.controls,
        inputs.trigger,
    );
    entries.extend(controls.entries);
    PlannedRequest {
        plan: RequestPlan {
            template: inputs.intent.into(),
            entries,
        },
        notes: controls.notes,
    }
}

/// Строит запросы серии: кадр `i` — базовый план с экспозицией `bracket.frames()[i]` (сенсор
/// вручную), фокус и всё остальное — из `inputs`; триггера в серии нет. Каждый запрос идёт тем же
/// путём [`plan_request`], поэтому рычаг частоты и вендорские ключи есть в каждом.
pub fn plan_bracket(inputs: &RequestInputs<'_>, bracket: &ExposureBracket) -> Vec<PlannedRequest> {
    bracket
        .frames()
        .iter()
        .map(|frame| {
            let controls = RequestControls {
                exposure: ExposurePlan::Sensor {
                    exposure_time: frame.exposure_time,
                    iso: frame.iso,
                },
                focus: inputs.controls.focus,
            };
            plan_request(&RequestInputs {
                controls: &controls,
                trigger: None,
                ..*inputs
            })
        })
        .collect()
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

#[cfg(test)]
#[path = "request_plan_oneshot_tests.rs"]
mod oneshot_tests;
