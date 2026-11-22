//! Шов «сервис снимка → камера» для вспышки (сессия 14d): совет для режима Auto и команда «снять со вспышкой».
//!
//! Сервис снимка не знает ни ручку `CameraHandle`, ни статус движка целиком: ему нужны два намерения —
//! «что камера советует для Auto» и «сними кадр со вспышкой номер N». Настоящий порт ([`CameraHandleFlashPort`])
//! стоит над ручкой движка, подделки тестов подставляются без правок сервиса.
//!
//! # Соглашение о номерах
//! Снимок службы ([`StillId`]) и снимок со вспышкой камеры ([`FlashShotId`]) — один и тот же номер: так итог камеры
//! («вспышка не снята») возвращается в рендер и к службе без таблицы соответствия. Соглашение живёт ТОЛЬКО здесь
//! ([`flash_shot_of`], [`still_of_flash_shot`]): остальной код номера не переводит сам.

use camera_contract::flash::FlashShotId;
use camera_core::flash_status::FlashStatus;
use camera_core::{CameraHandle, CommandRejected};
use still_contract::plan::StillId;

/// Номер снимка со вспышкой для снимка службы.
pub const fn flash_shot_of(still: StillId) -> FlashShotId {
    FlashShotId(still.0)
}

/// Снимок службы, которому принадлежит снимок со вспышкой камеры.
pub const fn still_of_flash_shot(shot: FlashShotId) -> StillId {
    StillId(shot.0)
}

/// Что сервис снимка просит у камеры ради вспышки. Не блокирует: вызывается с потока рендера.
pub trait FlashCameraPort: Send + Sync {
    /// Состояние вспышки активной камеры (совет для Auto, фаза, счётчики) на этот момент.
    fn flash_status(&self) -> FlashStatus;

    /// Поставить движку снимок со вспышкой `shot`. `Err` — команда не дошла до движка (очередь полна или движок
    /// остановлен): итога камеры не будет, о срыве обязан сообщить вызывающий. `Ok` — команда в очереди движка,
    /// итог придёт наблюдателем ([`crate::flash_still_relay::FlashStillRelay`]).
    fn capture_flash_still(&self, shot: FlashShotId) -> Result<(), CommandRejected>;
}

/// Настоящий порт поверх ручки движка камеры.
pub struct CameraHandleFlashPort {
    handle: CameraHandle,
}

impl CameraHandleFlashPort {
    pub fn new(handle: CameraHandle) -> Self {
        Self { handle }
    }
}

impl FlashCameraPort for CameraHandleFlashPort {
    fn flash_status(&self) -> FlashStatus {
        self.handle.status().three_a.flash
    }

    fn capture_flash_still(&self, shot: FlashShotId) -> Result<(), CommandRejected> {
        // Номер команды сервису не нужен: итог адресуется номером снимка, а не номером команды.
        self.handle.capture_flash_still(shot).map(|_command| ())
    }
}

#[cfg(test)]
#[path = "flash_camera_port_tests.rs"]
mod tests;
