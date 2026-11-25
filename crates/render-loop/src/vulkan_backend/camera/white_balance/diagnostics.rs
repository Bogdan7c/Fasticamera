//! Снимок диагностики белой: собирается из частей раннера, ничего не меняя.

use camera_contract::control::WhiteBalanceSetting;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv};

use super::WhiteBalanceRunner;
use super::hal_white::{delta_between, reading_of};
use crate::viewfinder::{WhiteBalanceDiagnostics, WhiteReading};

/// Белая цели владельца в виде показаний.
fn reading_of_setting(setting: WhiteBalanceSetting) -> WhiteReading {
    reading_of(CctDuv {
        temperature: Kelvin(setting.temperature.kelvin()),
        duv: Duv(setting.tint.duv()),
    })
}

impl WhiteBalanceRunner {
    /// Снимок диагностики.
    pub fn diagnostics(&self) -> WhiteBalanceDiagnostics {
        let applied = self.applied.applied_point().map(|point| point.cct_duv());
        let source_counters = self.sources.counters();
        WhiteBalanceDiagnostics {
            applied_white: applied.map(reading_of),
            target_white: self.applied.target().map(reading_of_setting),
            transition_active: self.applied.is_transitioning(),
            directive_generation: self.applied_generation.map(|generation| generation.0),
            request: self.request(),
            last_answer: self.sources.last_answer(),
            answers_published: source_counters.answers_published,
            answers_signal_not_bound: source_counters.answers_signal_not_bound,
            source_replies_dropped: source_counters.replies_dropped,
            unconvertible_targets: self.applied.unconvertible_targets(),
            hal_white: self.hal_white.map(reading_of),
            hal_delta: applied.zip(self.hal_white).map(|(ours, hal)| delta_between(ours, hal)),
            cold_start: self.cold_start.state(),
            hidden_frames_cold_start: self.counters.hidden_frames,
            cold_start_forced: self.counters.forced,
            frames_without_directive: self.counters.frames_without_directive,
        }
    }
}
