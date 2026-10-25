//! Serde-структуры JSON-отчётов, которые читает Kotlin-сторона.
//! Имена полей — часть контракта, менять только вместе с Kotlin.

use serde::Serialize;

/// Универсальная ошибка: `{"ok":false,"stage":"...","error":"..."}`.
#[derive(Debug, Serialize)]
pub struct ErrorReport {
    pub ok: bool,
    pub stage: String,
    pub error: String,
}

impl ErrorReport {
    pub fn new(stage: &str, error: &str) -> Self {
        Self {
            ok: false,
            stage: stage.to_owned(),
            error: error.to_owned(),
        }
    }
}

/// Выходная RAW-конфигурация потока камеры.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct RawStreamConfig {
    pub format: i32,
    pub format_name: String,
    pub width: i32,
    pub height: i32,
    /// `None`, если в `MIN_FRAME_DURATIONS` нет записи для этого режима.
    pub min_frame_duration_ns: Option<i64>,
}

/// Одна камера в каталоге Camera2 NDK.
#[derive(Debug, Serialize)]
pub struct CameraEntry {
    pub id: String,
    /// Был ли id в `ACameraManager_getCameraIdList`.
    pub listed: bool,
    pub characteristics_status: String,
    pub open_status: String,
    pub capabilities: Vec<i32>,
    pub capability_names: Vec<String>,
    pub physical_ids: Vec<String>,
    pub raw_stream_configs: Vec<RawStreamConfig>,
}

/// Результат `ndkCameraCatalog`.
#[derive(Debug, Serialize)]
pub struct CameraCatalogReport {
    pub ok: bool,
    pub ids: Vec<String>,
    pub cameras: Vec<CameraEntry>,
}

/// Успешное создание `AImageReader`.
#[derive(Debug, Serialize)]
pub struct ReaderCreated {
    pub ok: bool,
    pub handle: i64,
    pub media_status: String,
}

/// Неуспешное создание `AImageReader` (штатный отрицательный результат).
#[derive(Debug, Serialize)]
pub struct ReaderCreateFailed {
    pub ok: bool,
    pub media_status: String,
    pub media_status_code: i32,
    pub width: i32,
    pub height: i32,
    pub format: i32,
    pub usage: u64,
    pub max_images: i32,
}

/// Результат `rawReaderDestroy`.
#[derive(Debug, Serialize)]
pub struct ReaderDestroyed {
    pub ok: bool,
}

/// Описание `AHardwareBuffer_Desc`.
#[derive(Debug, Serialize)]
pub struct AhbDescription {
    pub format: u32,
    pub format_name: String,
    pub width: u32,
    pub height: u32,
    pub layers: u32,
    /// Stride в пикселях (как отдаёт `AHardwareBuffer_describe`).
    pub stride: u32,
    pub usage_hex: String,
    pub usage_flags: Vec<String>,
    /// `AImage_getPlaneRowStride(image, 0)` в байтах или текст ошибки.
    pub plane0_row_stride_bytes: Option<i32>,
}

/// Ответ `vkGetAndroidHardwareBufferPropertiesANDROID` + format properties.
#[derive(Debug, Serialize)]
pub struct AhbVulkanProperties {
    pub allocation_size: u64,
    pub memory_type_bits: u32,
    pub format: String,
    pub external_format: u64,
    pub format_features_hex: String,
}

/// Попытка импорта одним из путей.
#[derive(Debug, Serialize)]
pub struct ImportAttempt {
    /// `image` | `image_external_format` | `buffer`.
    pub path: String,
    /// Формат VkImage (для `buffer` — `null`).
    pub vk_format: Option<String>,
    pub ok: bool,
    pub error: Option<String>,
    /// Можно ли на этом пути запустить compute-контрольную сумму.
    pub compute_capable: bool,
}

/// Результат compute-проверки одного кадра.
#[derive(Debug, Serialize)]
pub struct ComputeResult {
    pub path: String,
    pub gpu_checksum: u32,
    pub cpu_checksum: Option<u32>,
    #[serde(rename = "match")]
    pub matches: Option<bool>,
    pub gpu_time_us: f64,
    /// `timestamp_query` или `host_clock` (если очередь не поддерживает timestamp).
    pub gpu_time_source: String,
    pub cpu_checksum_error: Option<String>,
}

/// Отчёт по одному кадру.
#[derive(Debug, Default, Serialize)]
pub struct FrameReport {
    pub index: usize,
    pub ahb: Option<AhbDescription>,
    pub ahb_vulkan: Option<AhbVulkanProperties>,
    pub import_attempts: Vec<ImportAttempt>,
    pub compute: Option<ComputeResult>,
    /// `none` | `imported_as_vk_semaphore` | `cpu_poll_fallback(<причина>)`.
    pub acquire_fence: String,
    /// `sync_fd_via_delete_async` | `delete_after_queue_wait_idle(<причина>)` | `not_released`.
    pub release: String,
    pub error: Option<String>,
}

/// Сводка по прогону.
#[derive(Debug, Serialize)]
pub struct ImportSummary {
    pub frames_received: usize,
    /// `image_r16_uint` | `buffer` | `none`.
    pub import_path: String,
    pub all_checksums_match: bool,
    pub acquire_fence_import: String,
    pub release_fence_export: String,
}

/// Результат `rawReaderRunImportTest`.
#[derive(Debug, Serialize)]
pub struct ImportTestReport {
    pub ok: bool,
    pub frames: Vec<FrameReport>,
    pub summary: ImportSummary,
}

/// Сводка по всем кадрам прогона (чистая функция — тестируется на host).
pub fn build_summary(frames: &[FrameReport]) -> ImportSummary {
    let computed: Vec<&ComputeResult> = frames.iter().filter_map(|frame| frame.compute.as_ref()).collect();
    ImportSummary {
        frames_received: frames.len(),
        import_path: computed
            .first()
            .map_or_else(|| "none".to_owned(), |compute| compute.path.clone()),
        // «Все сошлись» истинно только если GPU реально отработал хотя бы на одном кадре
        // и на каждом кадре есть подтверждённое равенство с CPU.
        all_checksums_match: !frames.is_empty()
            && frames.iter().all(|frame| {
                frame
                    .compute
                    .as_ref()
                    .is_some_and(|compute| compute.matches == Some(true))
            }),
        acquire_fence_import: frames
            .first()
            .map_or_else(|| "n/a (нет кадров)".to_owned(), |frame| frame.acquire_fence.clone()),
        release_fence_export: frames
            .first()
            .map_or_else(|| "n/a (нет кадров)".to_owned(), |frame| frame.release.clone()),
    }
}

/// Наличие нужных Vulkan-расширений: имя → есть ли в списке устройства.
pub fn extension_presence(required: &[&str], available: &[String]) -> serde_json::Map<String, serde_json::Value> {
    required
        .iter()
        .map(|name| {
            let present = available.iter().any(|candidate| candidate == name);
            ((*name).to_owned(), serde_json::Value::Bool(present))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_report_shape() {
        let json = serde_json::to_value(ErrorReport::new("s", "e")).unwrap();
        assert_eq!(json, serde_json::json!({"ok": false, "stage": "s", "error": "e"}));
    }

    #[test]
    fn catalog_shape_matches_contract() {
        let report = CameraCatalogReport {
            ok: true,
            ids: vec!["2".into()],
            cameras: vec![CameraEntry {
                id: "2".into(),
                listed: true,
                characteristics_status: "ACAMERA_OK".into(),
                open_status: "ACAMERA_OK".into(),
                capabilities: vec![0, 3],
                capability_names: vec!["BACKWARD_COMPATIBLE".into(), "RAW".into()],
                physical_ids: vec![],
                raw_stream_configs: vec![RawStreamConfig {
                    format: 32,
                    format_name: "RAW16".into(),
                    width: 4096,
                    height: 3072,
                    min_frame_duration_ns: Some(33_333_333),
                }],
            }],
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["ok"], true);
        assert_eq!(json["cameras"][0]["raw_stream_configs"][0]["format"], 32);
        assert_eq!(
            json["cameras"][0]["raw_stream_configs"][0]["min_frame_duration_ns"],
            33_333_333
        );
        assert_eq!(json["cameras"][0]["open_status"], "ACAMERA_OK");
    }

    #[test]
    fn compute_result_uses_match_key() {
        let result = ComputeResult {
            path: "image_r16_uint".into(),
            gpu_checksum: 5,
            cpu_checksum: Some(5),
            matches: Some(true),
            gpu_time_us: 1.5,
            gpu_time_source: "timestamp_query".into(),
            cpu_checksum_error: None,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["match"], true);
        assert!(json.get("matches").is_none());
    }

    #[test]
    fn extension_presence_marks_missing() {
        let available = vec!["VK_KHR_external_semaphore_fd".to_owned()];
        let map = extension_presence(
            &["VK_KHR_external_semaphore_fd", "VK_EXT_queue_family_foreign"],
            &available,
        );
        assert_eq!(map["VK_KHR_external_semaphore_fd"], true);
        assert_eq!(map["VK_EXT_queue_family_foreign"], false);
    }

    fn frame(path: Option<(&str, Option<bool>)>) -> FrameReport {
        FrameReport {
            acquire_fence: "none".into(),
            release: "sync_fd_via_delete_async".into(),
            compute: path.map(|(path, matches)| ComputeResult {
                path: path.into(),
                gpu_checksum: 1,
                cpu_checksum: Some(1),
                matches,
                gpu_time_us: 1.0,
                gpu_time_source: "timestamp_query".into(),
                cpu_checksum_error: None,
            }),
            ..FrameReport::default()
        }
    }

    #[test]
    fn summary_without_frames_is_none_and_not_matching() {
        let summary = build_summary(&[]);
        assert_eq!(summary.frames_received, 0);
        assert_eq!(summary.import_path, "none");
        assert!(!summary.all_checksums_match);
    }

    #[test]
    fn summary_all_match_requires_every_frame_confirmed() {
        let good = build_summary(&[
            frame(Some(("image_r16_uint", Some(true)))),
            frame(Some(("image_r16_uint", Some(true)))),
        ]);
        assert!(good.all_checksums_match);
        assert_eq!(good.import_path, "image_r16_uint");
        assert_eq!(good.release_fence_export, "sync_fd_via_delete_async");

        let mismatch = build_summary(&[
            frame(Some(("buffer", Some(true)))),
            frame(Some(("buffer", Some(false)))),
        ]);
        assert!(!mismatch.all_checksums_match);

        let unverified = build_summary(&[frame(Some(("buffer", None)))]);
        assert!(
            !unverified.all_checksums_match,
            "без CPU-эталона совпадение не доказано"
        );

        let no_compute = build_summary(&[frame(None)]);
        assert_eq!(no_compute.import_path, "none");
        assert!(!no_compute.all_checksums_match);
    }
}
