//! Сбор отчёта `ndkCameraCatalog`: список камер, RAW-режимы, статусы открытия.

use ndk_sys::acamera_metadata_tag as Tag;

use super::manager::{CameraManager, StatusCode};
use crate::camera_metadata::build_raw_stream_configs;
use crate::error::ProbeError;
use crate::report::{CameraCatalogReport, CameraEntry};
use crate::status_names::{camera_status_name, capability_name};

/// Id, которые пробуем даже если их нет в списке (скрытые/системные камеры).
const EXTRA_PROBE_IDS: std::ops::RangeInclusive<u32> = 0..=7;

/// Читает характеристики и пробует открыть одну камеру.
fn describe_camera(manager: &CameraManager, id: &str, listed: bool) -> CameraEntry {
    let (characteristics_status, capabilities, physical_ids, raw_stream_configs) = match manager.characteristics(id) {
        Ok(metadata) => {
            let capabilities = metadata.i32_values(Tag::ACAMERA_REQUEST_AVAILABLE_CAPABILITIES.0);
            let physical = crate::camera_metadata::parse_physical_ids(
                &metadata.byte_values(Tag::ACAMERA_LOGICAL_MULTI_CAMERA_PHYSICAL_IDS.0),
            );
            let raw_configs = build_raw_stream_configs(
                &metadata.i32_values(Tag::ACAMERA_SCALER_AVAILABLE_STREAM_CONFIGURATIONS.0),
                &metadata.i64_values(Tag::ACAMERA_SCALER_AVAILABLE_MIN_FRAME_DURATIONS.0),
            );
            (camera_status_name(0), capabilities, physical, raw_configs)
        }
        Err(status) => (camera_status_name(status), Vec::new(), Vec::new(), Vec::new()),
    };
    let open_status: StatusCode = manager.probe_open(id);
    CameraEntry {
        id: id.to_owned(),
        listed,
        characteristics_status,
        open_status: camera_status_name(open_status),
        capability_names: capabilities.iter().map(|code| capability_name(*code)).collect(),
        capabilities,
        physical_ids,
        raw_stream_configs,
    }
}

/// Собирает полный каталог: сначала перечисленные id, затем непроверенные из "0".."7".
fn collect_cameras(manager: &CameraManager, listed_ids: &[String]) -> Vec<CameraEntry> {
    let mut cameras: Vec<CameraEntry> = listed_ids.iter().map(|id| describe_camera(manager, id, true)).collect();
    for numeric_id in EXTRA_PROBE_IDS {
        let id = numeric_id.to_string();
        if !listed_ids.contains(&id) {
            cameras.push(describe_camera(manager, &id, false));
        }
    }
    cameras
}

/// JSON-каталог камер. Ошибка `ProbeError` — только если нет самого менеджера или списка id.
pub fn build_catalog_json() -> Result<String, ProbeError> {
    let manager = CameraManager::create().ok_or_else(|| ProbeError::platform("ACameraManager_create вернул null"))?;
    let ids = manager
        .list_ids()
        .map_err(|status| ProbeError::platform(format!("getCameraIdList: {}", camera_status_name(status))))?;
    let cameras = collect_cameras(&manager, &ids);
    let report = CameraCatalogReport { ok: true, ids, cameras };
    Ok(serde_json::to_string(&report)?)
}
