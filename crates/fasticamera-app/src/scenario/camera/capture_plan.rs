//! План шагов сценария `capture` (сессия 14): что снимаем и в каком порядке.
//!
//! Порядок важен: «HEIF + DNG» включает RAW и возвращает его, а серия идёт СЛЕДОМ и снимает без RAW — по
//! настройке запроса серии вердикт доказывает, что RAW действительно вернулся.

use camera_screen::settings::RawCapture;

/// Шаг: один снимок без RAW.
pub const STEP_SINGLE_HEIF: &str = "single_heif";
/// Шаг: один снимок с RAW (два файла: HEIC и DNG).
pub const STEP_SINGLE_HEIF_DNG: &str = "single_heif_dng";
/// Шаг: серия быстрых нажатий без RAW.
pub const STEP_BURST: &str = "burst";

/// Что делает шаг.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    /// Одно нажатие при указанной настройке RAW.
    Single { raw: RawCapture },
    /// Серия нажатий (число и интервал — из конфига) при выключенном RAW.
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
            StepKind::Single { raw } => raw,
            StepKind::Burst => RawCapture::Off,
        }
    }
}

/// Шаги прогона по порядку.
pub fn planned_steps() -> Vec<PlannedStep> {
    vec![
        PlannedStep {
            name: STEP_SINGLE_HEIF,
            kind: StepKind::Single { raw: RawCapture::Off },
        },
        PlannedStep {
            name: STEP_SINGLE_HEIF_DNG,
            kind: StepKind::Single { raw: RawCapture::On },
        },
        PlannedStep {
            name: STEP_BURST,
            kind: StepKind::Burst,
        },
    ]
}
