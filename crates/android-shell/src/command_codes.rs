//! Коды команд платформы в JNI-вызове об отказе (`ShellBridge.nativePlatformCommandFailed`).
//!
//! Контракт с Kotlin: те же числа — константы `ShellPlatform.COMMAND_*`
//! (`android/app/src/main/kotlin/dev/fasticamera/ShellPlatform.kt`).

use shell_contract::commands::PlatformCommand;
use shell_contract::operator_prompt::OperatorPromptKind;

pub const REQUEST_CAMERA_PERMISSION: i32 = 1;
pub const PLAY_HAPTIC: i32 = 2;
pub const OPEN_SYSTEM_GALLERY: i32 = 3;
pub const KEEP_SCREEN_ON: i32 = 4;
pub const SHOW_OPERATOR_PROMPT: i32 = 5;
pub const CLEAR_OPERATOR_PROMPT: i32 = 6;
pub const PLAY_SHUTTER_SOUND: i32 = 7;
pub const REQUEST_TEXT_INPUT: i32 = 8;

/// Код вида подсказки для `ShellPlatform.showOperatorPrompt(…, kind)`; Kotlin по нему выбирает цвет.
/// Контракт с Kotlin: те же числа — константы `ShellPlatform.PROMPT_KIND_*`.
pub fn prompt_kind_code(kind: OperatorPromptKind) -> i32 {
    match kind {
        OperatorPromptKind::Prepare => 1,
        OperatorPromptKind::Act => 2,
        OperatorPromptKind::Done => 3,
        OperatorPromptKind::Info => 4,
    }
}

/// Код из Kotlin → команда; неизвестный код — `None` (сообщается как ошибка контракта).
pub fn platform_command_from_code(code: i32) -> Option<PlatformCommand> {
    match code {
        REQUEST_CAMERA_PERMISSION => Some(PlatformCommand::RequestCameraPermission),
        PLAY_HAPTIC => Some(PlatformCommand::PlayHaptic),
        OPEN_SYSTEM_GALLERY => Some(PlatformCommand::OpenSystemGallery),
        KEEP_SCREEN_ON => Some(PlatformCommand::KeepScreenOn),
        SHOW_OPERATOR_PROMPT => Some(PlatformCommand::ShowOperatorPrompt),
        CLEAR_OPERATOR_PROMPT => Some(PlatformCommand::ClearOperatorPrompt),
        PLAY_SHUTTER_SOUND => Some(PlatformCommand::PlayShutterSound),
        REQUEST_TEXT_INPUT => Some(PlatformCommand::RequestTextInput),
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
            SHOW_OPERATOR_PROMPT,
            CLEAR_OPERATOR_PROMPT,
            PLAY_SHUTTER_SOUND,
            REQUEST_TEXT_INPUT,
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
                Some(PlatformCommand::ShowOperatorPrompt),
                Some(PlatformCommand::ClearOperatorPrompt),
                Some(PlatformCommand::PlayShutterSound),
                Some(PlatformCommand::RequestTextInput),
            ]
        );
    }

    #[test]
    fn command_codes_are_unique() {
        let codes = [
            REQUEST_CAMERA_PERMISSION,
            PLAY_HAPTIC,
            OPEN_SYSTEM_GALLERY,
            KEEP_SCREEN_ON,
            SHOW_OPERATOR_PROMPT,
            CLEAR_OPERATOR_PROMPT,
            PLAY_SHUTTER_SOUND,
            REQUEST_TEXT_INPUT,
        ];
        let unique: std::collections::HashSet<_> = codes.iter().collect();
        assert_eq!(unique.len(), codes.len());
    }

    #[test]
    fn prompt_kind_codes_are_unique_and_match_kotlin_constants() {
        let codes: Vec<i32> = OperatorPromptKind::ALL.into_iter().map(prompt_kind_code).collect();
        // ShellPlatform.PROMPT_KIND_PREPARE/ACT/DONE/INFO = 1/2/3/4.
        assert_eq!(codes, vec![1, 2, 3, 4]);
    }

    #[test]
    fn unknown_code_is_none() {
        assert_eq!(platform_command_from_code(0), None);
        assert_eq!(platform_command_from_code(99), None);
    }
}
