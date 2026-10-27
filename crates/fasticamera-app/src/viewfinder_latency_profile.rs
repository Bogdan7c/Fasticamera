//! Профили сценария `viewfinder-latency` (сессия 07, P13): набор настроек рендера, который сценарий
//! ставит на время одного окна A/B.
//!
//! Профиль = политика линии кадра + поздний захват + сход на ближнюю линию + режим ADPF + политика
//! подачи кадров видоискателя (`LatestWins` или джиттер-буфер). Сценарий сравнивает ровно два
//! профиля: первый в списке — база (`baseline`, A), второй — проверяемый (`tuned`, B). Имена —
//! только подписи для отчёта; роль профиля задаёт его место в списке.

use raw_frame_stream::ViewfinderPacing;
use render_loop::config::{AdpfMode, DepthReductionMode, LateLatchMode, PresentTimelinePolicy};
use serde::{Deserialize, Serialize};

use crate::viewfinder_control::PresentTargetSetting;

/// Сколько профилей сравнивает сценарий.
pub const LATENCY_PROFILE_COUNT: usize = 2;
/// Наибольшая длина имени профиля, символов: имя повторяется в строке результата и в сообщениях.
pub const MAX_LATENCY_PROFILE_NAME_CHARS: usize = 16;

/// Один профиль окна A/B.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LatencyProfileConfig {
    /// Подпись профиля в отчёте: непустая, до [`MAX_LATENCY_PROFILE_NAME_CHARS`] символов.
    pub name: String,
    /// Политика линии кадра окна.
    pub policy: PresentTimelinePolicy,
    /// Поздний захват окна.
    pub late_latch: LateLatchMode,
    /// Сход на ближнюю линию кадра пропуском vsync (решение 54) в окне.
    pub depth_reduction: DepthReductionMode,
    /// Режим подсказок производительности системе (ADPF) окна.
    pub adpf: AdpfMode,
    /// Политика подачи кадров видоискателя в окне (решение 53): ставится на потоке кадров, а не в рендере.
    pub viewfinder_pacing: ViewfinderPacing,
}

impl LatencyProfileConfig {
    /// Политика, поздний захват и сход на ближнюю линию профиля — одной настройкой, как принимает рендер.
    pub fn present_target_setting(&self) -> PresentTargetSetting {
        PresentTargetSetting {
            policy: self.policy,
            late_latch: self.late_latch,
            depth_reduction: self.depth_reduction,
        }
    }
}

/// Запас `NearestFeasible` сверх оценки стоимости кадра по умолчанию, мкс (как пример в конфиге рендера).
pub const DEFAULT_NEAREST_MARGIN_US: u64 = 1_500;

/// Профили по умолчанию: `baseline` — старое поведение до решений 53–54 (Preferred, без позднего
/// захвата и схода на ближнюю линию, без ADPF, `LatestWins`); `tuned` — всё новое сразу
/// (NearestFeasible + поздний захват + `SkipOneVsync` + ADPF + джиттер-буфер).
pub fn default_latency_profiles() -> Vec<LatencyProfileConfig> {
    vec![
        LatencyProfileConfig {
            name: "baseline".to_owned(),
            policy: PresentTimelinePolicy::Preferred,
            late_latch: LateLatchMode::Disabled,
            depth_reduction: DepthReductionMode::Disabled,
            adpf: AdpfMode::Disabled,
            viewfinder_pacing: ViewfinderPacing::LatestWins,
        },
        LatencyProfileConfig {
            name: "tuned".to_owned(),
            policy: PresentTimelinePolicy::NearestFeasible {
                margin_us: DEFAULT_NEAREST_MARGIN_US,
            },
            late_latch: LateLatchMode::BeforeCameraPoll,
            depth_reduction: DepthReductionMode::SkipOneVsync,
            adpf: AdpfMode::Enabled,
            viewfinder_pacing: ViewfinderPacing::JitterBuffer,
        },
    ]
}

/// Что не так со списком профилей.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LatencyProfilesError {
    #[error("viewfinder.latency.profiles: нужно ровно {LATENCY_PROFILE_COUNT} профиля, а не {count}")]
    WrongCount { count: usize },
    #[error("viewfinder.latency.profiles[{index}].name пустое")]
    EmptyName { index: usize },
    #[error("viewfinder.latency.profiles[{index}].name длиннее {MAX_LATENCY_PROFILE_NAME_CHARS} символов")]
    NameTooLong { index: usize },
    #[error("viewfinder.latency.profiles: имя {name:?} повторяется")]
    DuplicateName { name: String },
}

/// Проверка списка: два профиля, имена непустые, короткие и разные.
pub fn validate_latency_profiles(profiles: &[LatencyProfileConfig]) -> Result<(), LatencyProfilesError> {
    if profiles.len() != LATENCY_PROFILE_COUNT {
        return Err(LatencyProfilesError::WrongCount { count: profiles.len() });
    }
    for (index, profile) in profiles.iter().enumerate() {
        if profile.name.trim().is_empty() {
            return Err(LatencyProfilesError::EmptyName { index });
        }
        if profile.name.chars().count() > MAX_LATENCY_PROFILE_NAME_CHARS {
            return Err(LatencyProfilesError::NameTooLong { index });
        }
    }
    if profiles[0].name == profiles[1].name {
        return Err(LatencyProfilesError::DuplicateName {
            name: profiles[0].name.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
#[path = "viewfinder_latency_profile_tests.rs"]
mod tests;
