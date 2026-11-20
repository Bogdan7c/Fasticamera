//! Подключение движка к экрану камеры по намерениям объектива (сессия 13).
//!
//! Экран камеры публикует [`camera_contract::zoom::LensIntent`] в ящик
//! [`camera_contract::zoom::LensIntentChannel`] (последнее значение побеждает), движок читает
//! его на потоке камеры. Толчок «в ящике новое» привязывает сам [`crate::CameraRuntime`] при
//! запуске: корню композиции достаточно передать ящик.

use camera_contract::zoom::LensIntentChannel;

/// Ящик намерений объектива, который движок читает.
#[derive(Debug, Clone, Default)]
pub struct LensWiring {
    pub intents: LensIntentChannel,
}

impl LensWiring {
    /// Подключение к уже созданному ящику (его держит и экран камеры).
    pub fn new(intents: LensIntentChannel) -> Self {
        Self { intents }
    }
}
