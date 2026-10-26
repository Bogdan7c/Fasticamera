//! Коды команд платформы в JNI-вызове об отказе (`ShellBridge.nativePlatformCommandFailed`).
//!
//! Контракт с Kotlin: те же числа — константы `ShellPlatform.COMMAND_*`
//! (`android/app/src/main/kotlin/dev/fasticamera/ShellPlatform.kt`).

use shell_contract::commands::PlatformCommand;

pub const REQUEST_CAMERA_PERMISSION: i32 = 1;
pub const PLAY_HAPTIC: i32 = 2;
pub const OPEN_SYSTEM_GALLERY: i32 = 3;
pub const KEEP_SCREEN_ON: i32 = 4;

/// Код из Kotlin → команда; неизвестный код — `None` (сообщается как ошибка контракта).
pub fn platform_command_from_code(code: i32) -> Option<PlatformCommand> {
    match code {
        REQUEST_CAMERA_PERMISSION => Some(PlatformCommand::RequestCameraPermission),
        PLAY_HAPTIC => Some(PlatformCommand::PlayHaptic),
        OPEN_SYSTEM_GALLERY => Some(PlatformCommand::OpenSystemGallery),
        KEEP_SCREEN_ON => Some(PlatformCommand::KeepScreenOn),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_codes_map_to_distinct_commands() {
        let commands: Vec<_> = [
            REQUEST_CAMERA_PERMISSION,
            PLAY_HAPTIC,
            OPEN_SYSTEM_GALLERY,
            KEEP_SCREEN_ON,
        ]
        .into_iter()
        .map(platform_command_from_code)
        .collect();
        assert_eq!(
            commands,
            vec![
                Some(PlatformCommand::RequestCameraPermission),
                Some(PlatformCommand::PlayHaptic),
                Some(PlatformCommand::OpenSystemGallery),
                Some(PlatformCommand::KeepScreenOn),
            ]
        );
    }

    #[test]
    fn unknown_code_is_none() {
        assert_eq!(platform_command_from_code(0), None);
        assert_eq!(platform_command_from_code(99), None);
    }
}
