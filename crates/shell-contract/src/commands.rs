//! Команды Rust → платформа (intent-методы оболочки).
//!
//! Все команды неблокирующие: вызывающий узнаёт только, принята ли команда. Если платформа
//! потом не смогла её выполнить (например, нет приложения галереи), оболочка присылает событие
//! `ShellEvent::PlatformCommandFailed` — отказ не теряется молча.

use serde::Serialize;

/// Желаемая частота кадров surface в герцах (конечное положительное число).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(transparent)]
pub struct FrameRate(f32);

/// Неверная частота кадров.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
#[error("частота кадров должна быть конечной и больше нуля, а не {0}")]
pub struct InvalidFrameRate(pub f32);

impl FrameRate {
    pub fn new(hertz: f32) -> Result<Self, InvalidFrameRate> {
        if hertz.is_finite() && hertz > 0.0 {
            Ok(Self(hertz))
        } else {
            Err(InvalidFrameRate(hertz))
        }
    }

    pub fn hertz(self) -> f32 {
        self.0
    }
}

/// Не гасить ли экран, пока приложение на переднем плане.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenWake {
    KeepOn,
    AllowSleep,
}

/// Итог постановки команды платформе.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOutcome {
    /// Команда принята и будет выполнена платформой асинхронно.
    Queued,
    /// Activity нет: выполнять команду некому.
    NotAttached,
    /// Очередь команд переполнена — команда отброшена и посчитана.
    Backpressure,
}

/// Итог `set_preferred_frame_rate`. Частота — свойство surface, поэтому без surface она
/// запоминается и применяется к следующей.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum FrameRateOutcome {
    /// Платформа приняла пожелание для текущей surface.
    Applied,
    /// Surface сейчас нет; пожелание будет применено к следующей.
    DeferredUntilSurface,
    /// Платформа отвергла пожелание (код ошибки платформы).
    Rejected { status: i32 },
}

/// Какая команда платформы не выполнилась (для события об отказе).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformCommand {
    RequestCameraPermission,
    PlayHaptic,
    OpenSystemGallery,
    KeepScreenOn,
    QueryHapticCapabilities,
    /// Показать подсказку владельцу на экране.
    ShowOperatorPrompt,
    /// Убрать подсказку владельцу с экрана.
    ClearOperatorPrompt,
    /// Сыграть звук затвора (`MediaActionSound`).
    PlayShutterSound,
    /// Показать диалог ввода текста (системная клавиатура).
    RequestTextInput,
}

/// Отказ платформы выполнить ранее принятую команду.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlatformCommandFailure {
    pub command: PlatformCommand,
    /// Объяснение от платформы (например, текст исключения Java).
    pub reason: String,
}

/// Команды оболочке. Хаптика — отдельный узкий трейт [`crate::haptics::HapticsService`],
/// чтобы её потребители не зависели от остальных команд.
pub trait ShellCommands: Send + Sync {
    /// Показать системный запрос разрешения камеры. Ответ придёт событием
    /// `ShellEvent::CameraPermission`; если разрешение уже есть — сразу `Granted` без диалога.
    fn request_camera_permission(&self) -> CommandOutcome;

    /// Пожелание частоты обновления экрана для surface видоискателя.
    fn set_preferred_frame_rate(&self, rate: FrameRate) -> FrameRateOutcome;

    /// Открыть системную галерею (в v1 миниатюра снимка ведёт туда, README «Бэклог»).
    fn open_system_gallery(&self) -> CommandOutcome;

    /// Не гасить экран (или разрешить гасить), пока приложение на переднем плане.
    fn keep_screen_on(&self, wake: ScreenWake) -> CommandOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_rate_accepts_positive_finite_only() {
        assert_eq!(FrameRate::new(120.0).map(FrameRate::hertz), Ok(120.0));
        assert_eq!(FrameRate::new(0.0), Err(InvalidFrameRate(0.0)));
        assert!(FrameRate::new(-60.0).is_err());
        assert!(FrameRate::new(f32::NAN).is_err());
        assert!(FrameRate::new(f32::INFINITY).is_err());
    }

    #[test]
    fn frame_rate_outcome_serializes_with_result_tag() {
        assert_eq!(
            serde_json::to_value(FrameRateOutcome::Rejected { status: -22 }).expect("json"),
            serde_json::json!({"result": "rejected", "status": -22})
        );
        assert_eq!(
            serde_json::to_value(FrameRateOutcome::Applied).expect("json"),
            serde_json::json!({"result": "applied"})
        );
    }
}
