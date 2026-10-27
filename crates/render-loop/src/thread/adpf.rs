//! Отчёты системе о работе кадров через ADPF (`APerformanceHint`, P13 сессии 07).
//!
//! Владелец: поток рендера. [`AdpfReporter`] получает службу [`PerformanceHintService`] при запуске
//! потока (как источник vsync) и САМ создаёт сессию на своём потоке при первом показанном кадре —
//! в сессию входит именно поток рендера (`current_thread` вызывается на нём). Сессия закрывается
//! выключением (команда) или остановкой потока (`Drop`).
//!
//! # Что и когда сообщается
//! Каждый ПОКАЗАННЫЙ кадр: начало периода — момент начала кадра потоком; CPU — время потока в
//! `render_and_present` за вычетом ожиданий (`acquire_frame` и момента позднего захвата: в них поток
//! спит, это не работа); GPU — время команд кадра по меткам GPU. GPU приходит на 1–3 кадра позже,
//! поэтому кадр ждёт его в очереди до `gpu_report_wait_frames` кадров и отчитывается вместе с ним;
//! не дождался — отчёт только по CPU (GPU = «не измерено»). Отчёт с опозданием на пару кадров
//! документация ADPF допускает: каждый отчёт несёт своё начало периода.
//! Общая длительность = CPU + GPU: GPU начинает кадр после сабмита, модель последовательная —
//! чуть завышенная оценка лишь слегка поднимает частоты.
//!
//! # Отказы
//! Ничто здесь не блокирует и не роняет рендер. Сессия не создалась — честное состояние
//! `Unsupported`/`Failed` с причиной, повтор только по новой команде включения (без шторма попыток
//! на каждом кадре). Отказ отчёта или смены цели — счётчик и последняя ошибка, сессия живёт дальше.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use shell_contract::clock::MonotonicNanos;
use shell_contract::performance_hint::{FrameWork, FrameWorkSession, PerformanceHintError, PerformanceHintService};

use crate::backend::{FrameCpuTiming, FrameGpuTiming};
use crate::config::{AdpfConfig, AdpfMode};

const LOG_TARGET: &str = "fasticamera::render::adpf";

/// Относительное изменение цели, при котором она отправляется системе заново: период экрана
/// дрожит на единицы мкс, переотправлять цель из-за этого незачем.
const TARGET_UPDATE_MIN_CHANGE: f64 = 0.01;

/// Состояние сессии ADPF.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum AdpfSessionState {
    /// ADPF выключен (конфиг или команда): сессии нет.
    #[default]
    Disabled,
    /// Включён, сессия создастся на первом показанном кадре.
    Pending,
    /// Сессия действует.
    Active,
    /// Платформа подсказки не поддерживает (причина в `last_error`); повтор — новой командой.
    Unsupported,
    /// Создание сессии не удалось (причина в `last_error`); повтор — новой командой.
    Failed,
}

/// Снимок учёта ADPF (часть `RenderStatus`). Счётчики накопительные за жизнь потока: сценарии
/// берут приращения.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdpfDiagnostics {
    /// Действующий режим.
    pub mode: AdpfMode,
    /// Состояние сессии.
    pub session: AdpfSessionState,
    /// Действующая цель длительности кадра (`None` — сессии нет).
    pub target: Option<Duration>,
    /// Сколько сессий создано.
    pub sessions_created: u64,
    /// Сколько раз создание сессии не удалось (включая «не поддерживается»).
    pub session_create_failures: u64,
    /// Сколько раз цель отправлена заново (смена периода экрана).
    pub target_updates: u64,
    /// Сколько смен цели не удалось.
    pub target_update_failures: u64,
    /// Сколько отчётов о кадрах принято системой.
    pub reports_sent: u64,
    /// Из них с временем GPU.
    pub reports_with_gpu: u64,
    /// Из них только по CPU (время GPU не пришло за `gpu_report_wait_frames` кадров).
    pub reports_cpu_only: u64,
    /// Отчёты, которые система отвергла.
    pub report_failures: u64,
    /// Кадры, чьи числа не годились для отчёта (нулевая длительность и т. п.) — не отправлялись.
    pub invalid_frames: u64,
    /// Последняя ошибка (создание, цель или отчёт).
    pub last_error: Option<String>,
}

/// Команда ADPF. JSON: `{"kind": "set", "mode": "enabled"}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdpfCommand {
    /// Включить или выключить ADPF.
    Set {
        /// Новый режим.
        mode: AdpfMode,
    },
}

/// Чем закончилась команда ADPF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdpfCommandOutcome {
    /// Режим изменён (включение после отказа сессии — тоже: это новая попытка создать сессию).
    Applied,
    /// Этот режим уже действовал, ничего не изменилось.
    AlreadyInEffect,
}

/// Учёт команд ADPF в статусе потока.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdpfCommandLog {
    /// Сколько команд применено.
    pub applied: u64,
    /// Сколько команд ничего не изменили.
    pub already_in_effect: u64,
    /// Последняя команда и её итог.
    pub last: Option<(AdpfCommand, AdpfCommandOutcome)>,
}

impl AdpfCommandLog {
    /// Записывает итог команды.
    pub fn note(&mut self, command: AdpfCommand, outcome: AdpfCommandOutcome) {
        match outcome {
            AdpfCommandOutcome::Applied => self.applied += 1,
            AdpfCommandOutcome::AlreadyInEffect => self.already_in_effect += 1,
        }
        self.last = Some((command, outcome));
    }
}

/// Замер одного показанного кадра потоком рендера (без GPU).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PresentedFrameWork {
    pub frame_index: u64,
    /// Когда поток начал кадр (`CLOCK_MONOTONIC`).
    pub period_start: MonotonicNanos,
    /// Работа CPU кадра (без ожиданий acquire и позднего захвата).
    pub cpu: Duration,
}

impl PresentedFrameWork {
    /// Собирает замер кадра: `started`/`finished` — часы потока вокруг `render_and_present`,
    /// `waits` — замер бэкенда (ожидания вычитаются). Часы не прочитались — замера нет (`None`):
    /// выдуманные числа системе отправлять нельзя.
    pub(super) fn measure(
        frame_index: u64,
        started: Option<MonotonicNanos>,
        finished: Option<MonotonicNanos>,
        waits: Option<&FrameCpuTiming>,
    ) -> Option<Self> {
        let (started, finished) = (started?, finished?);
        let elapsed = Duration::from_nanos(u64::try_from(finished.as_nanos().saturating_sub(started.as_nanos())).ok()?);
        let waited = waits.map_or(Duration::ZERO, |timing| {
            timing.acquire_wait.saturating_add(timing.latch_wait)
        });
        Some(Self {
            frame_index,
            period_start: started,
            cpu: elapsed.saturating_sub(waited),
        })
    }
}

/// Сессия и её цель.
struct ActiveSession {
    session: Box<dyn FrameWorkSession>,
    target: Duration,
}

/// Отчёты ADPF потока рендера (см. модуль).
pub(super) struct AdpfReporter {
    service: Arc<dyn PerformanceHintService>,
    config: AdpfConfig,
    session: Option<ActiveSession>,
    /// Показанные кадры, ждущие времени GPU (по возрастанию номера).
    awaiting_gpu: VecDeque<PresentedFrameWork>,
    diagnostics: AdpfDiagnostics,
}

impl AdpfReporter {
    pub(super) fn new(config: AdpfConfig, service: Arc<dyn PerformanceHintService>) -> Self {
        let session = match config.mode {
            AdpfMode::Disabled => AdpfSessionState::Disabled,
            AdpfMode::Enabled => AdpfSessionState::Pending,
        };
        Self {
            service,
            diagnostics: AdpfDiagnostics {
                mode: config.mode,
                session,
                ..AdpfDiagnostics::default()
            },
            config,
            session: None,
            awaiting_gpu: VecDeque::new(),
        }
    }

    /// Снимок для статуса.
    pub(super) fn snapshot(&self) -> AdpfDiagnostics {
        self.diagnostics.clone()
    }

    /// Применяет команду: выключение закрывает сессию и забывает ждущие кадры; включение после
    /// отказа — новая попытка создать сессию на следующем кадре.
    pub(super) fn apply_command(&mut self, command: AdpfCommand) -> AdpfCommandOutcome {
        let AdpfCommand::Set { mode } = command;
        let retry_after_failure = mode == AdpfMode::Enabled
            && matches!(
                self.diagnostics.session,
                AdpfSessionState::Unsupported | AdpfSessionState::Failed
            );
        if mode == self.config.mode && !retry_after_failure {
            return AdpfCommandOutcome::AlreadyInEffect;
        }
        self.config.mode = mode;
        self.diagnostics.mode = mode;
        match mode {
            AdpfMode::Disabled => {
                // Drop сессии закрывает её у системы.
                self.session = None;
                self.awaiting_gpu.clear();
                self.diagnostics.session = AdpfSessionState::Disabled;
                self.diagnostics.target = None;
            }
            AdpfMode::Enabled => self.diagnostics.session = AdpfSessionState::Pending,
        }
        AdpfCommandOutcome::Applied
    }

    /// Показанный кадр: при включённом ADPF — создать сессию (первый кадр), поправить цель под
    /// период экрана, поставить кадр ждать время GPU и отчитаться за кадры, которые ждали слишком долго.
    pub(super) fn frame_presented(&mut self, frame: PresentedFrameWork, refresh_period: Duration) {
        if self.config.mode == AdpfMode::Disabled {
            return;
        }
        let target = self.target_for(refresh_period);
        if self.diagnostics.session == AdpfSessionState::Pending {
            self.create_session(target);
        }
        if self.session.is_none() {
            return;
        }
        self.update_target_if_changed(target);
        self.awaiting_gpu.push_back(frame);
        while let Some(oldest) = self.awaiting_gpu.front().copied() {
            let waited = frame.frame_index.saturating_sub(oldest.frame_index);
            if waited < u64::try_from(self.config.gpu_report_wait_frames).unwrap_or(u64::MAX) {
                break;
            }
            self.awaiting_gpu.pop_front();
            self.report(oldest, None);
        }
    }

    /// Время кадра на GPU пришло: отчёт за этот кадр с GPU. Более старые ждущие кадры своего GPU
    /// уже не получат (замеры идут по возрастанию номера) — отчёт за них только по CPU.
    pub(super) fn gpu_timing(&mut self, timing: FrameGpuTiming) {
        if self.session.is_none() {
            return;
        }
        while let Some(oldest) = self.awaiting_gpu.front().copied() {
            if oldest.frame_index > timing.frame_index {
                // Кадр не был показан (или до включения ADPF): отчитываться не за что.
                return;
            }
            self.awaiting_gpu.pop_front();
            if oldest.frame_index == timing.frame_index {
                self.report(oldest, Some(timing.gpu_duration));
                return;
            }
            self.report(oldest, None);
        }
    }

    fn target_for(&self, refresh_period: Duration) -> Duration {
        refresh_period.mul_f64(self.config.target_fraction)
    }

    fn create_session(&mut self, target: Duration) {
        let created = self
            .service
            .current_thread()
            .and_then(|thread| self.service.create_session(&[thread], target));
        match created {
            Ok(session) => {
                self.diagnostics.sessions_created += 1;
                self.diagnostics.session = AdpfSessionState::Active;
                self.diagnostics.target = Some(target);
                self.session = Some(ActiveSession { session, target });
                tracing::info!(target: LOG_TARGET, target_us = target.as_micros(), "сессия ADPF создана");
            }
            Err(error) => {
                self.diagnostics.session_create_failures += 1;
                self.diagnostics.session = match error {
                    PerformanceHintError::Unsupported { .. } => AdpfSessionState::Unsupported,
                    PerformanceHintError::Failed { .. } | PerformanceHintError::InvalidWork(_) => {
                        AdpfSessionState::Failed
                    }
                };
                tracing::warn!(target: LOG_TARGET, %error, "сессия ADPF не создана: рендер идёт без подсказок");
                self.diagnostics.last_error = Some(error.to_string());
            }
        }
    }

    fn update_target_if_changed(&mut self, target: Duration) {
        let Some(active) = self.session.as_mut() else {
            return;
        };
        let current = active.target.as_secs_f64();
        let change = (target.as_secs_f64() - current).abs() / current.max(f64::MIN_POSITIVE);
        if change < TARGET_UPDATE_MIN_CHANGE {
            return;
        }
        match active.session.update_target(target) {
            Ok(()) => {
                active.target = target;
                self.diagnostics.target = Some(target);
                self.diagnostics.target_updates += 1;
            }
            Err(error) => {
                // Прежняя цель остаётся; следующая попытка — на следующем кадре (вызов дешёвый).
                self.diagnostics.target_update_failures += 1;
                self.diagnostics.last_error = Some(error.to_string());
            }
        }
    }

    fn report(&mut self, frame: PresentedFrameWork, gpu: Option<Duration>) {
        let Some(active) = self.session.as_mut() else {
            return;
        };
        let total = frame.cpu.saturating_add(gpu.unwrap_or(Duration::ZERO));
        let work = match FrameWork::new(frame.period_start, total, frame.cpu, gpu) {
            Ok(work) => work,
            Err(invalid) => {
                self.diagnostics.invalid_frames += 1;
                self.diagnostics.last_error = Some(invalid.to_string());
                return;
            }
        };
        match active.session.report_frame(&work) {
            Ok(()) => {
                self.diagnostics.reports_sent += 1;
                match gpu {
                    Some(_) => self.diagnostics.reports_with_gpu += 1,
                    None => self.diagnostics.reports_cpu_only += 1,
                }
            }
            Err(error) => {
                self.diagnostics.report_failures += 1;
                self.diagnostics.last_error = Some(error.to_string());
            }
        }
    }
}

#[cfg(test)]
#[path = "adpf_tests.rs"]
mod tests;
