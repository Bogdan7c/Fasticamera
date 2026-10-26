//! Ответ системного диалога разрешения → [`CameraPermission`].
//!
//! Kotlin передаёт сырой ответ `onRequestPermissionsResult` (массив `grantResults`) и значение
//! `shouldShowRequestPermissionRationale` **после** ответа. Смысл (Android 11+):
//! * пустой массив — диалог прерван, ответа нет;
//! * `PERMISSION_GRANTED` (0) — разрешено;
//! * отказ и rationale = true — первый отказ, диалог можно показать снова;
//! * отказ и rationale = false — второй отказ («больше не спрашивать» неявно), система
//!   диалог больше не покажет.

use shell_contract::permission::CameraPermission;

/// `PackageManager.PERMISSION_GRANTED` (android.jar SDK 36).
const PERMISSION_GRANTED: i32 = 0;

/// Можно ли ещё показать диалог разрешения (`shouldShowRequestPermissionRationale`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RationaleVisibility {
    CanAskAgain,
    SystemStopsAsking,
}

impl RationaleVisibility {
    pub fn from_should_show_rationale(should_show: bool) -> Self {
        if should_show {
            RationaleVisibility::CanAskAgain
        } else {
            RationaleVisibility::SystemStopsAsking
        }
    }
}

/// Классифицирует ответ на запрос одного разрешения (камеры).
pub fn classify_permission_answer(grant_results: &[i32], rationale: RationaleVisibility) -> CameraPermission {
    match grant_results.first() {
        None => CameraPermission::Cancelled,
        Some(&PERMISSION_GRANTED) => CameraPermission::Granted,
        Some(_) => match rationale {
            RationaleVisibility::CanAskAgain => CameraPermission::Denied,
            RationaleVisibility::SystemStopsAsking => CameraPermission::DeniedPermanently,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PERMISSION_DENIED: i32 = -1;

    #[test]
    fn granted_ignores_rationale() {
        assert_eq!(
            classify_permission_answer(&[PERMISSION_GRANTED], RationaleVisibility::SystemStopsAsking),
            CameraPermission::Granted
        );
    }

    #[test]
    fn first_denial_is_denied() {
        assert_eq!(
            classify_permission_answer(&[PERMISSION_DENIED], RationaleVisibility::CanAskAgain),
            CameraPermission::Denied
        );
    }

    #[test]
    fn second_denial_is_permanent() {
        assert_eq!(
            classify_permission_answer(&[PERMISSION_DENIED], RationaleVisibility::SystemStopsAsking),
            CameraPermission::DeniedPermanently
        );
    }

    #[test]
    fn empty_answer_is_cancelled() {
        assert_eq!(
            classify_permission_answer(&[], RationaleVisibility::CanAskAgain),
            CameraPermission::Cancelled
        );
    }

    #[test]
    fn rationale_flag_maps_both_ways() {
        assert_eq!(
            RationaleVisibility::from_should_show_rationale(true),
            RationaleVisibility::CanAskAgain
        );
        assert_eq!(
            RationaleVisibility::from_should_show_rationale(false),
            RationaleVisibility::SystemStopsAsking
        );
    }
}
