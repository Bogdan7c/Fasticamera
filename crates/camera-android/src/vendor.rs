//! Реестр вендорских тегов `VendorCapabilities` (решения 22, 23; инвариант 7 README).
//!
//! Вендорские теги CamX не входят в NDK: их номера выдаёт устройство по имени. Реестр:
//! * хранит таблицу спецификаций «имя → ожидаемый тип → где разрешён» ([`VENDOR_TAG_SPECS`]);
//! * разрешает имена через [`TagResolver`] (на Android — `ACameraMetadata_getTagFromName` по
//!   характеристикам камеры, на ПК — фейк);
//! * пишет значения в запрос через [`RequestEntryWriter`] (на Android — `ACaptureRequest_setEntry_*`).
//!
//! Исходы: тег не найден — [`VendorSupport::Unsupported`] (не паника); значение не того типа —
//! [`VendorWriteError::TypeMismatch`]; камера вне списка проверенных — `Unsupported` с причиной
//! «не разрешено для камеры» **и без обращения к HAL за номером тега**; запись одного ключа
//! не трогает другие ключи запроса. Имена тегов наружу не уходят (только смысл рычага).

use camera_contract::ids::CameraId;
use serde::Serialize;

use crate::config::AndroidCameraConfig;
use crate::entry::EntryValue;
use crate::sensor_modes::SENSOR_MODE_TABLE_TAG_NAME;

/// Вендорский ключ, который знает бэкенд (по смыслу, не по имени тега).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VendorKey {
    /// CamX `manualExposure.disableFPSLimits`: снять потолок частоты (без HFR-сессии).
    DisableFpsLimits,
    /// CamX `manualExposure.overrideSensorFrameDuration`: длительность кадра сенсора, нс.
    OverrideSensorFrameDuration,
    /// Qualcomm `is_qcfa_sensor`: сенсор Quad Bayer (характеристика, не запрос).
    IsQcfaSensor,
    /// Qualcomm `SensorModeTable`: режимы сенсора «размер + макс. частота» (характеристика; разбор — `sensor_modes`).
    SensorModeTable,
}

/// Тип значения вендорского тега (из дампа `dumpsys media.camera`, 00 п. 114).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VendorValueKind {
    Byte,
    Int32,
    Int64,
}

/// Значение вендорского тега — типизировано, чтобы не перепутать `byte` и `int64` на записи.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorValue {
    Byte(u8),
    Int32(i32),
    Int64(i64),
}

impl VendorValue {
    pub fn kind(self) -> VendorValueKind {
        match self {
            VendorValue::Byte(_) => VendorValueKind::Byte,
            VendorValue::Int32(_) => VendorValueKind::Int32,
            VendorValue::Int64(_) => VendorValueKind::Int64,
        }
    }

    /// Запись метаданных из одного элемента.
    pub fn to_entry(self) -> EntryValue {
        match self {
            VendorValue::Byte(value) => EntryValue::Bytes(vec![value]),
            VendorValue::Int32(value) => EntryValue::Int32s(vec![value]),
            VendorValue::Int64(value) => EntryValue::Int64s(vec![value]),
        }
    }
}

/// Где вендорский ключ разрешён.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VendorAccess {
    /// Запись в запрос — только на камерах из `vendor_frame_duration_override_cameras`
    /// (на ширике ломает HAL, 00 п. 17).
    RequestOnVerifiedCamerasOnly,
    /// Чтение статической характеристики — на любой камере (чтение ничего не ломает).
    CharacteristicOnAnyCamera,
}

/// Спецификация вендорского тега.
#[derive(Debug, Clone, Copy)]
pub struct VendorTagSpec {
    pub key: VendorKey,
    /// Имя тега, как в `dumpsys media.camera` (раздел «Vendor tags»).
    pub name: &'static str,
    pub kind: VendorValueKind,
    pub access: VendorAccess,
}

/// Все вендорские теги, которые использует бэкенд.
pub const VENDOR_TAG_SPECS: [VendorTagSpec; 4] = [
    VendorTagSpec {
        key: VendorKey::DisableFpsLimits,
        name: "org.quic.camera.manualExposure.disableFPSLimits",
        kind: VendorValueKind::Byte,
        access: VendorAccess::RequestOnVerifiedCamerasOnly,
    },
    VendorTagSpec {
        key: VendorKey::OverrideSensorFrameDuration,
        name: "org.quic.camera.manualExposure.overrideSensorFrameDuration",
        kind: VendorValueKind::Int64,
        access: VendorAccess::RequestOnVerifiedCamerasOnly,
    },
    VendorTagSpec {
        key: VendorKey::IsQcfaSensor,
        // Имя по справочнику Qualcomm CamX; в отчётах 00 записано только как `is_qcfa_sensor=1`,
        // точное имя не сверено с устройством. Не найдено — родная мозаика честно «неизвестна».
        name: "org.codeaurora.qcamera3.quadra_cfa.is_qcfa_sensor",
        kind: VendorValueKind::Byte,
        access: VendorAccess::CharacteristicOnAnyCamera,
    },
    VendorTagSpec {
        key: VendorKey::SensorModeTable,
        name: SENSOR_MODE_TABLE_TAG_NAME,
        kind: VendorValueKind::Int32,
        access: VendorAccess::CharacteristicOnAnyCamera,
    },
];

fn spec_of(key: VendorKey) -> Option<&'static VendorTagSpec> {
    VENDOR_TAG_SPECS.iter().find(|spec| spec.key == key)
}

/// Итог разрешения имени тега устройством.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagResolution {
    Found {
        tag: u32,
    },
    /// Устройство такого тега не знает.
    NotFound,
    /// Вызов не удался (`camera_status_t` кроме «не найдено»).
    Failed {
        status: i32,
    },
}

/// Разрешает имена вендорских тегов для одной камеры.
pub trait TagResolver {
    fn resolve_tag(&self, name: &str) -> TagResolution;
}

/// Почему вендорский ключ недоступен.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "reason")]
pub enum VendorUnsupportedReason {
    /// Камера не в списке проверенных; номер тега у HAL даже не запрашивался.
    NotAllowedForCamera,
    /// Устройство не знает такого тега.
    TagNotFound,
    /// Запрос номера тега завершился ошибкой.
    ResolveFailed { status: i32 },
}

/// Доступность одного вендорского ключа на камере.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum VendorSupport {
    Available { tag: u32 },
    Unsupported(VendorUnsupportedReason),
}

/// Доступен ли вендорский рычаг частоты (пара ключей) на камере.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDurationOverrideSupport {
    Available,
    /// Камера вне списка проверенных (ширик, фронт, логическая).
    NotAllowedForCamera,
    /// Камера проверена, но устройство не знает нужных тегов (другая прошивка).
    TagsMissing,
}

/// Куда записывается значение запроса. На Android — `ACaptureRequest`, на ПК — фейк-карта.
pub trait RequestEntryWriter {
    /// Записать запись по числовому тегу; `Err(status)` — код `camera_status_t`.
    fn write_entry(&mut self, tag: u32, value: &EntryValue) -> Result<(), i32>;
}

/// Почему запись вендорского ключа не удалась.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VendorWriteError {
    #[error("вендорский ключ {key:?} недоступен: {reason:?}")]
    Unsupported {
        key: VendorKey,
        reason: VendorUnsupportedReason,
    },
    #[error("вендорский ключ {key:?} ждёт {expected:?}, а записывают {actual:?}")]
    TypeMismatch {
        key: VendorKey,
        expected: VendorValueKind,
        actual: VendorValueKind,
    },
    #[error("запрос отверг вендорский ключ {key:?}, код {status}")]
    Rejected { key: VendorKey, status: i32 },
}

/// Сведения о разрешённом теге для диагностики.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VendorTagReport {
    pub key: VendorKey,
    pub name: &'static str,
    #[serde(flatten)]
    pub support: VendorSupport,
}

/// Вендорские возможности одной камеры, разрешённые один раз при чтении каталога.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VendorCapabilities {
    camera: CameraId,
    resolved: Vec<(VendorKey, VendorSupport)>,
}

impl VendorCapabilities {
    /// Разрешает все теги спецификации для камеры.
    ///
    /// Ключи запроса на камерах вне списка проверенных **не разрешаются вовсе**:
    /// лишний вызов к HAL не нужен, а решение «не трогать» не должно зависеть от прошивки.
    pub fn resolve(camera: &CameraId, config: &AndroidCameraConfig, resolver: &dyn TagResolver) -> Self {
        let verified = config.allows_vendor_frame_duration_override(camera);
        let resolved = VENDOR_TAG_SPECS
            .iter()
            .map(|spec| {
                let support = match spec.access {
                    VendorAccess::RequestOnVerifiedCamerasOnly if !verified => {
                        VendorSupport::Unsupported(VendorUnsupportedReason::NotAllowedForCamera)
                    }
                    VendorAccess::RequestOnVerifiedCamerasOnly | VendorAccess::CharacteristicOnAnyCamera => {
                        match resolver.resolve_tag(spec.name) {
                            TagResolution::Found { tag } => VendorSupport::Available { tag },
                            TagResolution::NotFound => VendorSupport::Unsupported(VendorUnsupportedReason::TagNotFound),
                            TagResolution::Failed { status } => {
                                VendorSupport::Unsupported(VendorUnsupportedReason::ResolveFailed { status })
                            }
                        }
                    }
                };
                (spec.key, support)
            })
            .collect();
        Self {
            camera: camera.clone(),
            resolved,
        }
    }

    pub fn camera(&self) -> &CameraId {
        &self.camera
    }

    pub fn support(&self, key: VendorKey) -> VendorSupport {
        self.resolved
            .iter()
            .find(|(candidate, _)| *candidate == key)
            .map(|(_, support)| *support)
            .unwrap_or(VendorSupport::Unsupported(VendorUnsupportedReason::TagNotFound))
    }

    /// Доступна ли пара ключей CamX «снять потолок частоты + длительность кадра».
    pub fn frame_duration_override_support(&self) -> FrameDurationOverrideSupport {
        let supports = [
            self.support(VendorKey::DisableFpsLimits),
            self.support(VendorKey::OverrideSensorFrameDuration),
        ];
        if supports
            .iter()
            .all(|support| matches!(support, VendorSupport::Available { .. }))
        {
            return FrameDurationOverrideSupport::Available;
        }
        if supports.contains(&VendorSupport::Unsupported(
            VendorUnsupportedReason::NotAllowedForCamera,
        )) {
            return FrameDurationOverrideSupport::NotAllowedForCamera;
        }
        FrameDurationOverrideSupport::TagsMissing
    }

    /// Номер тега для чтения характеристики (если разрешена и найдена).
    pub fn characteristic_tag(&self, key: VendorKey) -> Option<u32> {
        match self.support(key) {
            VendorSupport::Available { tag } => Some(tag),
            VendorSupport::Unsupported(_) => None,
        }
    }

    /// Записывает значение вендорского ключа в запрос. Трогает только запись этого ключа.
    pub fn write_value(
        &self,
        key: VendorKey,
        value: VendorValue,
        writer: &mut dyn RequestEntryWriter,
    ) -> Result<(), VendorWriteError> {
        let tag = match self.support(key) {
            VendorSupport::Available { tag } => tag,
            VendorSupport::Unsupported(reason) => return Err(VendorWriteError::Unsupported { key, reason }),
        };
        if let Some(spec) = spec_of(key).filter(|spec| spec.kind != value.kind()) {
            return Err(VendorWriteError::TypeMismatch {
                key,
                expected: spec.kind,
                actual: value.kind(),
            });
        }
        writer
            .write_entry(tag, &value.to_entry())
            .map_err(|status| VendorWriteError::Rejected { key, status })
    }

    /// Отчёт для диагностики: какие теги разрешены и под какими номерами.
    pub fn report(&self) -> Vec<VendorTagReport> {
        self.resolved
            .iter()
            .filter_map(|(key, support)| {
                spec_of(*key).map(|spec| VendorTagReport {
                    key: *key,
                    name: spec.name,
                    support: *support,
                })
            })
            .collect()
    }
}

#[cfg(test)]
#[path = "vendor_tests.rs"]
mod tests;
