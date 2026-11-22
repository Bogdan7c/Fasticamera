//! Решение «снимать ли этот кадр со вспышкой» (сессия 14d, решение 159): режим экрана + совет камеры → маршрут.
//!
//! Чистая функция без часов и ввода-вывода: сервис снимка спрашивает маршрут при приёме запроса и выполняет его.
//!
//! | режим экрана | совет камеры (`FlashAdvice`)          | маршрут                                             |
//! |--------------|----------------------------------------|-----------------------------------------------------|
//! | `Off`        | не читается                            | без вспышки: показанный кадр                        |
//! | `On`         | не читается                            | со вспышкой                                         |
//! | `Auto`       | `Fire`                                 | со вспышкой                                         |
//! | `Auto`       | `NotNeeded`                            | без вспышки: показанный кадр                        |
//! | `Auto`       | `Unavailable { причина }`              | без вспышки: показанный кадр (честно, с причиной)   |

use camera_contract::flash::FlashShotId;
use camera_core::flash_status::{FlashAdvice, FlashAdviceUnavailable};
use camera_screen::settings::FlashMode;
use still_contract::plan::FrameSelection;

/// Почему снимок идёт БЕЗ вспышки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoFlashReason {
    /// Режим экрана `Off`.
    ModeOff,
    /// `Auto`: света хватает.
    AutoNotNeeded,
    /// `Auto`: камера совета не дала (нет вспышки, чужой AE, нет замера, поток не идёт).
    AutoAdviceUnavailable(FlashAdviceUnavailable),
}

/// Почему снимок идёт СО вспышкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashReason {
    /// Режим экрана `On`.
    ModeOn,
    /// `Auto`: камера посоветовала вспыхнуть.
    AutoAdvisedFire,
}

/// Как снимать кадр.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashRoute {
    /// Обычный снимок: кадр, который был на экране в момент нажатия.
    ShownAtPress(NoFlashReason),
    /// Снимок со вспышкой: кадр разового запроса камеры.
    FlashFrame(FlashReason),
}

impl FlashRoute {
    /// Решает маршрут. Совет камеры читается ЛЕНИВО и только для `Auto`: `Off`/`On` его не зависят, и читать
    /// статус движка ради них незачем.
    pub fn decide(mode: FlashMode, advice: impl FnOnce() -> FlashAdvice) -> Self {
        match mode {
            FlashMode::Off => Self::ShownAtPress(NoFlashReason::ModeOff),
            FlashMode::On => Self::FlashFrame(FlashReason::ModeOn),
            FlashMode::Auto => match advice() {
                FlashAdvice::Fire => Self::FlashFrame(FlashReason::AutoAdvisedFire),
                FlashAdvice::NotNeeded => Self::ShownAtPress(NoFlashReason::AutoNotNeeded),
                FlashAdvice::Unavailable { reason } => Self::ShownAtPress(NoFlashReason::AutoAdviceUnavailable(reason)),
            },
        }
    }

    /// Идёт ли снимок со вспышкой.
    pub const fn uses_flash(&self) -> bool {
        matches!(self, Self::FlashFrame(_))
    }

    /// Какой кадр должен выбрать рендер для снимка `shot` (номер вспышки нужен только вспышечному маршруту).
    pub const fn selection(&self, shot: FlashShotId) -> FrameSelection {
        match self {
            Self::ShownAtPress(_) => FrameSelection::ShownAtPress,
            Self::FlashFrame(_) => FrameSelection::FlashFrame { shot },
        }
    }
}

#[cfg(test)]
#[path = "flash_shot_route_tests.rs"]
mod tests;
