//! Сценарий `viewfinder-stage-budget` (сессия 07, решение 45; расширен в 16a): таблица «работа → мс GPU → мА».
//!
//! Ход: общий запуск продуктового видоискателя, выход на место замера ([`StageBudgetSurface`]), затем строки таблицы
//! по порядку конфига (`viewfinder.stage_budget.rows`), затем возврат экрана в PHOTO. Каждая строка — свой A/B-замер
//! тока ([`AbPowerExperiment`], ABBA, `power.ab`): на каждом тике сценарий забирает показания батареи, узнаёт, какое
//! окно идёт и должна ли работа быть включена, применяет состояние рычагов ([`stage_budget_apply`], только при смене
//! положения — команды идемпотентны, но засорять ящик рендера незачем) и в измеряемой части окна наблюдает рендер
//! ([`WindowObserver`]). Окно, чьё состояние диагностика не подтвердила, отвергается (`state_not_applied`,
//! `display_refresh_not_reached`), а не попадает в таблицу как «измерено». Отказ тока (внешнее питание, нет источника)
//! прерывает сценарий сразу: таблица без мА — не таблица решения 45, а шесть минут на строку жалко.
//!
//! # Место замера и политика экрана
//! `bare_viewfinder` — экран камеры на время сценария снят, как до 16a (поведение бит-в-бит прежнее). На
//! `photo_screen` и `calibrate_screen` экран остаётся и получает вход от драйвера. Снимков сценарий не делает, но
//! политика выбрана осознанно: [`CameraScreenPolicy::KeepRunningWithFakeCapture`] — случайное попадание удерживаемого
//! пальца в кнопку затвора дало бы фейк-снимок, а не файл в галерее владельца.
//!
//! Возврат рисовальщика, стадий, маски подавления и частоты к продукту делает маршрутизатор по концу сценария.

use std::time::{Duration, Instant};

use shell_contract::power::{BatterySample, PowerSampling};

use super::detail::{EVENTS_IN_REPORT_LIMIT, emit_details};
use super::engine_watch::RunFailure;
use super::look_report::LookReport;
use super::result_budget::finish_within_budget;
use super::stage_budget_apply::{
    HoldPress, HoldTouch, LeverHistory, apply_work_state, emit_hold_touch, restore_product_levers,
};
use super::stage_budget_look::{LookPlan, plan_for};
use super::stage_budget_states::{RowStates, consecutive_a_windows};
use super::stage_budget_surface::{
    EntryTick, ExitTick, SURFACE_GESTURE_STAGE, SurfaceEntry, SurfaceExit, SurfaceGesture, SurfaceSwipes, SurfaceView,
};
use super::stage_budget_surface_report::{
    CameraWithoutProductGpuRow, ScopesReport, SurfaceReport, backdrop_pyramid_rebuilds_of,
};
use super::stage_budget_window::{HoldIssued, WindowCondition, WindowIssued, WindowObserver};
use super::viewfinder_session::{SessionTick, SessionTiming, ViewfinderSession};
use super::viewfinder_stage_budget_report::{
    CameraWithoutIspRow, GpuWindowSample, RowPower, StageBudgetObservation, StageBudgetRow, evaluate_stage_budget,
};
use crate::camera_presentation::CameraPresentation;
use crate::camera_screen_service::CameraScreenPolicy;
use crate::camera_service::{CameraServices, RunningCamera};
use crate::clock_offset::monotonic_now;
use crate::power_measurement::{AbPowerExperiment, AbStep, Contribution, PowerSettings, StageSetting, ThermalSnapshot};
use crate::scenario::observing::{
    ObservedEvent, ObservingScenario, ScenarioContext, ScenarioStep, failed, stage_fields,
};
use crate::stage_budget_surface_config::StageBudgetSurface;
use crate::viewfinder_scenario_config::{StageBudgetRowSetting, ViewfinderScenarioConfig};

/// Политика экрана камеры для места замера (чистая функция: на ней стоит тест, роутер спрашивает её до `start`).
pub fn screen_policy_for(surface: StageBudgetSurface) -> CameraScreenPolicy {
    if surface.keeps_product_screen() {
        CameraScreenPolicy::KeepRunningWithFakeCapture
    } else {
        CameraScreenPolicy::SuspendForScenario
    }
}

/// Идущая строка.
struct OpenRow {
    row: StageBudgetRowSetting,
    states: RowStates,
    experiment: AbPowerExperiment,
    /// Положение, которое сейчас применено к рендеру (команды идут при смене положения, не окна).
    applied: Option<StageSetting>,
    /// Наблюдатель идущего окна.
    observer: Option<WindowObserver>,
    /// Время GPU закрытых окон.
    windows: Vec<GpuWindowSample>,
    /// Условия закрытых окон (тепло, частота, счётчики, отказ).
    conditions: Vec<WindowCondition>,
}

/// Чем занят сценарий.
enum Phase {
    /// Видоискатель ещё запускается.
    Starting,
    /// Идёт выход на место замера.
    Entering(SurfaceEntry),
    /// Идут строки таблицы.
    Measuring,
    /// Строки кончились: экран возвращается в PHOTO.
    Leaving(SurfaceExit),
}

struct BudgetRun {
    config: ViewfinderScenarioConfig,
    power: PowerSettings,
    session: ViewfinderSession,
    sampling: Option<PowerSampling>,
    phase: Phase,
    swipes: SurfaceSwipes,
    warmup_ui_frames: u64,
    open: Option<OpenRow>,
    next_row: usize,
    rows: Vec<StageBudgetRow>,
    power_refusal: Option<String>,
    samples: Vec<BatterySample>,
    scopes: Option<ScopesReport>,
    /// Вид и основа на входе на место замера (14o).
    look: Option<LookReport>,
    entry_failure: Option<String>,
    exit_failure: Option<String>,
    levers: RunLevers,
}

/// Что сценарий сделал с рычагами за прогон.
struct RunLevers {
    /// Какие рычаги уже трогались (строка обязана вернуть то, что тронула прежняя).
    history: LeverHistory,
    /// До какого момента держится выданное сценарием нажатие пальца (без запаса). Жест выхода раньше не выдаётся.
    hold_until: Option<Instant>,
    /// Недоставленные команды.
    undelivered: Vec<String>,
}

/// Сценарий `viewfinder-stage-budget`.
#[derive(Default)]
pub struct ViewfinderStageBudgetScenario {
    run: Option<BudgetRun>,
}

impl ViewfinderStageBudgetScenario {
    pub fn new() -> Self {
        Self::default()
    }
}

enum RunStep {
    Continue,
    Finish(Option<RunFailure>),
}

impl BudgetRun {
    fn surface(&self) -> StageBudgetSurface {
        self.config.stage_budget.surface
    }

    fn tick(&mut self, context: &mut ScenarioContext<'_>, running: &RunningCamera) -> RunStep {
        let tick = self.session.tick(context, running);
        for stage in self.session.take_stages() {
            context.emit_stage(stage.stage, stage.fields);
        }
        match tick {
            SessionTick::Preparing => RunStep::Continue,
            // Камера сломалась: возвращать на экране нечего, итог по тому, что успели.
            SessionTick::Failed(failure) => RunStep::Finish(Some(failure)),
            SessionTick::BecameLive(presentation) => self.start_power(context, &presentation),
            SessionTick::Live(presentation) => self.advance_phase(context, &presentation),
        }
    }

    /// Запускает опрос батареи и вход на место замера; отказ опроса — честный конец сценария.
    fn start_power(&mut self, context: &mut ScenarioContext<'_>, presentation: &CameraPresentation) -> RunStep {
        match context.services.power.start_sampling(self.power.sampling) {
            Ok(sampling) => {
                self.sampling = Some(sampling);
                let initial = SurfaceView::read(context, presentation);
                let entry = SurfaceEntry::begin(
                    self.surface(),
                    self.swipes,
                    self.config.stage_budget.scope_page,
                    Duration::from_millis(self.config.stage_budget.surface_ready_timeout_ms),
                    self.warmup_ui_frames,
                    context.now,
                    &initial,
                );
                self.phase = Phase::Entering(entry);
                RunStep::Continue
            }
            Err(error) => {
                self.power_refusal = Some(format!("опрос батареи не запущен: {error}"));
                RunStep::Finish(None)
            }
        }
    }

    /// Показания батареи, пока строки не идут (вход и выход): забираем, чтобы кольцо не переполнялось.
    fn discard_samples(&mut self) {
        if let Some(sampling) = &self.sampling {
            self.samples.clear();
            sampling.drain_samples(&mut self.samples);
        }
    }

    fn advance_phase(&mut self, context: &mut ScenarioContext<'_>, presentation: &CameraPresentation) -> RunStep {
        match self.phase {
            Phase::Starting => RunStep::Continue,
            Phase::Entering(_) => self.advance_entry(context, presentation),
            Phase::Measuring => {
                let step = self.advance(context, presentation);
                self.leave_when_finished(context, presentation, step)
            }
            Phase::Leaving(_) => self.advance_exit(context, presentation),
        }
    }

    fn advance_entry(&mut self, context: &mut ScenarioContext<'_>, presentation: &CameraPresentation) -> RunStep {
        self.discard_samples();
        let view = SurfaceView::read(context, presentation);
        let Phase::Entering(entry) = &mut self.phase else {
            return RunStep::Continue;
        };
        match entry.tick(context.now, &view) {
            EntryTick::Waiting => RunStep::Continue,
            EntryTick::Gesture(gesture) => {
                emit_surface_gesture(context, gesture);
                RunStep::Continue
            }
            EntryTick::Ready => {
                self.scopes = presentation.viewfinder.as_ref().map(ScopesReport::of);
                self.look = presentation
                    .viewfinder
                    .as_ref()
                    .map(|viewfinder| LookReport::of(viewfinder, None));
                self.phase = Phase::Measuring;
                let step = self.open_next_row(context);
                self.leave_when_finished(context, presentation, step)
            }
            EntryTick::Failed(reason) => {
                self.entry_failure = Some(reason);
                self.begin_leaving(context, presentation)
            }
        }
    }

    fn advance_exit(&mut self, context: &mut ScenarioContext<'_>, presentation: &CameraPresentation) -> RunStep {
        self.discard_samples();
        let view = SurfaceView::read(context, presentation);
        let Phase::Leaving(exit) = &mut self.phase else {
            return RunStep::Continue;
        };
        match exit.tick(context.now, &view) {
            ExitTick::Waiting => RunStep::Continue,
            ExitTick::Gesture(gesture) => {
                emit_surface_gesture(context, gesture);
                RunStep::Continue
            }
            ExitTick::Done => RunStep::Finish(None),
            ExitTick::Failed(reason) => {
                self.exit_failure = Some(reason);
                RunStep::Finish(None)
            }
        }
    }

    /// Строки кончились (или отказ тока): прежде чем кончить, вернуть экран в PHOTO.
    fn leave_when_finished(
        &mut self,
        context: &mut ScenarioContext<'_>,
        presentation: &CameraPresentation,
        step: RunStep,
    ) -> RunStep {
        match step {
            RunStep::Finish(None) => self.begin_leaving(context, presentation),
            other => other,
        }
    }

    /// Начало возврата: продуктовые рычаги, затем (из CALIBRATE) жест выхода.
    fn begin_leaving(&mut self, context: &mut ScenarioContext<'_>, presentation: &CameraPresentation) -> RunStep {
        if !self.surface().keeps_product_screen() {
            // Голый видоискатель: экран не трогали, рычаги вернёт маршрутизатор (поведение до 16a).
            return RunStep::Finish(None);
        }
        // Жест выхода должен идти при продуктовом состоянии: при подавленном UI экран остался бы «слепым» для глаза
        // владельца, а при подавленных приборах выход не проверялся бы честно.
        restore_product_levers(context, self.levers.history, &mut self.levers.undelivered);
        // Выданное нажатие пальца может ещё держаться (строка оборвана на первом окне): жест выхода — после конца
        // удержания плюс запас, иначе два «пальца» конфликтуют и свайп проглатывается.
        let guard = Duration::from_millis(self.config.stage_budget.display_hold_guard_ms);
        let not_before = self
            .levers
            .hold_until
            .map_or(context.now, |until| (until + guard).max(context.now));
        let exit = SurfaceExit::begin(
            self.surface(),
            self.swipes,
            Duration::from_millis(self.config.stage_budget.surface_ready_timeout_ms),
            context.now,
            not_before,
        );
        self.phase = Phase::Leaving(exit);
        self.advance_exit(context, presentation)
    }

    fn open_next_row(&mut self, context: &mut ScenarioContext<'_>) -> RunStep {
        let Some(row) = self.config.stage_budget.rows.get(self.next_row).copied() else {
            return RunStep::Finish(None);
        };
        let started_at = match monotonic_now() {
            Ok(now) => now,
            Err(error) => {
                self.power_refusal = Some(format!("часы CLOCK_MONOTONIC не прочитаны: {error}"));
                return RunStep::Finish(None);
            }
        };
        self.next_row += 1;
        context.emit_stage(
            "row_started",
            stage_fields([("row", row.name().into()), ("index", (self.next_row - 1).into())]),
        );
        let states = RowStates::of(row);
        self.open = Some(OpenRow {
            row,
            states,
            experiment: AbPowerExperiment::new(&self.power, started_at),
            applied: None,
            observer: None,
            windows: Vec::new(),
            conditions: Vec::new(),
        });
        RunStep::Continue
    }

    fn advance(&mut self, context: &mut ScenarioContext<'_>, presentation: &CameraPresentation) -> RunStep {
        let Some(sampling) = &self.sampling else {
            return RunStep::Continue;
        };
        self.samples.clear();
        sampling.drain_samples(&mut self.samples);
        let now = match monotonic_now() {
            Ok(now) => now,
            Err(error) => {
                self.power_refusal = Some(format!("часы CLOCK_MONOTONIC не прочитаны: {error}"));
                return RunStep::Finish(None);
            }
        };
        let Some(open) = &mut self.open else {
            return RunStep::Continue;
        };
        match open.experiment.on_tick(now, &self.samples) {
            AbStep::Measure { stage, window } => {
                if open.observer.as_ref().map(WindowObserver::index) != Some(window.index) {
                    // Началось новое окно: закрыть прошлое. Если его состояние не подтвердилось, замер кончается
                    // (следующий тик вернёт `Finished`), а новое окно не начинаем.
                    if close_window(open, context) {
                        return RunStep::Continue;
                    }
                    begin_window(
                        open,
                        context,
                        &self.config,
                        &self.power,
                        &mut self.levers,
                        stage,
                        window.index,
                    );
                }
                if !window.warming_up
                    && let Some(observer) = open.observer.as_mut()
                {
                    observer.observe(presentation);
                }
                RunStep::Continue
            }
            AbStep::Finished => {
                // Последнее окно закрывается здесь: отказ по его состоянию перекрывает готовый вывод замера.
                close_window(open, context);
                self.close_row(context)
            }
        }
    }

    /// Строка кончилась: собрать её; отказ тока — прервать сценарий (дальше мерить бессмысленно).
    fn close_row(&mut self, context: &mut ScenarioContext<'_>) -> RunStep {
        let Some(open) = self.open.take() else {
            return RunStep::Continue;
        };
        let power = match open.experiment.report() {
            Some(report) => RowPower::Ab { report: report.clone() },
            None => RowPower::SamplingUnavailable {
                reason: "A/B-замер кончился без отчёта".to_owned(),
            },
        };
        let rejected = match &power {
            // Отказ по состоянию окна объявил сам сценарий: тракт тока исправен, остальные строки мерить можно.
            RowPower::Ab { report } => match &report.contribution {
                Contribution::Rejected { reason } if !reason.is_declared_by_scenario() => Some(format!("{reason:?}")),
                _ => None,
            },
            RowPower::SamplingUnavailable { reason } => Some(reason.clone()),
        };
        let row = StageBudgetRow::assemble(open.row.name(), open.windows, power).with_conditions(open.conditions);
        context.emit_stage(
            "row_finished",
            stage_fields([("row", row.row.into()), ("line", row.table_line().into())]),
        );
        self.rows.push(row);
        if let Some(reason) = rejected {
            self.power_refusal = Some(format!("строка {}: {reason}", open.row.name()));
            return RunStep::Finish(None);
        }
        self.open_next_row(context)
    }

    fn into_observation(
        self,
        running: &RunningCamera,
        gpu_window_frames: usize,
        failure: Option<RunFailure>,
    ) -> StageBudgetObservation {
        let surface = SurfaceReport {
            surface: self.config.stage_budget.surface,
            scopes: self.scopes,
            look: self.look,
            backdrop_pyramid_rebuilds: backdrop_pyramid_rebuilds_of(&self.rows),
            entry_failure: self.entry_failure,
            exit_failure: self.exit_failure,
        };
        StageBudgetObservation {
            failure,
            power_refusal: self.power_refusal,
            session: self.session.summary(running, EVENTS_IN_REPORT_LIMIT),
            requested_rows: self.config.stage_budget.rows.iter().map(|row| row.name()).collect(),
            camera_without_isp: CameraWithoutIspRow::from_rows(&self.rows),
            camera_without_product_gpu: CameraWithoutProductGpuRow::from_rows(&self.rows),
            rows: self.rows,
            surface,
            gpu_window_frames,
            undelivered: self.levers.undelivered,
        }
    }
}

/// Закрывает наблюдателя идущего окна: условия и время GPU — в строку; отказ по состоянию — в эксперимент. Возвращает
/// `true`, если окно отвергнуто (замер кончится).
fn close_window(open: &mut OpenRow, context: &ScenarioContext<'_>) -> bool {
    let Some(observer) = open.observer.take() else {
        return false;
    };
    let thermal_end = ThermalSnapshot::read(context.services.thermal.as_ref());
    let Some(closed) = observer.finish(thermal_end) else {
        return false;
    };
    let refusal = closed.condition.refusal.clone();
    let (index, stage) = (closed.condition.index, closed.condition.stage);
    open.windows.push(closed.sample);
    open.conditions.push(closed.condition);
    match refusal {
        Some(refusal) => {
            open.experiment.reject_window(refusal.to_rejection(index, stage));
            true
        }
        None => false,
    }
}

/// Начинает окно `index`: ПОЛНОЕ состояние рычагов (при смене положения; рычаги, тронутые прежними строками, тоже
/// приводятся к состоянию), удерживаемое касание (в каждом окне A строки с частотой) и наблюдатель.
fn begin_window(
    open: &mut OpenRow,
    context: &mut ScenarioContext<'_>,
    config: &ViewfinderScenarioConfig,
    power: &PowerSettings,
    levers: &mut RunLevers,
    stage: StageSetting,
    index: usize,
) {
    let state = open.states.state_for(stage);
    // План вида окна (14o): рецепт считается для КАЖДОГО окна, чтобы наблюдатель знал, что ждать, а команды уходят
    // только при смене положения.
    let look_plan = state.look.map(|look| plan_for(context, look));
    if open.applied != Some(stage) {
        open.applied = Some(stage);
        let label = format!("строка {}, окно {index}", open.row.name());
        apply_work_state(
            context,
            &open.states,
            levers.history,
            &state,
            look_plan.as_ref(),
            &label,
            &mut levers.undelivered,
        );
        levers.history.note_applied(&open.states, &state);
    }
    let mut issued = WindowIssued {
        requested_hz: state.display.target_hz(),
        hold: None,
    };
    if state.display.holds_touch() {
        // Стадия выдаётся в начале КАЖДОГО окна A. Окно уже включает разогрев, поэтому нажатие покрывает окна и
        // короче их на запас: нажатие начинается с задержкой драйвера и без запаса кончилось бы в разогреве
        // следующего окна B (первые секунды B шли бы на 120 Гц).
        let window_ms = u64::try_from(power.ab.window.as_millis()).unwrap_or(u64::MAX);
        let total = open.experiment.total_windows();
        let previous_is_a = index > 0 && AbPowerExperiment::stage_of(index - 1) == StageSetting::On;
        let covered = consecutive_a_windows(index, total);
        let full_ms = window_ms.saturating_mul(u64::try_from(covered).unwrap_or(u64::MAX));
        let hold_ms = full_ms
            .saturating_sub(config.stage_budget.display_hold_margin_ms)
            .max(1);
        let press = if previous_is_a {
            HoldPress::ContinuesPrevious
        } else {
            HoldPress::Start
        };
        if press == HoldPress::Start {
            levers.hold_until = Some(context.now + Duration::from_millis(hold_ms));
        }
        issued.hold = Some(HoldIssued {
            hold_ms,
            new_press: press == HoldPress::Start,
        });
        emit_hold_touch(
            context,
            HoldTouch {
                row: open.row.name(),
                window: index,
                point: config.stage_budget.display_hold_point,
                hold_ms,
                windows_covered: covered,
                press,
            },
        );
    }
    let thermal_start = ThermalSnapshot::read(context.services.thermal.as_ref());
    open.observer = Some(
        WindowObserver::begin(
            index,
            stage,
            state,
            config.stage_budget.surface,
            config.stage_budget.display_refresh_tolerance_hz,
            issued,
            thermal_start,
        )
        .expecting_look(look_plan.as_ref().map(LookPlan::expected_recipe).cloned()),
    );
}

/// Выдаёт жест входа или выхода драйверу.
fn emit_surface_gesture(context: &mut ScenarioContext<'_>, gesture: SurfaceGesture) {
    context.emit_stage(
        SURFACE_GESTURE_STAGE,
        stage_fields([
            ("step", gesture.step.into()),
            ("purpose", gesture.purpose.into()),
            ("gesture", gesture.to_stage_value()),
        ]),
    );
}

impl ObservingScenario for ViewfinderStageBudgetScenario {
    fn name(&self) -> &'static str {
        "viewfinder-stage-budget"
    }

    /// Экран остаётся на `photo_screen` / `calibrate_screen` (см. модуль); на голом видоискателе он снят.
    fn camera_screen_policy_for(&self, camera: &CameraServices) -> CameraScreenPolicy {
        screen_policy_for(camera.scenarios().viewfinder.stage_budget.surface)
    }

    fn start(&mut self, context: &mut ScenarioContext<'_>) -> ScenarioStep {
        let camera = context.camera;
        let running = match camera.running() {
            Ok(running) => running,
            Err(unavailable) => return ScenarioStep::Finished(failed(unavailable.to_string())),
        };
        let config = camera.scenarios().viewfinder.clone();
        let power = match camera.scenarios().power.settings() {
            Ok(power) => power,
            Err(error) => return ScenarioStep::Finished(failed(format!("camera.scenarios.power: {error}"))),
        };
        let step = match config.product_step() {
            Ok(step) => step,
            Err(error) => return ScenarioStep::Finished(failed(format!("camera.scenarios.viewfinder: {error}"))),
        };
        let surface = config.stage_budget.surface;
        if surface.keeps_product_screen() && !context.camera_screen.status().is_installed() {
            return ScenarioStep::Finished(failed(format!(
                "место замера {}: экран камеры не установлен (статус {:?})",
                surface.name(),
                context.camera_screen.status()
            )));
        }
        let session = match ViewfinderSession::start(context, running, &step, SessionTiming::of(&config)) {
            Ok(session) => session,
            Err(failure) => return ScenarioStep::Finished(failed(failure.describe())),
        };
        let rows: Vec<serde_json::Value> = config.stage_budget.rows.iter().map(|row| row.name().into()).collect();
        context.emit_stage(
            "started",
            stage_fields([
                ("rows", rows.into()),
                ("surface", surface.name().into()),
                (
                    "scope_page",
                    serde_json::to_value(config.stage_budget.scope_page).unwrap_or(serde_json::Value::Null),
                ),
                (
                    "window_ms",
                    u64::try_from(power.ab.window.as_millis()).unwrap_or(u64::MAX).into(),
                ),
                ("pairs", power.ab.pairs.into()),
            ]),
        );
        let touch = &camera.scenarios().screen.touch;
        let page_swipes = config.stage_budget.scope_page_swipes;
        self.run = Some(BudgetRun {
            config,
            power,
            session,
            sampling: None,
            phase: Phase::Starting,
            swipes: SurfaceSwipes {
                enter_calibrate: touch.modes_swipe_left,
                leave_calibrate: touch.modes_swipe_right,
                scope_page_next: page_swipes.next,
                scope_page_previous: page_swipes.previous,
            },
            warmup_ui_frames: camera.scenarios().screen.warmup_ui_frames,
            open: None,
            next_row: 0,
            rows: Vec::new(),
            power_refusal: None,
            samples: Vec::new(),
            scopes: None,
            look: None,
            entry_failure: None,
            exit_failure: None,
            levers: RunLevers {
                history: LeverHistory::default(),
                hold_until: None,
                undelivered: Vec::new(),
            },
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
        let RunStep::Finish(failure) = run.tick(context, running) else {
            return ScenarioStep::Continue;
        };
        let Some(finished) = self.run.take() else {
            return ScenarioStep::Continue;
        };
        let gpu_window_frames = context.gpu.render_config.viewfinder.gpu_time_window_frames;
        let observation = finished.into_observation(running, gpu_window_frames, failure);
        emit_details(context, "detail_stage_budget", observation.detail_items());
        let violations = evaluate_stage_budget(&observation);
        ScenarioStep::Finished(finish_within_budget(context, observation.summary(), violations))
    }
}

#[cfg(test)]
#[path = "viewfinder_stage_budget_tests.rs"]
mod tests;
