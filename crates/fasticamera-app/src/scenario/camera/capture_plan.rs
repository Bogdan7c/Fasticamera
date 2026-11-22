//! План шагов сценария `capture` (сессия 14): что снимаем и в каком порядке.
//!
//! Порядок важен: «HEIF + DNG» включает RAW и возвращает его, а серия идёт СЛЕДОМ и снимает без RAW — по
//! настройке запроса серии вердикт доказывает, что RAW действительно вернулся. Так же серия идёт после вспышечных
//! шагов: её запрос обязан показать, что вспышка вернулась в `Off` (14d).

use camera_screen::settings::{FlashMode, RawCapture};

/// Шаг: один снимок без RAW.
pub const STEP_SINGLE_HEIF: &str = "single_heif";
/// Шаг: один снимок с RAW (два файла: HEIC и DNG).
pub const STEP_SINGLE_HEIF_DNG: &str = "single_heif_dng";
/// Шаг: один снимок со вспышкой `On` (кадр разового запроса камеры, 14d).
pub const STEP_SINGLE_FLASH_ON: &str = "single_flash_on";
/// Шаг: один снимок со вспышкой `Auto` (14d). Наблюдательный: сработала ли вспышка, зависит от света в комнате,
/// поэтому вердикт требует только сохранения, а сработала ли — пишет в отчёт.
pub const STEP_SINGLE_FLASH_AUTO: &str = "single_flash_auto";
/// Шаг: серия быстрых нажатий без RAW.
pub const STEP_BURST: &str = "burst";

/// Что делает шаг.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// Одно нажатие при указанных настройках RAW и вспышки.
    Single { raw: RawCapture, flash: FlashMode },
    /// Серия нажатий (число и интервал — из конфига) при выключенных RAW и вспышке.
    Burst,
}

/// Шаг плана.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlannedStep {
    pub name: &'static str,
    pub kind: StepKind,
}

impl PlannedStep {
    /// Какая настройка RAW нужна шагу.
    pub fn required_raw(&self) -> RawCapture {
        match self.kind {
            StepKind::Single { raw, .. } => raw,
            StepKind::Burst => RawCapture::Off,
        }
    }

    /// Какая настройка вспышки нужна шагу.
    pub fn required_flash(&self) -> FlashMode {
        match self.kind {
            StepKind::Single { flash, .. } => flash,
            StepKind::Burst => FlashMode::Off,
        }
    }
}

/// Шаги прогона по порядку.
pub fn planned_steps() -> Vec<PlannedStep> {
    vec![
        PlannedStep {
            name: STEP_SINGLE_HEIF,
            kind: StepKind::Single {
                raw: RawCapture::Off,
                flash: FlashMode::Off,
            },
        },
        PlannedStep {
            name: STEP_SINGLE_HEIF_DNG,
            kind: StepKind::Single {
                raw: RawCapture::On,
                flash: FlashMode::Off,
            },
        },
        PlannedStep {
            name: STEP_SINGLE_FLASH_ON,
            kind: StepKind::Single {
                raw: RawCapture::Off,
                flash: FlashMode::On,
            },
        },
        PlannedStep {
            name: STEP_SINGLE_FLASH_AUTO,
            kind: StepKind::Single {
                raw: RawCapture::Off,
                flash: FlashMode::Auto,
            },
        },
        PlannedStep {
            name: STEP_BURST,
            kind: StepKind::Burst,
        },
    ]
}

/// Сколько нажатий значка вспышки возвращают её в `Off` по кругу `Off → Auto → On → Off` (iOS 26).
pub fn taps_to_flash_off(flash: FlashMode) -> usize {
    let mut mode = flash;
    let mut taps = 0;
    while mode != FlashMode::Off {
        mode = mode.next_in_cycle();
        taps += 1;
    }
    taps
}

#[cfg(test)]
#[path = "capture_plan_tests.rs"]
mod tests;
