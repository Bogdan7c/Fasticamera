//! Коды NDK → смысловые отказы бэкенда (правило 6 `AGENTS.md`: вытеснение, запрет, отказ HAL
//! и сбой платформы не сливаются в один общий код).
//!
//! * `camera_status_t` от `openCamera` → [`OpenFailure`];
//! * `camera_status_t` от создания сессии → [`ConfigureFailure`];
//! * `camera_status_t` от запроса → [`RequestFailure`];
//! * код `onError` устройства → [`DeviceErrorKind`];
//! * причина `ACameraCaptureFailure` → [`CaptureFailureReason`].
//!
//! Неизвестный код не теряется: он едет в [`BackendFailure`] (операция, код, имя) или в
//! `DeviceErrorKind::Unknown(code)`.

use camera_contract::availability::CameraUnavailableReason;
use camera_core::backend::{
    BackendFailure, CaptureFailureReason, ConfigureFailure, DeviceErrorKind, OpenFailure, RequestFailure,
};

use crate::tags;

/// Имя кода `camera_status_t` для сообщений (код вне таблицы — `UNKNOWN_STATUS`).
pub fn camera_status_name(status: i32) -> &'static str {
    match status {
        tags::STATUS_OK => "ACAMERA_OK",
        tags::STATUS_UNKNOWN => "ACAMERA_ERROR_UNKNOWN",
        tags::STATUS_INVALID_PARAMETER => "ACAMERA_ERROR_INVALID_PARAMETER",
        tags::STATUS_CAMERA_DISCONNECTED => "ACAMERA_ERROR_CAMERA_DISCONNECTED",
        tags::STATUS_NOT_ENOUGH_MEMORY => "ACAMERA_ERROR_NOT_ENOUGH_MEMORY",
        tags::STATUS_METADATA_NOT_FOUND => "ACAMERA_ERROR_METADATA_NOT_FOUND",
        tags::STATUS_CAMERA_DEVICE => "ACAMERA_ERROR_CAMERA_DEVICE",
        tags::STATUS_CAMERA_SERVICE => "ACAMERA_ERROR_CAMERA_SERVICE",
        tags::STATUS_SESSION_CLOSED => "ACAMERA_ERROR_SESSION_CLOSED",
        tags::STATUS_INVALID_OPERATION => "ACAMERA_ERROR_INVALID_OPERATION",
        tags::STATUS_STREAM_CONFIGURE_FAIL => "ACAMERA_ERROR_STREAM_CONFIGURE_FAIL",
        tags::STATUS_CAMERA_IN_USE => "ACAMERA_ERROR_CAMERA_IN_USE",
        tags::STATUS_MAX_CAMERA_IN_USE => "ACAMERA_ERROR_MAX_CAMERA_IN_USE",
        tags::STATUS_CAMERA_DISABLED => "ACAMERA_ERROR_CAMERA_DISABLED",
        tags::STATUS_PERMISSION_DENIED => "ACAMERA_ERROR_PERMISSION_DENIED",
        tags::STATUS_UNSUPPORTED_OPERATION => "ACAMERA_ERROR_UNSUPPORTED_OPERATION",
        _ => "UNKNOWN_STATUS",
    }
}

/// Имя кода `media_status_t` (`AImageReader`) для сообщений.
pub fn media_status_name(status: i32) -> &'static str {
    match status {
        tags::MEDIA_STATUS_OK => "AMEDIA_OK",
        tags::MEDIA_STATUS_NO_BUFFER_AVAILABLE => "AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE",
        tags::MEDIA_STATUS_MAX_IMAGES_ACQUIRED => "AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED",
        -10000 => "AMEDIA_ERROR_UNKNOWN",
        -10004 => "AMEDIA_ERROR_INVALID_PARAMETER",
        -10005 => "AMEDIA_ERROR_INVALID_OPERATION",
        _ => "UNKNOWN_MEDIA_STATUS",
    }
}

/// Сбой вызова камеры без смыслового варианта: операция, код и имя кода.
pub fn camera_backend_failure(operation: &'static str, status: i32) -> BackendFailure {
    BackendFailure {
        operation,
        code: status,
        detail: camera_status_name(status).to_owned(),
    }
}

/// Сбой вызова `AImageReader`/`AImage`.
pub fn media_backend_failure(operation: &'static str, status: i32) -> BackendFailure {
    BackendFailure {
        operation,
        code: status,
        detail: media_status_name(status).to_owned(),
    }
}

/// Отказ `ACameraManager_openCamera`.
pub fn open_failure(operation: &'static str, status: i32) -> OpenFailure {
    let unavailable = |reason| OpenFailure::Unavailable { reason };
    match status {
        tags::STATUS_PERMISSION_DENIED => unavailable(CameraUnavailableReason::PermissionDenied),
        tags::STATUS_CAMERA_IN_USE => unavailable(CameraUnavailableReason::InUseByAnotherClient),
        tags::STATUS_MAX_CAMERA_IN_USE => unavailable(CameraUnavailableReason::TooManyCamerasOpen),
        tags::STATUS_CAMERA_DISABLED => unavailable(CameraUnavailableReason::DisabledByPolicy),
        // Соединение со службой камеры оборвано при открытии: камеру отняли — лечится ожиданием её освобождения.
        tags::STATUS_CAMERA_DISCONNECTED => unavailable(CameraUnavailableReason::Evicted),
        tags::STATUS_CAMERA_DEVICE => unavailable(CameraUnavailableReason::DeviceFailure),
        tags::STATUS_CAMERA_SERVICE => unavailable(CameraUnavailableReason::ServiceFailure),
        other => OpenFailure::Backend(camera_backend_failure(operation, other)),
    }
}

/// Отказ создания сессии.
pub fn configure_failure(operation: &'static str, status: i32) -> ConfigureFailure {
    match status {
        tags::STATUS_CAMERA_DISCONNECTED => ConfigureFailure::DeviceDisconnected,
        tags::STATUS_STREAM_CONFIGURE_FAIL => ConfigureFailure::RejectedByHal {
            detail: format!("{operation}: {} (код {status})", camera_status_name(status)),
        },
        other => ConfigureFailure::Backend(camera_backend_failure(operation, other)),
    }
}

/// Отказ постановки или остановки повторяющегося запроса.
pub fn request_failure(operation: &'static str, status: i32) -> RequestFailure {
    match status {
        tags::STATUS_CAMERA_DISCONNECTED => RequestFailure::DeviceDisconnected,
        tags::STATUS_SESSION_CLOSED => RequestFailure::NoSession,
        other => RequestFailure::Backend(camera_backend_failure(operation, other)),
    }
}

/// Код из `ACameraDevice_ErrorStateCallback`.
pub fn device_error_kind(code: i32) -> DeviceErrorKind {
    match code {
        tags::DEVICE_ERROR_CAMERA_IN_USE => DeviceErrorKind::CameraInUse,
        tags::DEVICE_ERROR_MAX_CAMERAS_IN_USE => DeviceErrorKind::MaxCamerasInUse,
        tags::DEVICE_ERROR_CAMERA_DISABLED => DeviceErrorKind::CameraDisabled,
        tags::DEVICE_ERROR_CAMERA_DEVICE => DeviceErrorKind::CameraDevice,
        tags::DEVICE_ERROR_CAMERA_SERVICE => DeviceErrorKind::CameraService,
        other => DeviceErrorKind::Unknown(other),
    }
}

/// Причина из `ACameraCaptureFailure::reason`: `FLUSHED` — сброс запроса, всё остальное — ошибка HAL.
pub fn capture_failure_reason(code: i32) -> CaptureFailureReason {
    match code {
        tags::CAPTURE_FAILURE_FLUSHED => CaptureFailureReason::Flushed,
        _ => CaptureFailureReason::Error,
    }
}

#[cfg(test)]
#[path = "status_mapping_tests.rs"]
mod tests;
