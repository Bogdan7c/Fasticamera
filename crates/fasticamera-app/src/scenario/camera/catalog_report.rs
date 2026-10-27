//! Отчёт и вердикт `camera-catalog`: что телефон показал приложению о своих камерах.
//!
//! Форма отчёта — контракт со `scripts/smoke_checks.py` и сверкой с `dumpsys media.camera`
//! (`scripts/smoke_dumpsys.py`): по каждой камере `id`, `facing` и `raw_streams` (формат, размер),
//! список ролей и снимок диагностики бэкенда. Отчёт строится отдельно от вердикта: [`evaluate_catalog`]
//! видит только каталог и возвращает нарушения.

use camera_android::DiagnosticsSnapshot;
use camera_contract::capabilities::{
    CameraDescriptor, FocusCapability, HardwareLevel, LensShading, Optics, RawSupport, ReferenceIlluminant,
    RequestCapability,
};
use camera_contract::catalog::CameraCatalog;
use camera_contract::geometry::{PixelRect, PixelSize};
use camera_contract::ids::{CameraTopology, LensFacing};
use camera_contract::raw_format::{CfaLayout, NativeMosaic};
use camera_core::CameraCounters;
use serde::Serialize;

use super::backend_summary::BackendSummary;
use super::detail::to_value;

/// Один RAW-поток камеры в плоском виде.
#[derive(Debug, Serialize)]
pub struct RawStreamEntry {
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
    pub min_frame_duration_ns: u64,
}

/// RAW-возможности камеры.
#[derive(Debug, Serialize)]
pub struct RawEntry {
    pub supported: bool,
    pub black_level_pattern: Option<[u32; 4]>,
    pub white_level: Option<u32>,
    pub cfa: Option<CfaLayout>,
    pub native_mosaic: Option<NativeMosaic>,
    /// Иллюминанты наборов калибровки (по одному на набор).
    pub calibration_illuminants: Vec<ReferenceIlluminant>,
}

/// Габариты сенсора в миллиметрах.
#[derive(Debug, Serialize)]
pub struct SensorSizeEntry {
    pub width_mm: f32,
    pub height_mm: f32,
}

/// Камера каталога.
#[derive(Debug, Serialize)]
pub struct CameraEntry<'catalog> {
    pub id: String,
    pub facing: LensFacing,
    pub topology: &'catalog CameraTopology,
    /// Роль, которую камера играет в продукте; `None` — роли нет (логическая 0).
    pub role: Option<&'static str>,
    pub hardware_level: HardwareLevel,
    pub request_capabilities: &'catalog [RequestCapability],
    pub pixel_array: PixelSize,
    pub active_array: PixelRect,
    pub raw_streams: Vec<RawStreamEntry>,
    pub ae_fps_ranges: Vec<[u32; 2]>,
    /// Угол обзора по длинной стороне сенсора для самого короткого фокусного, градусы.
    pub widest_field_of_view_degrees: Option<f64>,
    pub apertures_f_number: &'catalog [f32],
    pub focal_lengths_mm: &'catalog [f32],
    pub sensor_physical_size: SensorSizeEntry,
    pub focus: FocusCapability,
    pub raw: RawEntry,
    pub lens_shading: LensShading,
}

/// Привязка роли.
#[derive(Debug, Serialize)]
pub struct RoleEntry {
    pub role: &'static str,
    pub camera: String,
}

/// Сжатая запись RAW-потока для результата.
#[derive(Debug, Serialize)]
pub struct RawStreamBrief {
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
}

/// Камера в результате: то, что сверяют скрипты, и то, что нужно знать сразу.
#[derive(Debug, Serialize)]
pub struct CameraSummary<'catalog> {
    pub id: String,
    pub facing: LensFacing,
    pub topology: &'catalog CameraTopology,
    pub role: Option<&'static str>,
    pub hardware_level: HardwareLevel,
    pub raw_streams: Vec<RawStreamBrief>,
    pub widest_field_of_view_degrees: Option<f64>,
}

/// Отчёт `camera-catalog` (строка результата).
#[derive(Debug, Serialize)]
pub struct CatalogReport<'catalog> {
    pub state: &'static str,
    pub cameras: Vec<CameraSummary<'catalog>>,
    pub roles: Vec<RoleEntry>,
    pub backend: BackendSummary<'catalog>,
    pub engine_counters: CameraCounters,
    /// Событий движка за ожидание каталога (сами события в результат не входят).
    pub event_count: usize,
}

/// Подробности (строки стадий): полные характеристики камер и вся диагностика бэкенда.
#[derive(Debug)]
pub struct CatalogDetails {
    /// Элемент на камеру: полный [`CameraEntry`].
    pub cameras: Vec<serde_json::Value>,
    /// Первый элемент — счётчики и отказы бэкенда, дальше по элементу на камеру (вендорские теги, заметки).
    pub backend: Vec<serde_json::Value>,
}

impl CatalogDetails {
    pub fn build(catalog: &CameraCatalog, backend: &DiagnosticsSnapshot) -> Self {
        let cameras = catalog
            .cameras()
            .iter()
            .map(|descriptor| to_value(&camera_entry(catalog, descriptor)))
            .collect();
        let mut backend_items = vec![serde_json::json!({
            "reader": backend.reader,
            "callback_panics": backend.callback_panics,
            "callbacks_after_close": backend.callbacks_after_close,
            "unmatched_frame_numbers": backend.unmatched_frame_numbers,
            "retained_session_contexts": backend.retained_session_contexts,
            "leaked_availability_contexts": backend.leaked_availability_contexts,
            "skipped_cameras": backend.skipped_cameras,
            "recent_failures": backend.recent_failures,
            "dropped_failures": backend.dropped_failures,
        })];
        backend_items.extend(backend.cameras.iter().map(to_value));
        Self {
            cameras,
            backend: backend_items,
        }
    }
}

fn raw_entry(descriptor: &CameraDescriptor) -> RawEntry {
    match &descriptor.capabilities.raw {
        RawSupport::Supported(info) => RawEntry {
            supported: true,
            black_level_pattern: Some(info.levels.black_level_pattern),
            white_level: Some(info.levels.white_level),
            cfa: Some(info.cfa),
            native_mosaic: Some(info.native_mosaic),
            calibration_illuminants: info.color_calibration.iter().map(|set| set.illuminant).collect(),
        },
        RawSupport::NotSupported => RawEntry {
            supported: false,
            black_level_pattern: None,
            white_level: None,
            cfa: None,
            native_mosaic: None,
            calibration_illuminants: Vec::new(),
        },
    }
}

fn optics_sizes(optics: &Optics) -> SensorSizeEntry {
    SensorSizeEntry {
        width_mm: optics.sensor_physical_size.width_mm,
        height_mm: optics.sensor_physical_size.height_mm,
    }
}

fn camera_entry<'catalog>(
    catalog: &'catalog CameraCatalog,
    descriptor: &'catalog CameraDescriptor,
) -> CameraEntry<'catalog> {
    let capabilities = &descriptor.capabilities;
    CameraEntry {
        id: descriptor.id.to_string(),
        facing: capabilities.facing,
        topology: &capabilities.topology,
        role: catalog.role_of(&descriptor.id).map(|role| role.name()),
        hardware_level: capabilities.hardware_level,
        request_capabilities: &capabilities.request_capabilities,
        pixel_array: capabilities.pixel_array,
        active_array: capabilities.active_array,
        raw_streams: capabilities
            .raw_streams
            .iter()
            .map(|option| RawStreamEntry {
                format: option.format.name(),
                width: option.size.width,
                height: option.size.height,
                min_frame_duration_ns: option.min_frame_duration.as_nanos(),
            })
            .collect(),
        ae_fps_ranges: capabilities
            .ae_fps_ranges
            .iter()
            .map(|range| [range.min, range.max])
            .collect(),
        widest_field_of_view_degrees: capabilities.optics.widest_field_of_view_degrees(),
        apertures_f_number: &capabilities.optics.apertures_f_number,
        focal_lengths_mm: &capabilities.optics.focal_lengths_mm,
        sensor_physical_size: optics_sizes(&capabilities.optics),
        focus: capabilities.focus,
        raw: raw_entry(descriptor),
        lens_shading: capabilities.vendor_processing.lens_shading,
    }
}

fn camera_summary<'catalog>(
    catalog: &'catalog CameraCatalog,
    descriptor: &'catalog CameraDescriptor,
) -> CameraSummary<'catalog> {
    let capabilities = &descriptor.capabilities;
    CameraSummary {
        id: descriptor.id.to_string(),
        facing: capabilities.facing,
        topology: &capabilities.topology,
        role: catalog.role_of(&descriptor.id).map(|role| role.name()),
        hardware_level: capabilities.hardware_level,
        raw_streams: capabilities
            .raw_streams
            .iter()
            .map(|option| RawStreamBrief {
                format: option.format.name(),
                width: option.size.width,
                height: option.size.height,
            })
            .collect(),
        widest_field_of_view_degrees: capabilities.optics.widest_field_of_view_degrees(),
    }
}

impl<'catalog> CatalogReport<'catalog> {
    pub fn build(
        state: &'static str,
        catalog: &'catalog CameraCatalog,
        backend: &'catalog DiagnosticsSnapshot,
        engine_counters: CameraCounters,
        event_count: usize,
    ) -> Self {
        Self {
            state,
            cameras: catalog
                .cameras()
                .iter()
                .map(|descriptor| camera_summary(catalog, descriptor))
                .collect(),
            roles: catalog
                .roles()
                .iter()
                .map(|binding| RoleEntry {
                    role: binding.role.name(),
                    camera: binding.camera.to_string(),
                })
                .collect(),
            backend: BackendSummary::of(backend),
            engine_counters,
            event_count,
        }
    }
}

/// Судит каталог: нарушения правил (пусто — каталог пригоден).
///
/// * каталог не пуст;
/// * каждая роль привязана к камере, которая есть в каталоге;
/// * каждая камера с возможностью `RAW` предлагает хотя бы один RAW-поток.
pub fn evaluate_catalog(catalog: &CameraCatalog) -> Vec<String> {
    let mut violations = Vec::new();
    if catalog.cameras().is_empty() {
        violations.push("каталог пуст: приложению не видна ни одна камера".to_owned());
    }
    for binding in catalog.roles() {
        if catalog.camera(&binding.camera).is_none() {
            violations.push(format!(
                "роль {} привязана к камере {}, которой нет в каталоге",
                binding.role.name(),
                binding.camera
            ));
        }
    }
    for descriptor in catalog.cameras() {
        let capabilities = &descriptor.capabilities;
        let claims_raw = capabilities.request_capabilities.contains(&RequestCapability::Raw);
        if claims_raw && capabilities.raw_streams.is_empty() {
            violations.push(format!(
                "камера {} заявляет возможность RAW, но не предлагает ни одного RAW-потока",
                descriptor.id
            ));
        }
    }
    violations
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
