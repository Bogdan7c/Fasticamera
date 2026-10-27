//! Переключение политики линии кадра и позднего захвата на ходу (сессия 07).
//!
//! Зачем: A/B задержки «сенсор → экран» нужно мерить в ОДНОМ прогоне при одинаковых условиях
//! (тепло, свет, режим экрана). Перезапуск приложения ради другой политики меняет условия, поэтому
//! политика и поздний захват переключаются командой через `RenderInbox::present_target_command`.
//! Значение на старте по-прежнему из конфига (`present_target`); команда меняет только выбор линии,
//! режим захвата и сход на ближнюю линию, остальные настройки секции (окна, запасы ожидания) остаются конфигом.
//!
//! # Что делает применённая команда
//! Поток рендера заменяет политику в планировщике и ОБНУЛЯЕТ всё, что копилось при прежней политике
//! (см. `PresentTargetTracker::apply_command`): окно оценки стоимости кадра, окна диагностики
//! (время в acquire, ожидание захвата, захват → сабмит), счётчики выбора линии и гистограмму
//! опоздания. Записи о показе кадров, спланированных ещё при прежней политике, молча
//! отбрасываются. Так снимок `RenderStatus.present_target` после команды описывает только новую
//! фазу, и два снимка (до и после) сравниваются напрямую.

use serde::{Deserialize, Serialize};

use crate::config::{
    DepthReductionMode, LateLatchMode, PresentTargetConfig, PresentTargetConfigError, PresentTimelinePolicy,
};

/// Действующий выбор линии кадра и режима позднего захвата.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PresentTargetSelection {
    /// Политика выбора линии кадра.
    pub policy: PresentTimelinePolicy,
    /// Ждать ли момента позднего захвата перед опросом камеры.
    pub late_latch: LateLatchMode,
    /// Сход на ближнюю линию пропуском vsync (решение 54; только при `NearestFeasible`).
    pub depth_reduction: DepthReductionMode,
}

impl PresentTargetSelection {
    /// Выбор, записанный в конфиге.
    pub fn of_config(config: &PresentTargetConfig) -> Self {
        Self {
            policy: config.policy,
            late_latch: config.late_latch,
            depth_reduction: config.depth_reduction,
        }
    }
}

/// Команда выбору линии кадра. JSON: `{"kind": "set", "policy": {...}, "late_latch": "...",
/// "depth_reduction": "..."}` (`depth_reduction` можно не указывать — значение по умолчанию).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PresentTargetCommand {
    /// Выбирать линии по `policy`, поздний захват — `late_latch`.
    Set {
        /// Политика выбора линии кадра.
        policy: PresentTimelinePolicy,
        /// Режим позднего захвата.
        late_latch: LateLatchMode,
        /// Сход на ближнюю линию пропуском vsync.
        #[serde(default)]
        depth_reduction: DepthReductionMode,
    },
}

impl PresentTargetCommand {
    /// Какой выбор просит команда.
    pub fn selection(&self) -> PresentTargetSelection {
        match *self {
            Self::Set {
                policy,
                late_latch,
                depth_reduction,
            } => PresentTargetSelection {
                policy,
                late_latch,
                depth_reduction,
            },
        }
    }

    /// Конфиг после команды, проверенный ТЕМ ЖЕ правилом, что конфиг на старте
    /// (`PresentTargetConfig::validate`): например, запас `margin_us` сверх секунды отвергается.
    pub(crate) fn applied_to(
        &self,
        current: &PresentTargetConfig,
    ) -> Result<PresentTargetConfig, PresentTargetConfigError> {
        let selection = self.selection();
        let candidate = PresentTargetConfig {
            policy: selection.policy,
            late_latch: selection.late_latch,
            depth_reduction: selection.depth_reduction,
            ..current.clone()
        };
        candidate.validate()?;
        Ok(candidate)
    }
}

/// Чем закончилась команда.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresentTargetCommandOutcome {
    /// Выбор изменён; окна и счётчики выбора линии обнулены (новая фаза замера).
    Applied,
    /// Ровно этот выбор уже действовал: ничего не изменилось и не обнулилось.
    AlreadyInEffect,
    /// Команда не прошла проверку конфига (причина в тексте); прежний выбор остался.
    Rejected {
        /// Почему.
        reason: String,
    },
}

/// Последняя команда и её итог.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentTargetCommandRecord {
    /// Что просили.
    pub command: PresentTargetCommand,
    /// Что вышло.
    pub outcome: PresentTargetCommandOutcome,
}

/// Учёт команд выбора линии кадра в статусе потока.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PresentTargetCommandLog {
    /// Сколько команд применено.
    pub applied: u64,
    /// Сколько команд ничего не изменили.
    pub already_in_effect: u64,
    /// Сколько команд отвергнуто.
    pub rejected: u64,
    /// Последняя команда (`None` — команд не было).
    pub last: Option<PresentTargetCommandRecord>,
}

impl PresentTargetCommandLog {
    /// Записывает итог команды.
    pub fn note(&mut self, command: PresentTargetCommand, outcome: PresentTargetCommandOutcome) {
        match &outcome {
            PresentTargetCommandOutcome::Applied => self.applied += 1,
            PresentTargetCommandOutcome::AlreadyInEffect => self.already_in_effect += 1,
            PresentTargetCommandOutcome::Rejected { .. } => self.rejected += 1,
        }
        self.last = Some(PresentTargetCommandRecord { command, outcome });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MAX_PRESENT_TARGET_DURATION_US;

    #[test]
    fn command_is_read_from_tagged_json() {
        let command: PresentTargetCommand = serde_json::from_str(
            r#"{"kind": "set", "policy": {"kind": "nearest_feasible", "margin_us": 1500}, "late_latch": "before_camera_poll"}"#,
        )
        .expect("json");
        assert_eq!(
            command.selection(),
            PresentTargetSelection {
                policy: PresentTimelinePolicy::NearestFeasible { margin_us: 1_500 },
                late_latch: LateLatchMode::BeforeCameraPoll,
                depth_reduction: DepthReductionMode::default(),
            }
        );
        assert!(
            serde_json::from_str::<PresentTargetCommand>(
                r#"{"kind": "set", "policy": {"kind": "preferred"}, "late_latch": "disabled", "extra": 1}"#
            )
            .is_err(),
            "лишние ключи — ошибка, как в конфиге"
        );
    }

    #[test]
    fn command_is_validated_by_the_config_rule_and_keeps_other_settings() {
        let current = PresentTargetConfig {
            latch_spin_us: 321,
            ..PresentTargetConfig::default()
        };
        let fine = PresentTargetCommand::Set {
            policy: PresentTimelinePolicy::NearestFeasible { margin_us: 2_000 },
            late_latch: LateLatchMode::BeforeCameraPoll,
            depth_reduction: DepthReductionMode::default(),
        };
        let applied = fine.applied_to(&current).expect("допустимая команда");
        assert_eq!(
            applied.policy,
            PresentTimelinePolicy::NearestFeasible { margin_us: 2_000 }
        );
        assert_eq!(applied.late_latch, LateLatchMode::BeforeCameraPoll);
        assert_eq!(applied.latch_spin_us, 321, "остальные настройки — из текущего конфига");

        let absurd = PresentTargetCommand::Set {
            policy: PresentTimelinePolicy::NearestFeasible {
                margin_us: MAX_PRESENT_TARGET_DURATION_US + 1,
            },
            late_latch: LateLatchMode::Disabled,
            depth_reduction: DepthReductionMode::default(),
        };
        assert!(matches!(
            absurd.applied_to(&current),
            Err(PresentTargetConfigError::DurationOutOfRange {
                field: "policy.margin_us",
                ..
            })
        ));
    }

    #[test]
    fn log_counts_each_outcome_and_keeps_the_last_command() {
        let mut log = PresentTargetCommandLog::default();
        let command = PresentTargetCommand::Set {
            policy: PresentTimelinePolicy::Preferred,
            late_latch: LateLatchMode::Disabled,
            depth_reduction: DepthReductionMode::default(),
        };
        log.note(command, PresentTargetCommandOutcome::Applied);
        log.note(command, PresentTargetCommandOutcome::AlreadyInEffect);
        let rejected = PresentTargetCommandOutcome::Rejected {
            reason: "причина".to_owned(),
        };
        log.note(command, rejected.clone());
        assert_eq!((log.applied, log.already_in_effect, log.rejected), (1, 1, 1));
        assert_eq!(
            log.last,
            Some(PresentTargetCommandRecord {
                command,
                outcome: rejected
            })
        );
    }
}
