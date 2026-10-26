//! Коды тепла Android → нейтральные типы.
//!
//! Значения `AThermalStatus` (NDK 30 `android/thermal.h`) совпадают с
//! `PowerManager.THERMAL_STATUS_*` (android.jar SDK 36); `-1` — ошибка сервиса.

use shell_contract::thermal::ThermalStatus;

/// Код, который не соответствует ни одному статусу (в том числе `ATHERMAL_STATUS_ERROR`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("тепловой сервис вернул код статуса {0}, а не 0–6")]
pub struct UnknownThermalStatus(pub i32);

pub fn thermal_status_from_platform_code(code: i32) -> Result<ThermalStatus, UnknownThermalStatus> {
    match code {
        0 => Ok(ThermalStatus::None),
        1 => Ok(ThermalStatus::Light),
        2 => Ok(ThermalStatus::Moderate),
        3 => Ok(ThermalStatus::Severe),
        4 => Ok(ThermalStatus::Critical),
        5 => Ok(ThermalStatus::Emergency),
        6 => Ok(ThermalStatus::Shutdown),
        other => Err(UnknownThermalStatus(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_platform_codes_map_in_severity_order() {
        let statuses: Vec<ThermalStatus> = (0..=6)
            .map(|code| thermal_status_from_platform_code(code).expect("код"))
            .collect();
        assert!(statuses.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(statuses[0], ThermalStatus::None);
        assert_eq!(statuses[6], ThermalStatus::Shutdown);
    }

    #[test]
    fn error_and_unknown_codes_are_errors() {
        assert_eq!(thermal_status_from_platform_code(-1), Err(UnknownThermalStatus(-1)));
        assert_eq!(thermal_status_from_platform_code(7), Err(UnknownThermalStatus(7)));
    }
}
