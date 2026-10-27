//! Автозапуск видоискателя при старте приложения (сессия 07) — ВРЕМЕННАЯ политика подключения.
//!
//! Решение владельца: запуск приложения сразу показывает живой видоискатель — основная камера
//! (роль `wide`), 4:3@60 RAW10, тот же режим, что у сценария `raw-stream`. До сессии 12 (экран
//! камеры) у продукта нет владельца «что сейчас на экране», поэтому это решение живёт здесь, в
//! маленьком отдельном модуле composition root, и уйдёт отсюда, когда экран камеры возьмёт его себе.
//!
//! Что модуль делает и чего НЕ делает:
//! * ставит движку **желаемое** состояние (`open` → `configure` → `start_streaming`) ровно один раз,
//!   когда движок камеры поднялся ([`ViewfinderAutostart::start_once`] из `camera_wiring`);
//! * после сценария, который заявлял права на камеру (он её закрывает и открывает свою), желаемое
//!   состояние автозапуска возвращается ([`ViewfinderAutostart::restore_after_scenario`]); сценарии,
//!   которые камеры не касались, не трогаются вовсе;
//! * передний план / фон и восстановление после вытеснения — НЕ здесь: это делает сам движок
//!   (решение 39). Желаемое состояние в фоне сохраняется, поэтому `Paused`/`Resumed` команд
//!   автозапуска не повторяют.
//!
//! Конфиг — `camera.viewfinder_autostart` ([`ViewfinderAutostartConfig`]).

use std::sync::atomic::{AtomicBool, Ordering};

use camera_contract::ids::CameraTarget;
use camera_contract::stream::{InvalidFrameRate, StreamMode, TargetFrameRate};
use camera_core::{CameraHandle, CommandId, CommandRejected};
use serde::{Deserialize, Serialize};

use crate::camera_scenario_config::{
    LensRoleSetting, RawFormatSetting, ReadoutSetting, ResolvedStep, SCENARIO_CAPTURE_INTENT,
};
use crate::camera_service::CameraUnavailable;

const LOG_TARGET: &str = "fasticamera::camera::viewfinder_autostart";

/// Режим потока видоискателя в конфиге.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderModeConfig {
    /// Формат RAW. RAW10 — рабочий формат продукта (решение 21).
    pub raw_format: RawFormatSetting,
    /// Какую часть сенсора читать. 4:3 — полный кадр сенсора, как у фото-видоискателя (решение 23).
    pub readout: ReadoutSetting,
    /// Целевая частота, к/с. 60 — цель видоискателя (решение 36, подтверждено в 04–05).
    pub frame_rate: u32,
}

impl Default for ViewfinderModeConfig {
    fn default() -> Self {
        // Тот же режим, что у сценария `raw-stream` (`RawStreamScenarioConfig::default`): на нём
        // в 05 подтверждены 5 минут без сбросов и CPU-копий.
        Self {
            raw_format: RawFormatSetting::Raw10,
            readout: ReadoutSetting::FourByThree,
            frame_rate: 60,
        }
    }
}

/// Секция `camera.viewfinder_autostart`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ViewfinderAutostartConfig {
    /// Открывать ли видоискатель при старте. По умолчанию да (решение владельца в 07); `false` —
    /// приложение стартует с закрытой камерой, как до сессии 07 (камеру открывают только сценарии).
    pub enabled: bool,
    /// Какую камеру открывать по роли. Основная (`wide`).
    pub role: LensRoleSetting,
    /// Режим потока.
    pub mode: ViewfinderModeConfig,
}

impl Default for ViewfinderAutostartConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            role: LensRoleSetting::Wide,
            mode: ViewfinderModeConfig::default(),
        }
    }
}

/// Неверная секция `camera.viewfinder_autostart`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ViewfinderAutostartConfigError {
    #[error("camera.viewfinder_autostart.mode.frame_rate: {0}")]
    InvalidFrameRate(#[from] InvalidFrameRate),
}

impl ViewfinderAutostartConfig {
    /// Цель и режим в словаре контракта. Проверяется и при выключенном автозапуске: опечатка в
    /// частоте не должна «всплыть» только в день, когда автозапуск включат.
    pub fn resolve(&self) -> Result<ResolvedStep, ViewfinderAutostartConfigError> {
        Ok(ResolvedStep {
            target: CameraTarget::Role(self.role.to_contract()),
            mode: StreamMode {
                raw_format: self.mode.raw_format.to_contract(),
                readout: self.mode.readout.to_contract(),
                frame_rate: TargetFrameRate::new(self.mode.frame_rate)?,
            },
        })
    }

    pub fn validate(&self) -> Result<(), ViewfinderAutostartConfigError> {
        self.resolve().map(|_| ())
    }
}

/// Какая из команд автозапуска не ушла.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AutostartCommand {
    Open,
    Configure,
    StartStreaming,
}

/// Номера команд, которыми выставлено желаемое состояние видоискателя.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewfinderCommands {
    pub open: CommandId,
    pub configure: CommandId,
    pub start_streaming: CommandId,
}

/// Чем кончилась попытка выставить желаемое состояние видоискателя. Не `bool`: «выключено
/// конфигом», «уже ставили», «движка нет» и отказ очереди — разные ответы для лога и тестов.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewfinderAutostartOutcome {
    /// Автозапуск выключен конфигом: команд не было.
    Disabled,
    /// Стартовые команды уже отправлялись в этом процессе: повтор — не их работа (фон/передний
    /// план ведёт движок).
    AlreadyStarted,
    /// Движка камеры нет (не поднялся): слать некому, причина — в логе.
    EngineUnavailable,
    /// Команды приняты очередью движка (итог каждой движок сообщит событием `CommandHandled`).
    DesiredStateSent(ViewfinderCommands),
    /// Очередь движка отвергла команду; следующие не отправлялись.
    Rejected {
        command: AutostartCommand,
        rejected: CommandRejected,
    },
}

/// Что включено в этой сборке/конфиге.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AutostartPlan {
    Disabled,
    Enabled(ResolvedStep),
}

/// Владелец политики автозапуска: план из конфига и признак «стартовые команды уже ушли».
#[derive(Debug)]
pub struct ViewfinderAutostart {
    plan: AutostartPlan,
    /// `true` после первой попытки старта (любым итогом): старт — ровно один раз на процесс.
    start_attempted: AtomicBool,
}

impl ViewfinderAutostart {
    /// Автозапуск выключен (сервис без конфига, тесты, «камера недоступна»).
    pub fn disabled() -> Self {
        Self::with_plan(AutostartPlan::Disabled)
    }

    /// План из конфига. Ошибка — неверная частота.
    pub fn from_config(config: &ViewfinderAutostartConfig) -> Result<Self, ViewfinderAutostartConfigError> {
        let resolved = config.resolve()?;
        let plan = if config.enabled {
            AutostartPlan::Enabled(resolved)
        } else {
            AutostartPlan::Disabled
        };
        Ok(Self::with_plan(plan))
    }

    fn with_plan(plan: AutostartPlan) -> Self {
        Self {
            plan,
            start_attempted: AtomicBool::new(false),
        }
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self.plan, AutostartPlan::Enabled(_))
    }

    /// Выставляет желаемое состояние видоискателя один раз за процесс. Движок в этот момент может
    /// быть в фоне: команды меняют только желаемое, камера откроется по `Resumed`.
    pub fn start_once(&self, engine: Result<&CameraHandle, CameraUnavailable>) -> ViewfinderAutostartOutcome {
        let AutostartPlan::Enabled(step) = &self.plan else {
            tracing::info!(target: LOG_TARGET, "автозапуск видоискателя выключен конфигом");
            return ViewfinderAutostartOutcome::Disabled;
        };
        if self.start_attempted.swap(true, Ordering::SeqCst) {
            return ViewfinderAutostartOutcome::AlreadyStarted;
        }
        let outcome = match engine {
            Ok(handle) => send_desired_viewfinder(handle, step),
            Err(unavailable) => {
                tracing::error!(
                    target: LOG_TARGET,
                    reason = %unavailable.reason,
                    "камера недоступна: видоискатель при старте не открыт"
                );
                ViewfinderAutostartOutcome::EngineUnavailable
            }
        };
        log_outcome(outcome, "start");
        outcome
    }

    /// Сценарий, заявлявший права на камеру, закончился, и вызывающий уже отправил `close` (он
    /// стирает желаемое сценария, включая ручные настройки): вернуть желаемое состояние
    /// видоискателя. Выключенный автозапуск ничего не шлёт — камера остаётся закрытой, как до 07.
    pub fn restore_after_scenario(
        &self,
        engine: Result<&CameraHandle, CameraUnavailable>,
    ) -> ViewfinderAutostartOutcome {
        let AutostartPlan::Enabled(step) = &self.plan else {
            return ViewfinderAutostartOutcome::Disabled;
        };
        let outcome = match engine {
            Ok(handle) => send_desired_viewfinder(handle, step),
            Err(_) => ViewfinderAutostartOutcome::EngineUnavailable,
        };
        log_outcome(outcome, "restore_after_scenario");
        outcome
    }
}

/// `open` → `configure` → `start_streaming`; первая отвергнутая команда останавливает цепочку
/// (следующие без неё бессмысленны и получили бы `InvalidState`).
fn send_desired_viewfinder(handle: &CameraHandle, step: &ResolvedStep) -> ViewfinderAutostartOutcome {
    match send_commands(handle, step) {
        Ok(commands) => ViewfinderAutostartOutcome::DesiredStateSent(commands),
        Err((command, rejected)) => ViewfinderAutostartOutcome::Rejected { command, rejected },
    }
}

fn send_commands(
    handle: &CameraHandle,
    step: &ResolvedStep,
) -> Result<ViewfinderCommands, (AutostartCommand, CommandRejected)> {
    let open = handle
        .open(step.target.clone())
        .map_err(|rejected| (AutostartCommand::Open, rejected))?;
    let configure = handle
        .configure(step.mode)
        .map_err(|rejected| (AutostartCommand::Configure, rejected))?;
    let start_streaming = handle
        .start_streaming(SCENARIO_CAPTURE_INTENT)
        .map_err(|rejected| (AutostartCommand::StartStreaming, rejected))?;
    Ok(ViewfinderCommands {
        open,
        configure,
        start_streaming,
    })
}

fn log_outcome(outcome: ViewfinderAutostartOutcome, when: &'static str) {
    match outcome {
        ViewfinderAutostartOutcome::DesiredStateSent(_) => {
            tracing::info!(target: LOG_TARGET, when, "желаемое состояние видоискателя выставлено");
        }
        ViewfinderAutostartOutcome::Rejected { command, rejected } => {
            tracing::error!(target: LOG_TARGET, when, ?command, %rejected, "движок не принял команду видоискателя");
        }
        ViewfinderAutostartOutcome::EngineUnavailable => {
            tracing::error!(target: LOG_TARGET, when, "движка камеры нет: видоискатель не запущен");
        }
        ViewfinderAutostartOutcome::Disabled | ViewfinderAutostartOutcome::AlreadyStarted => {}
    }
}

#[cfg(test)]
#[path = "viewfinder_autostart_tests.rs"]
mod tests;
