//! Разрешение камеры глазами приложения.

use serde::Serialize;

/// Итог запроса разрешения камеры.
///
/// Разница между `Denied` и `DeniedPermanently` важна для UI: после первого отказа можно
/// спросить ещё раз, после второго Android диалог больше не показывает, и остаётся только
/// отправить пользователя в системные настройки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraPermission {
    Granted,
    /// Отказ, но системный диалог ещё можно показать снова.
    Denied,
    /// Отказ, после которого система диалог больше не показывает.
    DeniedPermanently,
    /// Диалог прерван без ответа (ответ пришёл пустым).
    Cancelled,
}

impl CameraPermission {
    /// Имя для логов и отчётов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            CameraPermission::Granted => "granted",
            CameraPermission::Denied => "denied",
            CameraPermission::DeniedPermanently => "denied_permanently",
            CameraPermission::Cancelled => "cancelled",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matches_serde() {
        for permission in [
            CameraPermission::Granted,
            CameraPermission::Denied,
            CameraPermission::DeniedPermanently,
            CameraPermission::Cancelled,
        ] {
            assert_eq!(
                serde_json::to_value(permission).expect("json"),
                serde_json::json!(permission.name())
            );
        }
    }
}
