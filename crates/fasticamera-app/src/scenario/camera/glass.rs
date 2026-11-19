//! Сценарий `glass` (сессия 11): стекло поверх живого видоискателя.
//!
//! Ход: общий запуск и замер видоискателя ([`SmokeRun`] — тот же, что у `viewfinder`; длина замера =
//! `camera.scenarios.glass.duration_ms`) → на тике, когда видоискатель установился, подписка на гравитацию,
//! установка демо-сцены [`GlassShowcase`] источником кадра UI в поток рендера и стадия `glass_live` (на ней
//! скрипт снимает скриншоты) → окно наблюдения → снятие источника, `detail_glass` и результат с вердиктом
//! [`evaluate_glass`]. Рендер сценарий не регулирует, кроме слоя UI: источник ставится и снимается командами
//! `GpuServices`, а при любом конце сценария его снимает и маршрутизатор.

use shell_contract::gravity::{GravitySlot, GravitySubscription};
use std::sync::Arc;
use ui_kit::tokens::UiKitTokens;

use render_loop::ui_overlay::UiOverlayDiagnostics;

use super::detail::{emit_details, to_value};
use super::glass_report::{
    GlassObservation, GravityReport, OverlayWindow, SourceRemoval, UiGpuBudget, evaluate_glass,
    gravity_samples_received, gravity_source_name,
};
use super::glass_showcase::GlassShowcase;
use super::result_budget::finish_within_budget;
use super::viewfinder_smoke::{RunStep, SmokeRun};
use crate::glass_scenario_config::GlassScenarioConfig;
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::viewfinder_control::ControlDelivery;
use crate::viewfinder_scenario_config::{ViewfinderScenarioConfig, ViewfinderSmokeConfig};

/// Подписка на гравитацию и число показаний на её старте (датчик живёт, пока жива подписка).
struct GravityWatch {
    subscription: GravitySubscription,
    samples_at_install: u64,
}

/// Что известно о гравитации в этом прогоне.
enum GravityState {
    Watching(GravityWatch),
    Unavailable { reason: String },
}

impl GravityState {
    /// Подписаться; отказ — не провал, а причина в отчёте.
    fn subscribe(context: &ScenarioContext<'_>) -> Self {
        match context.services.motion.start_gravity() {
            Ok(subscription) => {
                let samples_at_install = gravity_samples_received(subscription.slot());
                Self::Watching(GravityWatch {
                    subscription,
                    samples_at_install,
                })
            }
            Err(error) => Self::Unavailable {
                reason: error.to_string(),
            },
        }
    }

    /// Ящик гравитации для сцены (`None` — подписки нет, свет в покое).
    fn slot(&self) -> Option<Arc<GravitySlot>> {
        match self {
            Self::Watching(watch) => Some(Arc::clone(watch.subscription.slot())),
            Self::Unavailable { .. } => None,
        }
    }

    /// Строка отчёта на конец окна.
    fn report(&self) -> GravityReport {
        match self {
            Self::Watching(watch) => GravityReport::Active {
                source: gravity_source_name(watch.subscription.source()),
                samples_in_window: gravity_samples_received(watch.subscription.slot())
                    .saturating_sub(watch.samples_at_install),
            },
            Self::Unavailable { reason } => GravityReport::Unavailable { reason: reason.clone() },
        }
    }
}

/// Слой UI, установленный сценарием: счётчики рендера на момент установки и гравитация.
struct InstalledOverlay {
    baseline: UiOverlayDiagnostics,
    gravity: GravityState,
}

/// Состояние прогона.
struct GlassRun {
    smoke: SmokeRun,
    config: GlassScenarioConfig,
    smoke_config: ViewfinderSmokeConfig,
    /// `None` — источник ещё не ставился (видоискатель устанавливается).
    overlay: Option<InstalledOverlay>,
}

/// Сценарий `glass`.
#[derive(Default)]
pub struct GlassScenario {
    run: Option<GlassRun>,
}

impl GlassScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Итог `ControlDelivery` для строки «почему не доставлено».
fn undelivered_reason(delivery: &ControlDelivery) -> Option<String> {
    match delivery {
        ControlDelivery::Delivered => None,
        ControlDelivery::RenderGone => Some("поток рендера уже завершён".to_owned()),
        ControlDelivery::RenderUnavailable { reason } => Some(format!("рендер не поднимался: {reason}")),
    }
}

/// Снимает источник UI и сообщает, дошла ли команда до потока рендера.
fn remove_overlay(context: &ScenarioContext<'_>) -> SourceRemoval {
    match undelivered_reason(&context.gpu.remove_ui_overlay_for_scenario()) {
        None => SourceRemoval::Delivered,
        Some(reason) => SourceRemoval::NotDelivered { reason },
    }
}

impl GlassRun {
    /// Ставит демо-сцену источником кадра UI. Вызывается один раз, на тике, когда видоискатель стал живым.
    fn install_overlay(&mut self, context: &mut ScenarioContext<'_>) -> Result<(), String> {
        let baseline = self
            .smoke
            .last_presentation()
            .and_then(|presentation| presentation.viewfinder.as_ref())
            .map(|viewfinder| viewfinder.ui_overlay.clone())
            .ok_or_else(|| "в статусе рендера нет диагностики видоискателя: слой UI не наблюдаем".to_owned())?;
        let gravity = GravityState::subscribe(context);
        let showcase = GlassShowcase::new(self.config.showcase.clone(), &UiKitTokens::default(), gravity.slot())
            .map_err(|error| format!("демо-сцена стекла не создана: {error}"))?;
        let delivery = context.gpu.install_ui_overlay_for_scenario(Box::new(showcase));
        if let Some(reason) = undelivered_reason(&delivery) {
            return Err(format!("источник UI не установлен: {reason}"));
        }
        self.overlay = Some(InstalledOverlay { baseline, gravity });
        context.emit_stage(
            "glass_live",
            stage_fields([
                ("duration_ms", self.config.duration_ms.into()),
                ("gravity", to_value(&self.overlay_gravity_report())),
            ]),
        );
        Ok(())
    }

    fn overlay_gravity_report(&self) -> GravityReport {
        self.overlay.as_ref().map_or_else(
            || GravityReport::Unavailable {
                reason: "слой UI не установлен".to_owned(),
            },
            |overlay| overlay.gravity.report(),
        )
    }

    /// Конец окна: снять источник, собрать наблюдение.
    fn into_observation(
        self,
        context: &ScenarioContext<'_>,
        running: &crate::camera_service::RunningCamera,
        failure: Option<super::engine_watch::RunFailure>,
    ) -> GlassObservation {
        let after = self
            .smoke
            .last_presentation()
            .and_then(|presentation| presentation.viewfinder.as_ref())
            .map(|viewfinder| viewfinder.ui_overlay.clone());
        let (overlay_window, gravity, removal) = match &self.overlay {
            Some(installed) => {
                // Окно кончилось: убираем стекло с экрана и фиксируем, дошла ли команда.
                let removal = remove_overlay(context);
                let window = after
                    .map(|after| OverlayWindow::between(&installed.baseline, &after))
                    .unwrap_or_default();
                (window, installed.gravity.report(), removal)
            }
            None => (
                OverlayWindow::default(),
                GravityReport::Unavailable {
                    reason: "слой UI не устанавливался".to_owned(),
                },
                SourceRemoval::NotInstalled,
            ),
        };
        let refresh_hz = f64::from(context.config.preferred_frame_rate_hz);
        GlassObservation {
            budget: UiGpuBudget::of(&self.config, refresh_hz),
            overlay: overlay_window,
            gravity,
            removal,
            viewfinder: self.smoke.into_observation(context, running, failure),
        }
    }
}

impl ObservingScenario for GlassScenario {
    fn name(&self) -> &'static str {
        "glass"
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let running = match camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let glass_config = camera.scenarios().glass.clone();
        // Окно замера видоискателя — окно наблюдения за стеклом.
        let viewfinder_config = ViewfinderScenarioConfig {
            smoke: crate::viewfinder_scenario_config::ViewfinderSmokeConfig {
                measure_ms: glass_config.duration_ms,
                ..camera.scenarios().viewfinder.smoke.clone()
            },
            ..camera.scenarios().viewfinder.clone()
        };
        let smoke_config = viewfinder_config.smoke.clone();
        let smoke = match SmokeRun::begin(context, running, viewfinder_config) {
            Ok(smoke) => smoke,
            Err(reason) => return ScenarioStep::Finished(failed(reason)),
        };
        let refresh_hz = f64::from(context.config.preferred_frame_rate_hz);
        context.emit_stage(
            "started",
            stage_fields([
                ("duration_ms", glass_config.duration_ms.into()),
                ("ui_gpu_budget_ms", glass_config.ui_gpu_budget_ms(refresh_hz).into()),
            ]),
        );
        self.run = Some(GlassRun {
            smoke,
            config: glass_config,
            smoke_config,
            overlay: None,
        });
        ScenarioStep::Continue
    }

    fn on_event(&mut self, _event: ObservedEvent, _context: &mut ScenarioContext<'_>) -> ScenarioStep {
        ScenarioStep::Continue
    }

    fn on_tick(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let (Ok(running), Some(run)) = (camera.running(), self.run.as_mut()) else {
            return ScenarioStep::Continue;
        };
        let step = run.smoke.tick(context, running);
        // Видоискатель установился — ставим стекло (один раз). Дальше окно идёт само.
        if run.overlay.is_none()
            && run.smoke.is_measuring()
            && let Err(reason) = run.install_overlay(context)
        {
            return ScenarioStep::Finished(failed(reason));
        }
        let RunStep::Finish(failure) = step else {
            return ScenarioStep::Continue;
        };
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let (config, smoke_config) = (finished.config.clone(), finished.smoke_config.clone());
        let observation = finished.into_observation(context, running, failure);
        emit_details(context, "detail_glass", observation.detail_items());
        let violations = evaluate_glass(&config, &smoke_config, &observation);
        let summary = observation.summary(&config);
        ScenarioStep::Finished(finish_within_budget(context, summary, violations))
    }
}
