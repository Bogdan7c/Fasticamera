//! Каркас шагов сценария `3a`: окружение шага, трейт шага и общие помощники.
//!
//! Сценарий двигается по тикам, ничего не блокируя: на каждом тике шаг получает [`StepEnv`] (снимок
//! движка, события тика, журнал результатов HAL, показ рендера) и возвращает [`StepProgress`]. Шаг
//! сам отправляет свои команды (`begin`), сам ждёт (`poll`) и сам кладёт итог в наблюдение. Таймаут шага
//! — НЕ обрыв прогона: шаг записывает то, что видел, и отдаёт управление, а вердикт скажет, чего не хватило.
//! Обрыв (`Failed`) — только когда команду отверг движок: дальше идти нельзя.

use std::time::Instant;

use camera_core::command::CommandId;
use camera_core::{CameraEvent, CameraStatus, CommandRejected};
use raw_frame_stream::RawStreamCounters;
use shell_contract::operator_prompt::OperatorPromptService;

use super::engine_watch::{RunFailure, rejected_failure};
use super::three_a_report::ThreeAObservation;
use super::three_a_results::ResultLog;
use super::viewfinder_session::SessionStage;
use crate::camera_presentation::CameraPresentation;
use crate::camera_service::RunningCamera;
use crate::scenario::observing::stage_fields;
use crate::three_a_scenario_config::ThreeAScenarioConfig;

/// Счётчики на момент начала сценария: отсчёт для «с начала» в отчёте.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StartBaselines {
    /// Скрыто кадров на холодных стартах AWB за сессию показа к началу сценария.
    pub hidden_frames: u64,
    /// Принудительных показов к началу сценария.
    pub forced_shows: u64,
    /// Предел скрытых кадров из конфига рендера (`viewfinder.white_balance.max_hidden_frames`).
    pub max_hidden_frames: u32,
}

/// Всё, что шаг видит на одном тике.
pub struct StepEnv<'tick> {
    pub now: Instant,
    pub running: &'tick RunningCamera,
    pub status: &'tick CameraStatus,
    /// События 3A этого тика (итог тапа, поправки, итог серии).
    pub events: &'tick [CameraEvent],
    pub presentation: &'tick CameraPresentation,
    pub results: &'tick ResultLog,
    pub config: &'tick ThreeAScenarioConfig,
    pub baselines: StartBaselines,
    pub raw_counters: RawStreamCounters,
    /// Подсказки владельцу на экране: шаги, где человек должен вмешаться (закрыть объектив), подают сигнал.
    pub prompts: &'tick dyn OperatorPromptService,
    /// Стадии тика (выводит сценарий: у него `&mut ScenarioContext`).
    pub stages: &'tick mut Vec<SessionStage>,
}

impl StepEnv<'_> {
    /// Добавляет строку стадии `FASTICAMERA_STAGE`.
    pub fn stage<const N: usize>(&mut self, stage: &'static str, pairs: [(&str, serde_json::Value); N]) {
        self.stages.push(SessionStage {
            stage,
            fields: stage_fields(pairs),
        });
    }
}

/// Куда пришёл шаг.
#[derive(Debug)]
pub enum StepProgress {
    /// Ждём дальше.
    Pending,
    /// Итог шага записан в наблюдение.
    Done,
    /// Дальше идти нельзя (команду отверг движок).
    Failed(RunFailure),
}

/// Шаг сценария `3a`.
pub trait ThreeAStep: Send {
    /// Имя для логов и тестов.
    fn name(&self) -> &'static str;

    /// Один раз при входе в шаг: отправить команды, запомнить точки отсчёта.
    fn begin(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> Result<(), RunFailure>;

    /// Каждый тик, пока шаг не `Done`/`Failed`.
    fn poll(&mut self, env: &mut StepEnv<'_>, observation: &mut ThreeAObservation) -> StepProgress;
}

/// Команда движка → результат шага: отказ очереди становится обрывом с именем команды.
pub fn sent(name: &'static str, result: Result<CommandId, CommandRejected>) -> Result<CommandId, RunFailure> {
    result.map_err(|rejected| rejected_failure(name, rejected))
}

/// Значение конфигурации сценария, не прошедшее конструктор контракта (в штатном прогоне недостижимо:
/// конфиг проверен, но тихо терять ошибку нельзя).
pub fn invalid_value(command: &'static str, error: impl std::fmt::Display) -> RunFailure {
    RunFailure::CommandRejected {
        command,
        rejected: format!("значение из конфига не принято контрактом: {error}"),
    }
}
