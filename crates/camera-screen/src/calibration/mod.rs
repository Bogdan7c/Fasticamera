//! Раздел калибровки (решения 185, 199–203): режим CALIBRATE с ручками вида, приборами и сборкой пресета.
//!
//! Модуль владеет ВЫБОРОМ экрана в разделе ([`CalibrationState`]) и ГЕОМЕТРИЕЙ зон ([`CalibrationLayout`]).
//! Редьюсер — `reduce/calibration.rs` (ручки, приборы, «чистяк») и `reduce/presets.rs` (пресеты и ввод имени);
//! значения ручек живут у библиотеки пресетов и приходят фактами (`look_view`).

mod config;
mod eye;
mod knob_stops_config;
mod layout;
mod layout_config;
mod scope_frames;
mod scope_labels_view;
mod scope_strip_frames;
mod state;
mod wheels_frames;
mod widgets_config;

pub use config::{CalibrationConfig, CalibrationConfigError};
pub use knob_stops_config::KnobStopsLookConfig;
pub use layout::{CalibrationLayout, CalibrationLayoutError, CalibrationZones};
pub use layout_config::{
    CalibrationLayoutConfig, CalibrationLayoutConfigError, CapsuleLine, KnobsSide, OrientationLayoutConfig, ZoneShape,
};
pub use scope_strip_frames::ScopeStripFrames;
pub use state::{
    CalibrationState, CleanViewState, ControlViewKind, DeleteConfirmation, Naming, PendingText, SectionCard, TextFlow,
};
pub use wheels_frames::WheelPadPlacement;
pub(crate) use wheels_frames::{ColumnFrames, WheelsFrames, wheel_pad_placement};
pub use widgets_config::{CalibrationWidgetsConfig, CalibrationWidgetsConfigError};

#[cfg(test)]
#[path = "config_tests.rs"]
mod config_tests;
#[cfg(test)]
#[path = "layout_bottom_row_tests.rs"]
mod layout_bottom_row_tests;
#[cfg(test)]
#[path = "layout_scope_tests.rs"]
mod layout_scope_tests;
#[cfg(test)]
#[path = "layout_tests.rs"]
mod layout_tests;
