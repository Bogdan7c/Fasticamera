//! Наблюдение сценариев за показом кадров камеры на экране.
//!
//! Поток рендера хранит счётчики показа в `RenderStatus`; сценариям не нужны ни его внутренности,
//! ни тип дескриптора. Этот модуль превращает снимок рендера в одно намерение: «что сейчас
//! показывается от камеры» ([`GpuServices::observe_camera_presentation`]), где отсутствие рендера и
//! отсутствие кадров камеры — разные ответы, а не один `None`.

use render_loop::backend::PastPresentation;
use render_loop::camera_diagnostics::CameraStreamDiagnostics;
use std::time::Duration;

use render_loop::pacing::PacingSnapshot;
use render_loop::thread::{
    AdpfCommandLog, AdpfDiagnostics, PresentTargetCommandLog, PresentTargetDiagnostics, RenderState,
};
use render_loop::viewfinder::{ViewfinderCommandLog, ViewfinderDiagnostics};

use crate::gpu_services::{GpuServices, RenderObservation};
use crate::scenario::render_metrics::ValidationReportPart;

/// Показ кадров камеры в один момент.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraPresentation {
    /// Состояние потока рендера.
    pub render_state: RenderState,
    /// Накопительные счётчики показа кадров камеры за жизнь бэкенда рендера.
    pub diagnostics: CameraStreamDiagnostics,
    /// Последние реальные времена показа (часы `MONOTONIC`), старые первыми.
    pub recent_presentations: Vec<PastPresentation>,
    /// Частота экрана, по которой судит пейсинг, Гц.
    pub display_refresh_hz: f64,
    /// Validation слоя Vulkan: режим и счётчики.
    pub validation: ValidationReportPart,
    /// Видоискатель (ISP, компоновка, время GPU, стенд запаса); `None` — бэкенд без видоискателя.
    pub viewfinder: Option<ViewfinderDiagnostics>,
    /// Выбор линии кадра: опоздания, откаты, действующая политика (сессия 07).
    pub present_target: PresentTargetDiagnostics,
    /// Итоги команд видоискателя и команд линии кадра — чтобы сценарий видел, что регулировка применена.
    pub viewfinder_commands: ViewfinderCommandLog,
    pub present_target_commands: PresentTargetCommandLog,
    /// Подсказки производительности системе (ADPF): режим, состояние сессии, счётчики отчётов.
    pub adpf: AdpfDiagnostics,
    /// Итоги команд ADPF — чтобы сценарий видел, что режим применён.
    pub adpf_commands: AdpfCommandLog,
    /// Ритм показа (окно пейсинга потока рендера).
    pub pacing: PacingSnapshot,
    /// Период обновления экрана по данным драйвера (`VK_GOOGLE_display_timing`); `None` — неизвестен.
    pub display_refresh_cycle: Option<Duration>,
}

/// Что показывает рендер от камеры.
#[derive(Debug, Clone, PartialEq)]
pub enum CameraPresentationObservation {
    /// Поток рендера не запущен (GPU или vsync не поднялись).
    RenderUnavailable {
        reason: String,
    },
    /// Рендер жив, но не в режиме показа камеры (нет канала кадров камеры).
    NotShowingCamera,
    Live(Box<CameraPresentation>),
}

impl GpuServices {
    /// Снимок показа кадров камеры. Не блокирует: читает опубликованный потоком рендера статус.
    pub fn observe_camera_presentation(&self) -> CameraPresentationObservation {
        match self.render.observe() {
            RenderObservation::Unavailable { reason } => CameraPresentationObservation::RenderUnavailable { reason },
            RenderObservation::Live { status, validation } => {
                let validation = ValidationReportPart::of(&status, validation);
                let period_seconds = status.pacing_refresh_period.as_secs_f64();
                let display_refresh_hz = if period_seconds > 0.0 {
                    1.0 / period_seconds
                } else {
                    0.0
                };
                match status.backend.camera {
                    None => CameraPresentationObservation::NotShowingCamera,
                    Some(diagnostics) => CameraPresentationObservation::Live(Box::new(CameraPresentation {
                        render_state: status.state,
                        diagnostics,
                        recent_presentations: status.recent_presentations,
                        display_refresh_hz,
                        validation,
                        viewfinder: status.backend.viewfinder,
                        present_target: status.present_target,
                        viewfinder_commands: status.viewfinder_commands,
                        present_target_commands: status.present_target_commands,
                        adpf: status.adpf,
                        adpf_commands: status.adpf_commands,
                        pacing: status.pacing,
                        display_refresh_cycle: status.display.and_then(|display| display.refresh_cycle),
                    })),
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "camera_presentation_tests.rs"]
mod tests;
