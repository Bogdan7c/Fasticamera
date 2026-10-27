//! Чистая логика замера тока батареи для сценариев (сессия 07, решение 45): без ввода-вывода,
//! без часов и платформы — только показания `BatterySample` и моменты `CLOCK_MONOTONIC`, которые
//! передал сценарий. Поэтому всё проверяется на ПК синтетическими потоками показаний.
//!
//! * [`window`] — одно окно: средний ток разряда, расход заряда, температура, дыры; отказ при
//!   внешнем питании.
//! * [`ab_experiment`] — A/B-замер вклада стадии по схеме ABBA (почему ABBA — там), вывод
//!   `Measured` / `Indistinguishable` («не различим») / `Rejected`.
//! * [`endurance`] — десятиминутный прогон: снимок раз в минуту (ток, заряд, тепло, места под
//!   частоту и время ISP на GPU), сводка в бюджете строки результата и строки `detail_*`.
//! * [`config`] — секция `camera.scenarios.power` и проверенные [`config::PowerSettings`].
//!
//! Как сценарий ведёт замер (сами сценарии `viewfinder*` появятся, когда будет диагностика рендера):
//! 1. `let settings = config.settings()?;` и `services.power.start_sampling(settings.sampling)`;
//! 2. на старте — `AbPowerExperiment::new(&settings, monotonic_now()?)` (или
//!    `EnduranceRecorder::start(..., ThermalSnapshot::read(&*services.thermal))`);
//! 3. на каждом тике: `sampling.drain_samples(...)`, затем `on_tick(monotonic_now()?, &samples)`;
//!    для A/B — применить `AbStep::Measure { stage, .. }` к ISP; `Finished` — взять `report()`.
//!
//! Первое показание на внешнем питании отвергает окно, и A/B-замер кончается отказом сразу.

pub mod ab_experiment;
pub mod config;
pub mod endurance;
pub mod window;

pub use ab_experiment::{AbPowerExperiment, AbPowerReport, AbRejection, AbStep, Contribution, StageSetting};
pub use config::{PowerMeasurementConfig, PowerMeasurementConfigError, PowerSettings};
pub use endurance::{EnduranceRecorder, EnduranceReport, EnduranceStep, MinuteObservation, ThermalSnapshot};
pub use window::{PowerWindowAccumulator, WindowMeasurement, WindowRejection, WindowSpan};

#[cfg(test)]
mod test_support;
