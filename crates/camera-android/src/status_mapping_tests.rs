use super::*;

#[test]
fn every_open_status_maps_to_its_own_unavailable_reason() {
    let expected = [
        (
            tags::STATUS_PERMISSION_DENIED,
            CameraUnavailableReason::PermissionDenied,
        ),
        (
            tags::STATUS_CAMERA_IN_USE,
            CameraUnavailableReason::InUseByAnotherClient,
        ),
        (
            tags::STATUS_MAX_CAMERA_IN_USE,
            CameraUnavailableReason::TooManyCamerasOpen,
        ),
        (tags::STATUS_CAMERA_DISABLED, CameraUnavailableReason::DisabledByPolicy),
        (tags::STATUS_CAMERA_DISCONNECTED, CameraUnavailableReason::Evicted),
        (tags::STATUS_CAMERA_DEVICE, CameraUnavailableReason::DeviceFailure),
        (tags::STATUS_CAMERA_SERVICE, CameraUnavailableReason::ServiceFailure),
    ];
    for (status, reason) in expected {
        assert_eq!(
            open_failure("open", status),
            OpenFailure::Unavailable { reason },
            "код {status}"
        );
    }
}

#[test]
fn open_statuses_without_a_meaning_keep_operation_code_and_name() {
    for status in [
        tags::STATUS_INVALID_PARAMETER,
        tags::STATUS_NOT_ENOUGH_MEMORY,
        tags::STATUS_UNKNOWN,
        -777,
    ] {
        let OpenFailure::Backend(failure) = open_failure("ACameraManager_openCamera", status) else {
            panic!("код {status} не должен получить смысловой вариант");
        };
        assert_eq!(failure.operation, "ACameraManager_openCamera");
        assert_eq!(failure.code, status);
        assert_eq!(failure.detail, camera_status_name(status));
    }
    assert_eq!(camera_status_name(-777), "UNKNOWN_STATUS");
}

#[test]
fn configure_statuses_are_split_into_disconnect_hal_rejection_and_backend() {
    assert_eq!(
        configure_failure("create_session", tags::STATUS_CAMERA_DISCONNECTED),
        ConfigureFailure::DeviceDisconnected
    );
    let ConfigureFailure::RejectedByHal { detail } =
        configure_failure("create_session", tags::STATUS_STREAM_CONFIGURE_FAIL)
    else {
        panic!("STREAM_CONFIGURE_FAIL — это отказ HAL");
    };
    assert!(
        detail.contains("ACAMERA_ERROR_STREAM_CONFIGURE_FAIL") && detail.contains("-10009"),
        "{detail}"
    );
    for status in [
        tags::STATUS_CAMERA_IN_USE,
        tags::STATUS_INVALID_OPERATION,
        tags::STATUS_UNSUPPORTED_OPERATION,
    ] {
        assert!(
            matches!(configure_failure("create_session", status), ConfigureFailure::Backend(failure) if failure.code == status)
        );
    }
}

#[test]
fn request_statuses_are_split_into_disconnect_no_session_and_backend() {
    assert_eq!(
        request_failure("set_repeating", tags::STATUS_CAMERA_DISCONNECTED),
        RequestFailure::DeviceDisconnected
    );
    assert_eq!(
        request_failure("set_repeating", tags::STATUS_SESSION_CLOSED),
        RequestFailure::NoSession
    );
    let RequestFailure::Backend(failure) = request_failure("set_repeating", tags::STATUS_INVALID_PARAMETER) else {
        panic!("INVALID_PARAMETER — общий сбой");
    };
    assert_eq!(
        (failure.operation, failure.code),
        ("set_repeating", tags::STATUS_INVALID_PARAMETER)
    );
}

#[test]
fn every_device_error_code_is_named_and_unknown_codes_are_kept() {
    assert_eq!(device_error_kind(1), DeviceErrorKind::CameraInUse);
    assert_eq!(device_error_kind(2), DeviceErrorKind::MaxCamerasInUse);
    assert_eq!(device_error_kind(3), DeviceErrorKind::CameraDisabled);
    assert_eq!(device_error_kind(4), DeviceErrorKind::CameraDevice);
    assert_eq!(device_error_kind(5), DeviceErrorKind::CameraService);
    assert_eq!(device_error_kind(0), DeviceErrorKind::Unknown(0));
    assert_eq!(device_error_kind(77), DeviceErrorKind::Unknown(77));
}

#[test]
fn capture_failure_reason_distinguishes_flush_from_hal_error() {
    assert_eq!(capture_failure_reason(0), CaptureFailureReason::Flushed);
    assert_eq!(capture_failure_reason(1), CaptureFailureReason::Error);
    assert_eq!(capture_failure_reason(42), CaptureFailureReason::Error);
}

#[test]
fn media_failures_carry_the_reader_status_name() {
    let failure = media_backend_failure("AImageReader_newWithUsage", -10004);
    assert_eq!(failure.detail, "AMEDIA_ERROR_INVALID_PARAMETER");
    assert_eq!(
        media_status_name(tags::MEDIA_STATUS_NO_BUFFER_AVAILABLE),
        "AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE"
    );
    assert_eq!(media_status_name(12345), "UNKNOWN_MEDIA_STATUS");
}
