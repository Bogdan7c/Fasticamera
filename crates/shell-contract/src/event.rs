//! События оболочки: всё, что платформа сообщает Rust.
//!
//! Порядок событий в очереди — порядок, в котором их прислал Android. Про surface и
//! «на переднем плане» Android порядок **не гарантирует** (`surfaceCreated` может прийти и до,
//! и после `onResume`), поэтому потребители держат машину состояний, а не ждут жёсткой
//! последовательности.

use serde::Serialize;

use crate::commands::PlatformCommandFailure;
use crate::hardware_key::HardwareKeyEvent;
use crate::orientation::DeviceOrientation;
use crate::permission::CameraPermission;
use crate::surface::{NativeSurface, SurfaceGeneration, SurfaceReleaseRequest, SurfaceSize};
use crate::thermal::ThermalReading;
use crate::touch::TouchSample;

/// Событие оболочки.
#[derive(Debug)]
pub enum ShellEvent {
    /// Появилась surface экрана. Рисовать на ней можно, пока не придёт `SurfaceDestroyed`
    /// того же поколения.
    SurfaceCreated(NativeSurface),
    /// Surface сменила размер (приходит и сразу после создания).
    SurfaceChanged {
        generation: SurfaceGeneration,
        size: SurfaceSize,
    },
    /// Surface уходит. Владелец рендера обязан отпустить всё, что на ней рисует, и
    /// подтвердить квитанцию: Android ждёт оболочку, а оболочка — это подтверждение.
    SurfaceDestroyed(SurfaceReleaseRequest),
    /// Activity на переднем плане (`onResume`).
    Resumed,
    /// Activity ушла с переднего плана (`onPause`).
    Paused,
    /// Касание.
    Touch(TouchSample),
    /// В потоке касаний дыра: столько записей выброшено из-за переполнения очереди.
    /// Распознаватели жестов обязаны сбросить незавершённые жесты.
    TouchGap { dropped_samples: u64 },
    /// Аппаратная клавиша (громкость) нажата или отпущена. Редкое событие: относится к
    /// «критичным» очереди и не вытесняется касаниями (при полной очереди — потеря со счётчиком).
    HardwareKey(HardwareKeyEvent),
    /// Физическая ориентация телефона сменилась.
    DeviceOrientation(DeviceOrientation),
    /// Новое тепловое показание (по смене статуса или по периодическому опросу запаса).
    ThermalStatus(ThermalReading),
    /// Ответ на запрос разрешения камеры.
    CameraPermission(CameraPermission),
    /// Платформа не смогла выполнить ранее принятую команду.
    PlatformCommandFailed(PlatformCommandFailure),
}

/// Вид события без данных — для логов, отчётов и машин состояний сценариев.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ShellEventKind {
    SurfaceCreated,
    SurfaceChanged,
    SurfaceDestroyed,
    Resumed,
    Paused,
    Touch,
    TouchGap,
    HardwareKey,
    DeviceOrientation,
    ThermalStatus,
    CameraPermission,
    PlatformCommandFailed,
}

impl ShellEventKind {
    /// Имя для логов и отчётов (совпадает с serde; контракт с `scripts/smoke_checks.py`).
    pub fn name(self) -> &'static str {
        match self {
            ShellEventKind::SurfaceCreated => "surface_created",
            ShellEventKind::SurfaceChanged => "surface_changed",
            ShellEventKind::SurfaceDestroyed => "surface_destroyed",
            ShellEventKind::Resumed => "resumed",
            ShellEventKind::Paused => "paused",
            ShellEventKind::Touch => "touch",
            ShellEventKind::TouchGap => "touch_gap",
            ShellEventKind::HardwareKey => "hardware_key",
            ShellEventKind::DeviceOrientation => "device_orientation",
            ShellEventKind::ThermalStatus => "thermal_status",
            ShellEventKind::CameraPermission => "camera_permission",
            ShellEventKind::PlatformCommandFailed => "platform_command_failed",
        }
    }
}

impl ShellEvent {
    pub fn kind(&self) -> ShellEventKind {
        match self {
            ShellEvent::SurfaceCreated(_) => ShellEventKind::SurfaceCreated,
            ShellEvent::SurfaceChanged { .. } => ShellEventKind::SurfaceChanged,
            ShellEvent::SurfaceDestroyed(_) => ShellEventKind::SurfaceDestroyed,
            ShellEvent::Resumed => ShellEventKind::Resumed,
            ShellEvent::Paused => ShellEventKind::Paused,
            ShellEvent::Touch(_) => ShellEventKind::Touch,
            ShellEvent::TouchGap { .. } => ShellEventKind::TouchGap,
            ShellEvent::HardwareKey(_) => ShellEventKind::HardwareKey,
            ShellEvent::DeviceOrientation(_) => ShellEventKind::DeviceOrientation,
            ShellEvent::ThermalStatus(_) => ShellEventKind::ThermalStatus,
            ShellEvent::CameraPermission(_) => ShellEventKind::CameraPermission,
            ShellEvent::PlatformCommandFailed(_) => ShellEventKind::PlatformCommandFailed,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_name_matches_serde() {
        for kind in [
            ShellEventKind::SurfaceCreated,
            ShellEventKind::SurfaceChanged,
            ShellEventKind::SurfaceDestroyed,
            ShellEventKind::Resumed,
            ShellEventKind::Paused,
            ShellEventKind::Touch,
            ShellEventKind::TouchGap,
            ShellEventKind::HardwareKey,
            ShellEventKind::DeviceOrientation,
            ShellEventKind::ThermalStatus,
            ShellEventKind::CameraPermission,
            ShellEventKind::PlatformCommandFailed,
        ] {
            assert_eq!(
                serde_json::to_value(kind).expect("json"),
                serde_json::json!(kind.name())
            );
        }
    }
}
