//! Физическая ориентация телефона.
//!
//! Раскладка экрана зафиксирована в портрете (как у камеры iOS), а иконки поворачиваются
//! вслед за телефоном. Поэтому нужна ориентация с датчика, а не поворот дисплея.
//! Названия совпадают с `UIDeviceOrientation` iOS.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceOrientation {
    /// Телефон вертикально, верх смотрит вверх.
    Portrait,
    /// Повёрнут на 90° против часовой стрелки: верх телефона смотрит влево.
    LandscapeLeft,
    /// Повёрнут на 90° по часовой стрелке: верх телефона смотрит вправо.
    LandscapeRight,
    /// Вверх ногами.
    PortraitUpsideDown,
}

impl DeviceOrientation {
    /// Имя для логов и отчётов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            DeviceOrientation::Portrait => "portrait",
            DeviceOrientation::LandscapeLeft => "landscape_left",
            DeviceOrientation::LandscapeRight => "landscape_right",
            DeviceOrientation::PortraitUpsideDown => "portrait_upside_down",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_matches_serde() {
        for orientation in [
            DeviceOrientation::Portrait,
            DeviceOrientation::LandscapeLeft,
            DeviceOrientation::LandscapeRight,
            DeviceOrientation::PortraitUpsideDown,
        ] {
            let json = serde_json::to_value(orientation).expect("json");
            assert_eq!(json, serde_json::json!(orientation.name()));
        }
    }
}
