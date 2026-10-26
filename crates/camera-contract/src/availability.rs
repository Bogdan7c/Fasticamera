//! Почему камера недоступна — варианты не сливаются (правило 6 `AGENTS.md`).

use serde::Serialize;

/// Причина, по которой камеры сейчас нет у приложения.
///
/// От причины зависит восстановление (решение 39): вытеснение лечится ожиданием свободной
/// камеры, сбой устройства — повтором, а запрет политики или отказ в разрешении — только
/// внешним действием. Политику восстановления держит `camera-core`, здесь только имена.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraUnavailableReason {
    /// Камеру отнял клиент с более высоким приоритетом (`onDisconnected` открытого устройства).
    Evicted,
    /// При открытии камера уже занята другим клиентом (`ACAMERA_ERROR_CAMERA_IN_USE`).
    InUseByAnotherClient,
    /// Достигнут системный предел открытых камер (`ACAMERA_ERROR_MAX_CAMERAS_IN_USE`).
    TooManyCamerasOpen,
    /// Камера запрещена политикой устройства (`ACAMERA_ERROR_CAMERA_DISABLED`).
    DisabledByPolicy,
    /// Нет разрешения `CAMERA` (`ACAMERA_ERROR_PERMISSION_DENIED`).
    PermissionDenied,
    /// Сбой устройства камеры (`ERROR_CAMERA_DEVICE`): нужно переоткрыть.
    DeviceFailure,
    /// Сбой службы камеры (`ERROR_CAMERA_SERVICE`, перезапуск HAL).
    ServiceFailure,
    /// Камеры больше нет в системе (внешняя отключена, ID исчез).
    CameraNotPresent,
}

impl CameraUnavailableReason {
    /// Имя для логов и отчётов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            CameraUnavailableReason::Evicted => "evicted",
            CameraUnavailableReason::InUseByAnotherClient => "in_use_by_another_client",
            CameraUnavailableReason::TooManyCamerasOpen => "too_many_cameras_open",
            CameraUnavailableReason::DisabledByPolicy => "disabled_by_policy",
            CameraUnavailableReason::PermissionDenied => "permission_denied",
            CameraUnavailableReason::DeviceFailure => "device_failure",
            CameraUnavailableReason::ServiceFailure => "service_failure",
            CameraUnavailableReason::CameraNotPresent => "camera_not_present",
        }
    }

    pub const ALL: [CameraUnavailableReason; 8] = [
        CameraUnavailableReason::Evicted,
        CameraUnavailableReason::InUseByAnotherClient,
        CameraUnavailableReason::TooManyCamerasOpen,
        CameraUnavailableReason::DisabledByPolicy,
        CameraUnavailableReason::PermissionDenied,
        CameraUnavailableReason::DeviceFailure,
        CameraUnavailableReason::ServiceFailure,
        CameraUnavailableReason::CameraNotPresent,
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matches_serde_for_every_reason() {
        for reason in CameraUnavailableReason::ALL {
            assert_eq!(
                serde_json::to_value(reason).expect("json"),
                serde_json::json!(reason.name())
            );
        }
    }
}
