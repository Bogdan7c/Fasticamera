//! Поминутная запись длинных прогонов (`viewfinder-endurance`, `video-headroom-bench`): ток и
//! заряд батареи ([`EnduranceRecorder`]) плюс то, что сценарий видит сам в конце минуты.
//!
//! Без источника батареи прогон не отменяется: тепло, частота и время GPU важны сами по себе, а
//! минуты без показаний честно помечаются отказом тока внутри отчёта (`no_current_readings`).

use shell_contract::power::{BatterySample, PowerSampling};

use crate::clock_offset::monotonic_now;
use crate::power_measurement::{EnduranceRecorder, EnduranceStep, MinuteObservation, PowerSettings, ThermalSnapshot};
use crate::scenario::observing::ScenarioContext;

/// Запись прогона по минутам.
pub struct MinuteLog {
    recorder: EnduranceRecorder,
    sampling: Option<PowerSampling>,
    power_unavailable: Option<String>,
    samples: Vec<BatterySample>,
}

impl MinuteLog {
    /// Начало прогона «сейчас». Ошибка — не прочитались часы `CLOCK_MONOTONIC`.
    pub fn start(context: &ScenarioContext<'_>, settings: &PowerSettings) -> Result<Self, String> {
        let started_at = monotonic_now().map_err(|error| format!("часы CLOCK_MONOTONIC: {error}"))?;
        let (sampling, power_unavailable) = match context.services.power.start_sampling(settings.sampling) {
            Ok(sampling) => (Some(sampling), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let start = ThermalSnapshot::read(context.services.thermal.as_ref());
        Ok(Self {
            recorder: EnduranceRecorder::start(settings, started_at, start),
            sampling,
            power_unavailable,
            samples: Vec::new(),
        })
    }

    /// Минут в прогоне.
    pub fn total_minutes(&self) -> u32 {
        self.recorder.total_minutes()
    }

    /// Тик: забрать показания, закрыть наступившие минуты (`observe` — по разу на минуту).
    pub fn tick(&mut self, observe: impl FnMut() -> MinuteObservation) -> Result<EnduranceStep, String> {
        self.samples.clear();
        if let Some(sampling) = &self.sampling {
            sampling.drain_samples(&mut self.samples);
        }
        let now = monotonic_now().map_err(|error| format!("часы CLOCK_MONOTONIC: {error}"))?;
        Ok(self.recorder.on_tick(now, &self.samples, observe))
    }

    pub fn recorder(&self) -> &EnduranceRecorder {
        &self.recorder
    }

    /// Почему ток не мерился (`None` — опрос шёл).
    pub fn power_unavailable(&self) -> Option<&str> {
        self.power_unavailable.as_deref()
    }
}
