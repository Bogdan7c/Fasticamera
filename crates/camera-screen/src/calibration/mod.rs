//! Раздел калибровки (решения 185, 199–203): режим CALIBRATE с ручками вида, приборами и сборкой пресета.
//!
//! Модуль владеет ВЫБОРОМ экрана в разделе ([`CalibrationState`]) и ГЕОМЕТРИЕЙ зон ([`CalibrationLayout`]).
//! Редьюсер — `reduce/calibration.rs` (ручки, приборы, «чистяк») и `reduce/presets.rs` (пресеты и ввод имени);
//! значения ручек живут у библиотеки пресетов и приходят фактами (`look_view`).

mod config;
mod layout;
mod state;
mod wheels_frames;
mod widgets_config;

pub use config::{CalibrationConfig, CalibrationConfigError, CalibrationLayoutConfig, ZoneFractions};
pub use layout::{CalibrationLayout, CalibrationLayoutError, CalibrationZones};
pub use state::{
    CalibrationMenu, CalibrationMenuItem, CalibrationState, CleanViewState, ControlViewKind, DeleteConfirmation,
    Naming, PendingText, TextFlow,
};
pub use wheels_frames::WheelPadPlacement;
pub(crate) use wheels_frames::{ColumnFrames, WheelsFrames, wheel_pad_placement};
pub use widgets_config::{CalibrationWidgetsConfig, CalibrationWidgetsConfigError};

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
#[cfg(test)]
#[path = "layout_tests.rs"]
mod layout_tests;
