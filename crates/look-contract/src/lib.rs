//! Нейтральный словарь вида картинки Fasticamera (решения 98, 99, 178; колёса цвета — 186, 191; основа тона в пресете — 195).
//!
//! Здесь только типы и проверка значений, никакой логики вида: формулы живут в `isp-model`,
//! хранилище пресетов — в `look-presets`, экран только шлёт [`LookIntent`].
//!
//! Принцип: непроверенное значение не существует. Ручка, имя полосы и имя пресета — newtype
//! с `try_from`, поэтому и экран, и файл на диске проходят одну и ту же проверку.
//! Crate — лист графа: не зависит ни от одного crate-а проекта.

mod active;
mod bands;
mod control;
mod error;
mod facts;
mod fingerprint;
mod intent;
mod knob;
mod preset;
mod primaries;
mod recipe;
mod settings;
mod tone_base;
mod wheels;
mod white_stops;

pub use active::{ActiveLook, EditState};
pub use bands::{BandAdjustment, BandAdjustments, BandAdjustmentsError, HueBandId, HueBandIdError};
pub use control::{BandAxis, GlobalKnob, LookControl, WheelPart};
pub use error::{PresetError, StorageFailure};
pub use facts::{
    BandFacts, BandSwatch, LookFacts, LookProblem, LookProblemKind, MIN_WHEEL_RING_STOPS, WheelHueRing,
    WheelHueRingError,
};
pub use intent::{CleanView, LookIntent};
pub use knob::{LookKnob, LookKnobError};
pub use preset::{
    BuiltinPreset, PRESET_NAME_MAX_CHARS, PresetEntry, PresetId, PresetKind, PresetName, PresetNameError, UserPresetId,
};
pub use primaries::{ChannelPrimaries, ChannelTriple, PrimaryChannel};
pub use recipe::LookRecipe;
pub use settings::LookSettings;
pub use tone_base::ToneBaseModel;
pub use wheels::{ColorWheel, ColorWheels, WheelPush, WheelPushError, WheelZone};
pub use white_stops::{
    KnobSideStops, KnobStop, KnobTravelStops, LookWhiteStops, LookWhiteStopsBoard, StopFraction, StopFractionError,
};
