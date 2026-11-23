//! Нейтральный словарь вида картинки Fasticamera (решения 98, 99, 178; колёса цвета — 186, 191).
//!
//! Здесь только типы и проверка значений, никакой логики вида: формулы живут в `isp-model`,
//! хранилище пресетов — в `look-presets`, экран только шлёт [`LookIntent`].
//!
//! Принцип: непроверенное значение не существует. Ручка, имя полосы и имя пресета — newtype
//! с `try_from`, поэтому и экран, и файл на диске проходят одну и ту же проверку.
//! Crate — лист графа: не зависит ни от одного crate-а проекта.

mod active;
mod bands;
mod error;
mod intent;
mod knob;
mod preset;
mod settings;
mod wheels;

pub use active::{ActiveLook, EditState};
pub use bands::{BandAdjustment, BandAdjustments, BandAdjustmentsError, HueBandId, HueBandIdError};
pub use error::{PresetError, StorageFailure};
pub use intent::LookIntent;
pub use knob::{LookKnob, LookKnobError};
pub use preset::{BuiltinPreset, PRESET_NAME_MAX_CHARS, PresetId, PresetName, PresetNameError, UserPresetId};
pub use settings::LookSettings;
pub use wheels::{ColorWheel, ColorWheels, WheelPush, WheelPushError, WheelZone};
