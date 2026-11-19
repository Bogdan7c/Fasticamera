//! Наш AE на стороне движка камеры (сессия 08a): исполнитель [`RawStatsExposure`].
//!
//! Раскладка:
//! * `config` — настройки (`OwnExposureConfig`, `FlickerPolicy`);
//! * `exposure` — сам исполнитель (`ExposureController`): принятое состояние и его смена;
//! * `plan` — чистое построение `ExposurePlan::Sensor` из принятого состояния и контекста;
//! * `split` — чистый делёж «выдержка × ISO» с кратностью периоду мерцания и пределами;
//! * `flicker` — ручная защита от мерцания (период кратности выдержки; автодетекта нет, решение 77);
//! * `proposal_gate` — отсев предложений прошлого потока.
//!
//! Словарь границы с рендером — `camera_contract::auto_exposure`.

mod config;
mod exposure;
mod flicker;
mod plan;
mod proposal_gate;
mod split;

pub use config::{FlickerPolicy, OwnExposureConfig};
pub use exposure::RawStatsExposure;

#[cfg(test)]
mod exposure_tests;
#[cfg(test)]
mod flicker_tests;
#[cfg(test)]
mod runtime_tests;
#[cfg(test)]
mod split_tests;
