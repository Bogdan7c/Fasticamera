//! Перевод числовых кодов NDK в читаемые имена.
//!
//! Значения сверены с заголовками NDK r30 (`NdkCameraError.h`, `NdkMediaError.h`,
//! `NdkCameraMetadataTags.h`). Модуль чистый (без FFI), поэтому покрыт host-тестами.

/// Имя `camera_status_t` (Camera2 NDK).
pub fn camera_status_name(code: i32) -> String {
    const BASE: i32 = -10000; // ACAMERA_ERROR_BASE
    let name = match code {
        0 => "ACAMERA_OK",
        BASE => "ACAMERA_ERROR_UNKNOWN",
        c if c == BASE - 1 => "ACAMERA_ERROR_INVALID_PARAMETER",
        c if c == BASE - 2 => "ACAMERA_ERROR_CAMERA_DISCONNECTED",
        c if c == BASE - 3 => "ACAMERA_ERROR_NOT_ENOUGH_MEMORY",
        c if c == BASE - 4 => "ACAMERA_ERROR_METADATA_NOT_FOUND",
        c if c == BASE - 5 => "ACAMERA_ERROR_CAMERA_DEVICE",
        c if c == BASE - 6 => "ACAMERA_ERROR_CAMERA_SERVICE",
        c if c == BASE - 7 => "ACAMERA_ERROR_SESSION_CLOSED",
        c if c == BASE - 8 => "ACAMERA_ERROR_INVALID_OPERATION",
        c if c == BASE - 9 => "ACAMERA_ERROR_STREAM_CONFIGURE_FAIL",
        c if c == BASE - 10 => "ACAMERA_ERROR_CAMERA_IN_USE",
        c if c == BASE - 11 => "ACAMERA_ERROR_MAX_CAMERA_IN_USE",
        c if c == BASE - 12 => "ACAMERA_ERROR_CAMERA_DISABLED",
        c if c == BASE - 13 => "ACAMERA_ERROR_PERMISSION_DENIED",
        _ => return format!("ACAMERA_STATUS_UNRECOGNIZED({code})"),
    };
    name.to_owned()
}

/// Имя `media_status_t` (AImageReader / AImage).
pub fn media_status_name(code: i32) -> String {
    let name = match code {
        0 => "AMEDIA_OK",
        -10000 => "AMEDIA_ERROR_UNKNOWN",
        -10001 => "AMEDIA_ERROR_MALFORMED",
        -10002 => "AMEDIA_ERROR_UNSUPPORTED",
        -10003 => "AMEDIA_ERROR_INVALID_OBJECT",
        -10004 => "AMEDIA_ERROR_INVALID_PARAMETER",
        -10005 => "AMEDIA_ERROR_INVALID_OPERATION",
        -10006 => "AMEDIA_ERROR_END_OF_STREAM",
        -10007 => "AMEDIA_ERROR_IO",
        -10008 => "AMEDIA_ERROR_WOULD_BLOCK",
        -30001 => "AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE",
        -30002 => "AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED",
        -30003 => "AMEDIA_IMGREADER_CANNOT_LOCK_IMAGE",
        -30004 => "AMEDIA_IMGREADER_CANNOT_UNLOCK_IMAGE",
        -30005 => "AMEDIA_IMGREADER_IMAGE_NOT_LOCKED",
        _ => return format!("AMEDIA_STATUS_UNRECOGNIZED({code})"),
    };
    name.to_owned()
}

/// Имя значения `ACAMERA_REQUEST_AVAILABLE_CAPABILITIES`.
///
/// Числа 4, 7, 9, 15, 17, 18 в NDK-заголовках не объявлены как константы, но
/// существуют в Java `CameraMetadata` — имена взяты оттуда и помечены суффиксом.
pub fn capability_name(code: i32) -> String {
    let name = match code {
        0 => "BACKWARD_COMPATIBLE",
        1 => "MANUAL_SENSOR",
        2 => "MANUAL_POST_PROCESSING",
        3 => "RAW",
        4 => "PRIVATE_REPROCESSING(java-only)",
        5 => "READ_SENSOR_SETTINGS",
        6 => "BURST_CAPTURE",
        7 => "YUV_REPROCESSING(java-only)",
        8 => "DEPTH_OUTPUT",
        9 => "CONSTRAINED_HIGH_SPEED_VIDEO(java-only)",
        10 => "MOTION_TRACKING",
        11 => "LOGICAL_MULTI_CAMERA",
        12 => "MONOCHROME",
        13 => "SECURE_IMAGE_DATA",
        14 => "SYSTEM_CAMERA",
        15 => "OFFLINE_PROCESSING(java-only)",
        16 => "ULTRA_HIGH_RESOLUTION_SENSOR",
        17 => "REMOSAIC_REPROCESSING(java-only)",
        18 => "DYNAMIC_RANGE_TEN_BIT(java-only)",
        19 => "STREAM_USE_CASE",
        _ => return format!("UNRECOGNIZED({code})"),
    };
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_status_names_match_ndk_values() {
        assert_eq!(camera_status_name(0), "ACAMERA_OK");
        assert_eq!(camera_status_name(-10013), "ACAMERA_ERROR_PERMISSION_DENIED");
        assert_eq!(camera_status_name(-10010), "ACAMERA_ERROR_CAMERA_IN_USE");
        assert_eq!(camera_status_name(-1), "ACAMERA_STATUS_UNRECOGNIZED(-1)");
    }

    #[test]
    fn media_status_names_match_ndk_values() {
        assert_eq!(media_status_name(0), "AMEDIA_OK");
        assert_eq!(media_status_name(-10004), "AMEDIA_ERROR_INVALID_PARAMETER");
        assert_eq!(media_status_name(-30001), "AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE");
        assert_eq!(media_status_name(7), "AMEDIA_STATUS_UNRECOGNIZED(7)");
    }

    #[test]
    fn capability_14_is_system_camera() {
        assert_eq!(capability_name(14), "SYSTEM_CAMERA");
        assert_eq!(capability_name(3), "RAW");
        assert_eq!(capability_name(99), "UNRECOGNIZED(99)");
    }
}
