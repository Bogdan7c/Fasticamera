//! Наблюдающие smoke-сценарии: живут в маршрутизаторе событий и завершаются позже запуска.
//!
//! Сценарий жизненного цикла, разрешения, датчиков и платформы не может ответить сразу:
//! ему нужно дождаться событий оболочки (свернули приложение, ответили в диалоге, прошло
//! три секунды гироскопа). Маршрутизатор передаёт ему каждое событие и тик таймера, а
//! сценарий пишет промежуточные стадии (`FASTICAMERA_STAGE`) и в конце — строку результата.
//!
//! Сценарии зависят только от трейтов `shell-contract` (через [`ShellServices`]), поэтому
//! на ПК проверяются с fake-сервисами.

use std::sync::Arc;
use std::time::Instant;

use serde::Serialize;
use shell_contract::commands::{PlatformCommand, ShellCommands};
use shell_contract::diagnostics::ShellDiagnostics;
use shell_contract::event::{ShellEvent, ShellEventKind};
use shell_contract::haptics::HapticsService;
use shell_contract::motion::{GyroRequest, MotionSource};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::permission::CameraPermission;
use shell_contract::surface::{NativeSurface, SurfaceSize};
use shell_contract::thermal::ThermalSource;
use shell_contract::thermal_policy::ThermalPolicy;

use super::{ScenarioConfig, ScenarioOutcome, ScenarioReport};

/// Состояние подписки на тепловой статус (то, что сценарию `platform` нужно знать о мониторе).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThermalListenerReport {
    Registered,
    Failed { reason: String },
}

impl ThermalListenerReport {
    /// `"registered"` либо `"failed: <причина>"` — контракт с `scripts/smoke_checks.py`.
    pub fn report_text(&self) -> String {
        match self {
            ThermalListenerReport::Registered => "registered".to_owned(),
            ThermalListenerReport::Failed { reason } => format!("failed: {reason}"),
        }
    }
}

/// Факты платформы, которых нет в нейтральных трейтах (нужны только отчётам сценариев).
pub trait PlatformFacts: Send + Sync {
    fn thermal_listener(&self) -> ThermalListenerReport;
    fn has_vibrator(&self) -> bool;
}

/// Сервисы оболочки, которые видят сценарии (на телефоне — один `AndroidShellServices`).
#[derive(Clone)]
pub struct ShellServices {
    pub commands: Arc<dyn ShellCommands>,
    pub haptics: Arc<dyn HapticsService>,
    pub motion: Arc<dyn MotionSource>,
    pub thermal: Arc<dyn ThermalSource>,
    pub diagnostics: Arc<dyn ShellDiagnostics>,
    pub facts: Arc<dyn PlatformFacts>,
}

/// Что маршрутизатор знает об экране. В сессии 02 он же — временный владелец surface
/// (рендера ещё нет): держит её и подтверждает освобождение.
#[derive(Debug, Default)]
pub struct DisplayState {
    resumed: bool,
    surface: Option<NativeSurface>,
    surface_size: Option<SurfaceSize>,
    orientation: Option<DeviceOrientation>,
}

impl DisplayState {
    /// Приложение на переднем плане и у него есть surface — на экране можно рисовать.
    pub fn is_presenting(&self) -> bool {
        self.resumed && self.surface.is_some()
    }

    /// На экране, и размер surface уже известен (`surfaceChanged`) — рендеру есть куда рисовать.
    pub fn is_ready_to_draw(&self) -> bool {
        self.is_presenting() && self.surface_size.is_some()
    }

    pub fn orientation(&self) -> Option<DeviceOrientation> {
        self.orientation
    }

    pub fn surface_size(&self) -> Option<SurfaceSize> {
        self.surface_size
    }

    pub(crate) fn set_resumed(&mut self, resumed: bool) {
        self.resumed = resumed;
    }

    /// Новая surface; прежняя (если Android не прислал destroyed) вытесняется с предупреждением.
    pub(crate) fn surface_created(&mut self, surface: NativeSurface) {
        if let Some(previous) = self.surface.replace(surface) {
            tracing::warn!(
                target: "fasticamera::router",
                previous = previous.generation().value(),
                "новая surface без уничтожения прежней"
            );
        }
        self.surface_size = None;
    }

    pub(crate) fn surface_changed(
        &mut self,
        generation: shell_contract::surface::SurfaceGeneration,
        size: SurfaceSize,
    ) {
        if self
            .surface
            .as_ref()
            .is_some_and(|surface| surface.generation() == generation)
        {
            self.surface_size = Some(size);
        }
    }

    /// Отпускает surface указанного поколения (если она текущая).
    pub(crate) fn release_surface(&mut self, generation: shell_contract::surface::SurfaceGeneration) {
        if self
            .surface
            .as_ref()
            .is_some_and(|surface| surface.generation() == generation)
        {
            self.surface = None;
            self.surface_size = None;
        }
    }

    pub(crate) fn set_orientation(&mut self, orientation: DeviceOrientation) {
        self.orientation = Some(orientation);
    }
}

/// Сводка события для сценария (сами события потребляет маршрутизатор: surface — ресурс).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObservedEvent {
    pub kind: ShellEventKind,
    pub camera_permission: Option<CameraPermission>,
    pub failed_command: Option<PlatformCommand>,
}

impl ObservedEvent {
    pub fn of(event: &ShellEvent) -> Self {
        Self {
            kind: event.kind(),
            camera_permission: match event {
                ShellEvent::CameraPermission(permission) => Some(*permission),
                _ => None,
            },
            failed_command: match event {
                ShellEvent::PlatformCommandFailed(failure) => Some(failure.command),
                _ => None,
            },
        }
    }
}

/// Промежуточная стадия сценария (`FASTICAMERA_STAGE {json}`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StageLine {
    pub scenario: &'static str,
    pub stage: &'static str,
    #[serde(flatten)]
    pub fields: serde_json::Map<String, serde_json::Value>,
}

/// Куда сценарии пишут стадии и итог (на телефоне — logcat, в тестах — сборщик).
pub trait ScenarioOutput {
    fn stage(&mut self, line: StageLine);
    fn result(&mut self, report: ScenarioReport);
}

/// Контекст одного вызова сценария.
pub struct ScenarioContext<'call> {
    pub services: &'call ShellServices,
    pub display: &'call DisplayState,
    pub thermal_policy: &'call ThermalPolicy,
    pub config: &'call ScenarioConfig,
    pub gyro_request: GyroRequest,
    pub now: Instant,
    output: &'call mut dyn ScenarioOutput,
    scenario: &'static str,
}

impl<'call> ScenarioContext<'call> {
    #[expect(
        clippy::too_many_arguments,
        reason = "контекст собирается в одном месте маршрутизатора"
    )]
    pub(crate) fn new(
        services: &'call ShellServices,
        display: &'call DisplayState,
        thermal_policy: &'call ThermalPolicy,
        config: &'call ScenarioConfig,
        gyro_request: GyroRequest,
        now: Instant,
        output: &'call mut dyn ScenarioOutput,
        scenario: &'static str,
    ) -> Self {
        Self {
            services,
            display,
            thermal_policy,
            config,
            gyro_request,
            now,
            output,
            scenario,
        }
    }

    /// Пишет стадию сценария с дополнительными полями.
    pub fn emit_stage(&mut self, stage: &'static str, fields: serde_json::Map<String, serde_json::Value>) {
        tracing::info!(target: "fasticamera::scenario", scenario = self.scenario, stage, "стадия сценария");
        self.output.stage(StageLine {
            scenario: self.scenario,
            stage,
            fields,
        });
    }
}

/// Что сценарий решил после события или тика.
#[derive(Debug)]
pub enum ScenarioStep {
    Continue,
    Finished(ScenarioOutcome),
}

/// Наблюдающий сценарий.
pub trait ObservingScenario: Send {
    fn name(&self) -> &'static str;

    /// Вызывается один раз, когда маршрутизатор принял сценарий.
    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep;

    /// Событие оболочки (уже применённое к [`DisplayState`]).
    fn on_event(&mut self, event: ObservedEvent, context: &mut ScenarioContext<'_>) -> ScenarioStep;

    /// Тик таймера маршрутизатора (для сценариев, которые ждут время).
    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep;
}

/// Поля стадии из пар «имя — значение».
pub fn stage_fields<const N: usize>(
    pairs: [(&str, serde_json::Value); N],
) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
}

/// Итог «прошёл» с отчётом; ошибка сериализации отчёта — честный провал, а не паника.
pub fn passed(report: impl Serialize) -> ScenarioOutcome {
    match serde_json::to_value(report) {
        Ok(report) => ScenarioOutcome::Passed { report },
        Err(error) => ScenarioOutcome::Failed {
            error: format!("отчёт сценария не сериализован: {error}"),
        },
    }
}

/// Итог «упал» с объяснением.
pub fn failed(error: impl Into<String>) -> ScenarioOutcome {
    ScenarioOutcome::Failed { error: error.into() }
}
