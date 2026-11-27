//! Окно A/B-замера `viewfinder-stage-budget`: что в нём наблюдали и применилось ли заданное состояние (16a).
//!
//! Ток окна меряет [`AbPowerExperiment`](crate::power_measurement::AbPowerExperiment), а ЭТОТ модуль отвечает за
//! вопрос «а было ли в окне именно то состояние, которое мы считали»: рычаг мог не дойти до рендера, экран мог
//! не нарисовать UI, драйвер скрипта мог не удержать касание. Замер в таком окне — «измерено», но не то, что
//! заявлено, поэтому окно, чьё состояние не подтвердилось, ОТВЕРГАЕТСЯ с причиной ([`WindowRefusal`]):
//! * [`WindowRefusal::StateNotApplied`] — диагностика рендера не показала заданное (рисовальщик, стадии, маска
//!   подавления, рост или неподвижность счётчиков приборов и UI);
//! * [`WindowRefusal::DisplayRefreshNotReached`] — частота показа окна не совпала с целевой (строка `display_120hz`).
//!
//! # Как судится рост счётчиков
//! Счётчики накопительные, поэтому судится ПРИРАЩЕНИЕ за измеряемую часть окна: первый снимок после разогрева —
//! базовая линия, последний — итог. Подавленная работа обязана не расти (приборы: `passes_recorded`; UI:
//! `frames_drawn`) и учитываться как подавленная (`frames_suppressed` растёт), неподавленная — расти.
//!
//! # Время GPU подавленной работы
//! Подавленная работа не выполнялась, а скользящее окно её времени в рендере при этом залипает на старых числах.
//! Поэтому время подавленной работы в окне НЕ читается вовсе (`None`), а колонка цены считает его нулём
//! ([`CostMetric::absent_when_off_means_zero`](super::stage_budget_states::CostMetric)).

use std::sync::Arc;

use look_contract::{LookRecipe, LookSettings, ToneBaseModel};
use render_loop::viewfinder::{SuppressibleGpuWork, ViewfinderDiagnostics};
use serde::Serialize;

use super::look_report::LookReport;
use super::stage_budget_states::WorkState;
use super::viewfinder_metrics::{GpuTimeMs, round2};
use super::viewfinder_stage_budget_report::GpuWindowSample;
use crate::camera_presentation::CameraPresentation;
use crate::power_measurement::{AbRejection, StageSetting, ThermalSnapshot};
use crate::stage_budget_surface_config::StageBudgetSurface;

/// Что должно произойти со счётчиком за измеряемую часть окна.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Growth {
    /// Обязан вырасти (работа идёт).
    MustGrow,
    /// Обязан стоять (работа подавлена).
    MustStay,
    /// Состояние ничего не говорит об этом счётчике.
    Unconstrained,
}

impl Growth {
    fn is_met_by(self, delta: u64) -> bool {
        match self {
            Self::MustGrow => delta > 0,
            Self::MustStay => delta == 0,
            Self::Unconstrained => true,
        }
    }
}

/// Ожидания окна по счётчикам: выводятся из состояния и места замера.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StateExpectation {
    pub scopes_passes: Growth,
    pub ui_frames_drawn: Growth,
    pub ui_frames_suppressed: Growth,
    /// Построения пирамиды размытия фона. В разделе калибровки размытия нет (решение 261): число обязано стоять.
    pub backdrop_pyramid_rebuilds: Growth,
}

impl StateExpectation {
    /// Что ожидать в окне с состоянием `state` на месте `surface`.
    pub fn of(state: &WorkState, surface: StageBudgetSurface) -> Self {
        let scopes_suppressed = state.suppression.suppresses(SuppressibleGpuWork::ColorScopes);
        let ui_suppressed = state.suppression.suppresses(SuppressibleGpuWork::UiOverlayDrawing);
        let mosaic = state.painter == render_loop::viewfinder::ViewfinderPainter::DebugMosaic;
        let scopes_passes = if scopes_suppressed || mosaic {
            // Под мозаикой приборы не считаются вовсе: они читают выход ISP, а мозаика приборов не знает.
            Growth::MustStay
        } else if surface.runs_color_scopes() {
            Growth::MustGrow
        } else {
            Growth::Unconstrained
        };
        let (ui_frames_drawn, ui_frames_suppressed) = match (surface.draws_ui_overlay(), mosaic, ui_suppressed) {
            (false, _, _) => (Growth::Unconstrained, Growth::Unconstrained),
            // Путь кадра мозаики (`Content::Mosaic`, render-loop `session/record.rs`) не доходит до развилки UI:
            // источник не получает `produce_frame`, кадр UI не рисуется и НЕ считается подавленным. Это поведение
            // рендера, а не сбой подавления; ожидание выводится из «рисовальщик × подавление».
            (true, true, _) => (Growth::MustStay, Growth::MustStay),
            (true, false, true) => (Growth::MustStay, Growth::MustGrow),
            (true, false, false) => (Growth::MustGrow, Growth::MustStay),
        };
        let backdrop_pyramid_rebuilds = if surface.runs_color_scopes() {
            Growth::MustStay
        } else {
            Growth::Unconstrained
        };
        Self {
            scopes_passes,
            ui_frames_drawn,
            ui_frames_suppressed,
            backdrop_pyramid_rebuilds,
        }
    }
}

/// Накопительные счётчики, по которым судится окно.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CounterSlice {
    scopes_passes: u64,
    ui_frames_drawn: u64,
    ui_frames_suppressed: u64,
    backdrop_pyramid_rebuilds: u64,
}

impl CounterSlice {
    fn of(viewfinder: &ViewfinderDiagnostics) -> Self {
        Self {
            scopes_passes: viewfinder.scopes.counters.passes_recorded,
            ui_frames_drawn: viewfinder.ui_overlay.frames_drawn,
            ui_frames_suppressed: viewfinder.ui_overlay.frames_suppressed,
            backdrop_pyramid_rebuilds: viewfinder.ui_overlay.pyramid_rebuilds,
        }
    }

    /// Приращение от `earlier` до `self` (счётчики не убывают; saturating — на случай пересоздания рендера).
    fn since(self, earlier: Self) -> CounterDelta {
        CounterDelta {
            scopes_passes: self.scopes_passes.saturating_sub(earlier.scopes_passes),
            ui_frames_drawn: self.ui_frames_drawn.saturating_sub(earlier.ui_frames_drawn),
            ui_frames_suppressed: self.ui_frames_suppressed.saturating_sub(earlier.ui_frames_suppressed),
            backdrop_pyramid_rebuilds: self
                .backdrop_pyramid_rebuilds
                .saturating_sub(earlier.backdrop_pyramid_rebuilds),
        }
    }
}

/// Приращения счётчиков за измеряемую часть окна (в подробности).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct CounterDelta {
    pub scopes_passes: u64,
    pub ui_frames_drawn: u64,
    pub ui_frames_suppressed: u64,
    /// Сколько раз за окно строилась пирамида размытия фона (в разделе калибровки обязано быть 0).
    pub backdrop_pyramid_rebuilds: u64,
}

/// Что именно не подтвердилось (имена — в отчёт).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StateMismatch {
    /// Рисовальщик или набор стадий ISP в статусе рендера не те.
    PainterOrStages,
    /// Маска подавления в статусе рендера не та.
    GpuWorkSuppression,
    /// В окне нет диагностики видоискателя вовсе.
    NoViewfinderDiagnostics,
    /// Приборы подавлены, но проходы записывались.
    ScopesStillRunning,
    /// Приборы должны считать, но проходы не записывались.
    ScopesIdle,
    /// UI подавлен, но кадры UI рисовались в GPU.
    UiStillDrawing,
    /// UI должен рисоваться, но кадры в GPU не писались.
    UiNotDrawing,
    /// UI подавлен, но подавленные кадры не считались (подавление не дошло до прохода UI).
    UiSuppressionNotCounted,
    /// UI не подавлен, но рендер считал кадры подавленными.
    UiCountedSuppressed,
    /// Вид или основа в диагностике рендера не те, что заданы окну (строки вида, 14o). Типичная причина на телефоне
    /// — вид не дошёл до рендера или его перебил исполнитель вида экрана.
    LookNotApplied,
    /// В разделе калибровки строилась пирамида размытия фона: размытия там быть не должно (решение 261).
    BackdropBlurBuilt,
}

/// Находит расхождения «ожидали / увидели» (чистая функция: на ней стоят тесты без рендера).
fn find_mismatches(
    state: &WorkState,
    expectation: &StateExpectation,
    expected_look: Option<&LookRecipe>,
    latest: Option<&Reading>,
    delta: Option<CounterDelta>,
) -> Vec<StateMismatch> {
    let (Some(latest), Some(delta)) = (latest, delta) else {
        return vec![StateMismatch::NoViewfinderDiagnostics];
    };
    let mut mismatches = Vec::new();
    if latest.painter != state.painter || latest.stages != state.stages {
        mismatches.push(StateMismatch::PainterOrStages);
    }
    if latest.suppression != state.suppression {
        mismatches.push(StateMismatch::GpuWorkSuppression);
    }
    if let Some(expected) = expected_look
        && (latest.tone_base != expected.base || *latest.look != expected.settings)
    {
        mismatches.push(StateMismatch::LookNotApplied);
    }
    if !expectation
        .backdrop_pyramid_rebuilds
        .is_met_by(delta.backdrop_pyramid_rebuilds)
    {
        mismatches.push(StateMismatch::BackdropBlurBuilt);
    }
    if !expectation.scopes_passes.is_met_by(delta.scopes_passes) {
        mismatches.push(match expectation.scopes_passes {
            Growth::MustStay => StateMismatch::ScopesStillRunning,
            _ => StateMismatch::ScopesIdle,
        });
    }
    if !expectation.ui_frames_drawn.is_met_by(delta.ui_frames_drawn) {
        mismatches.push(match expectation.ui_frames_drawn {
            Growth::MustStay => StateMismatch::UiStillDrawing,
            _ => StateMismatch::UiNotDrawing,
        });
    }
    if !expectation.ui_frames_suppressed.is_met_by(delta.ui_frames_suppressed) {
        mismatches.push(match expectation.ui_frames_suppressed {
            Growth::MustGrow => StateMismatch::UiSuppressionNotCounted,
            _ => StateMismatch::UiCountedSuppressed,
        });
    }
    mismatches
}

/// Одно показание рендера за тик (то, что нужно окну).
#[derive(Debug, Clone)]
struct Reading {
    painter: render_loop::viewfinder::ViewfinderPainter,
    stages: isp_vulkan::viewfinder::IspStageSet,
    suppression: render_loop::viewfinder::GpuWorkSuppression,
    /// Значения ручек и основа, как их применил рендер (14o).
    look: Arc<LookSettings>,
    tone_base: ToneBaseModel,
    isp_dispatch: Option<GpuTimeMs>,
    frame_total: Option<GpuTimeMs>,
    ui_overlay_gpu: Option<GpuTimeMs>,
    scopes_gpu: Option<GpuTimeMs>,
}

impl Reading {
    fn of(viewfinder: &ViewfinderDiagnostics) -> Self {
        Self {
            painter: viewfinder.painter,
            stages: viewfinder.stages,
            suppression: viewfinder.gpu_work_suppression,
            look: Arc::clone(&viewfinder.look),
            tone_base: viewfinder.tone_base,
            isp_dispatch: GpuTimeMs::of_optional(viewfinder.gpu_time.isp_dispatch.as_ref()),
            frame_total: GpuTimeMs::of_optional(viewfinder.gpu_time.frame_total.as_ref()),
            ui_overlay_gpu: GpuTimeMs::of_optional(viewfinder.ui_overlay.gpu_time.as_ref()),
            scopes_gpu: GpuTimeMs::of_optional(viewfinder.scopes.gpu_time.as_ref()),
        }
    }
}

/// Показание рендера за один тик.
struct Tick {
    /// Показание видоискателя и его счётчики; `None` — диагностики видоискателя нет.
    viewfinder: Option<(Reading, CounterSlice)>,
    /// Частота показа по периоду экрана от драйвера, Гц; `None` — драйвер период не сообщил.
    refresh_hz: Option<f64>,
}

impl Tick {
    fn of(presentation: &CameraPresentation) -> Self {
        Self {
            viewfinder: presentation
                .viewfinder
                .as_ref()
                .map(|viewfinder| (Reading::of(viewfinder), CounterSlice::of(viewfinder))),
            refresh_hz: presentation
                .display_refresh_cycle
                .map(|cycle| cycle.as_secs_f64())
                .filter(|seconds| *seconds > 0.0)
                .map(|seconds| 1.0 / seconds),
        }
    }
}

/// Частота показа окна: ограниченная сводка по показаниям драйвера (без хранения каждого).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct RefreshStats {
    readings: u32,
    sum_hz: f64,
    min_hz: f64,
    max_hz: f64,
}

impl RefreshStats {
    /// Учесть показание частоты, Гц (нечисловые и неположительные отбрасываются: это не частота).
    pub fn observe(&mut self, hz: f64) {
        if !(hz.is_finite() && hz > 0.0) {
            return;
        }
        if self.readings == 0 {
            self.min_hz = hz;
            self.max_hz = hz;
        } else {
            self.min_hz = self.min_hz.min(hz);
            self.max_hz = self.max_hz.max(hz);
        }
        self.readings += 1;
        self.sum_hz += hz;
    }

    /// Средняя частота окна, Гц; `None` — показаний не было (драйвер не сообщил период экрана).
    pub fn mean_hz(&self) -> Option<f64> {
        (self.readings > 0).then(|| self.sum_hz / f64::from(self.readings))
    }

    fn reading(&self) -> Option<RefreshReading> {
        Some(RefreshReading {
            mean_hz: round2(self.mean_hz()?),
            min_hz: round2(self.min_hz),
            max_hz: round2(self.max_hz),
            readings: self.readings,
        })
    }
}

/// Частота показа окна для отчёта.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RefreshReading {
    pub mean_hz: f64,
    pub min_hz: f64,
    pub max_hz: f64,
    pub readings: u32,
}

/// Почему окно отвергнуто (кроме отказов тока, их судит эксперимент).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WindowRefusal {
    /// Заданное состояние в окне не подтвердилось диагностикой.
    StateNotApplied { mismatches: Vec<StateMismatch> },
    /// Частота показа окна не совпала с целевой.
    DisplayRefreshNotReached {
        expected_hz: f64,
        /// Средняя частота окна; `None` — драйвер не сообщил период экрана.
        observed_mean_hz: Option<f64>,
        tolerance_hz: f64,
    },
}

impl WindowRefusal {
    /// Причина для эксперимента: он пометит вывод `Rejected`, а имя вида попадёт в строку таблицы.
    pub fn to_rejection(&self, index: usize, stage: StageSetting) -> AbRejection {
        match self {
            Self::StateNotApplied { .. } => AbRejection::StateNotApplied { index, stage },
            Self::DisplayRefreshNotReached { .. } => AbRejection::DisplayRefreshNotReached { index, stage },
        }
    }

    /// Одной строкой для списка нарушений.
    pub fn describe(&self) -> String {
        match self {
            Self::StateNotApplied { mismatches } => {
                let names: Vec<String> = mismatches
                    .iter()
                    .map(|mismatch| {
                        serde_json::to_value(mismatch)
                            .ok()
                            .and_then(|value| value.as_str().map(str::to_owned))
                            .unwrap_or_else(|| format!("{mismatch:?}"))
                    })
                    .collect();
                format!("state_not_applied ({})", names.join(", "))
            }
            Self::DisplayRefreshNotReached {
                expected_hz,
                observed_mean_hz,
                tolerance_hz,
            } => match observed_mean_hz {
                Some(observed) => format!(
                    "display_refresh_not_reached (ждали {expected_hz:.0} ± {tolerance_hz:.0} Гц, было {observed:.1})"
                ),
                None => format!("display_refresh_not_reached (ждали {expected_hz:.0} Гц, драйвер частоту не сообщил)"),
            },
        }
    }
}

/// Что сценарий выдал окну: запрошенная частота и удерживаемое касание. Нужно, чтобы на телефоне отличить «система не
/// подняла частоту» от «мы не попросили».
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct WindowIssued {
    /// Частота, запрошенная у платформы для этого положения, Гц; `None` — частоту строка не трогает.
    pub requested_hz: Option<f32>,
    /// Стадия удержания пальца в начале окна: `None` — не выдавалась.
    pub hold: Option<HoldIssued>,
}

/// Выданное удержание пальца.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct HoldIssued {
    pub hold_ms: u64,
    /// `true` — драйверу велено начать нажатие; `false` — окно покрыто нажатием предыдущего окна A.
    pub new_press: bool,
}

/// Условия окна для подробностей: тепло на границах, частота, приращения счётчиков, отказ.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WindowCondition {
    pub index: usize,
    pub stage: StageSetting,
    pub thermal_start: ThermalSnapshot,
    pub thermal_end: ThermalSnapshot,
    /// Что сценарий выдал окну (запрошенная частота, удержание).
    pub issued: WindowIssued,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<RefreshReading>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub counters: Option<CounterDelta>,
    /// Вид и основа, как их показывал рендер в конце окна (14o); `None` — диагностики видоискателя не было.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub look: Option<LookReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<WindowRefusal>,
}

/// Закрытое окно: время GPU для таблицы и условия для подробностей.
#[derive(Debug, Clone, PartialEq)]
pub struct ClosedWindow {
    pub sample: GpuWindowSample,
    pub condition: WindowCondition,
}

/// Наблюдатель одного окна: собирает показания рендера в измеряемой части и выносит приговор в конце.
pub struct WindowObserver {
    index: usize,
    stage: StageSetting,
    state: WorkState,
    expectation: StateExpectation,
    /// Рецепт, который окно обязано увидеть на рендере (строки вида); `None` — вид окном не ставился.
    expected_look: Option<LookRecipe>,
    refresh_tolerance_hz: f64,
    issued: WindowIssued,
    thermal_start: ThermalSnapshot,
    baseline: Option<CounterSlice>,
    latest_slice: Option<CounterSlice>,
    latest: Option<Reading>,
    observed_any_tick: bool,
    refresh: RefreshStats,
}

impl WindowObserver {
    /// Открывает окно `index` с состоянием `state`; `thermal_start` — тепло на его начало.
    pub fn begin(
        index: usize,
        stage: StageSetting,
        state: WorkState,
        surface: StageBudgetSurface,
        refresh_tolerance_hz: f64,
        issued: WindowIssued,
        thermal_start: ThermalSnapshot,
    ) -> Self {
        Self {
            index,
            stage,
            state,
            expectation: StateExpectation::of(&state, surface),
            expected_look: None,
            refresh_tolerance_hz,
            issued,
            thermal_start,
            baseline: None,
            latest_slice: None,
            latest: None,
            observed_any_tick: false,
            refresh: RefreshStats::default(),
        }
    }

    /// Окно проверяет, что рендер показывает именно этот вид и основу (строки вида, 14o); расхождение отвергает окно
    /// как `state_not_applied`.
    #[must_use]
    pub fn expecting_look(mut self, expected: Option<LookRecipe>) -> Self {
        self.expected_look = expected;
        self
    }

    pub fn index(&self) -> usize {
        self.index
    }

    /// Показание на тике измеряемой части окна (не разогрева).
    pub fn observe(&mut self, presentation: &CameraPresentation) {
        self.record(Tick::of(presentation));
    }

    /// Учитывает показание тика (отдельно от `CameraPresentation`: так окно проверяется синтетическими показаниями).
    fn record(&mut self, tick: Tick) {
        self.observed_any_tick = true;
        if let Some(hz) = tick.refresh_hz {
            self.refresh.observe(hz);
        }
        match tick.viewfinder {
            Some((reading, slice)) => {
                self.baseline.get_or_insert(slice);
                self.latest_slice = Some(slice);
                self.latest = Some(reading);
            }
            None => {
                // Диагностики видоискателя в этом тике нет: окно не может подтвердить состояние.
                self.latest = None;
                self.latest_slice = None;
            }
        }
    }

    /// Окно кончилось: строка для таблицы и условия. `None` — в измеряемой части не было ни одного тика.
    pub fn finish(self, thermal_end: ThermalSnapshot) -> Option<ClosedWindow> {
        if !self.observed_any_tick {
            return None;
        }
        let delta = match (self.latest_slice, self.baseline) {
            (Some(latest), Some(baseline)) => Some(latest.since(baseline)),
            _ => None,
        };
        let mismatches = find_mismatches(
            &self.state,
            &self.expectation,
            self.expected_look.as_ref(),
            self.latest.as_ref(),
            delta,
        );
        let refusal = if mismatches.is_empty() {
            self.display_refusal()
        } else {
            Some(WindowRefusal::StateNotApplied { mismatches })
        };
        let latest = self.latest.as_ref();
        let ui_suppressed = self.state.suppression.suppresses(SuppressibleGpuWork::UiOverlayDrawing);
        let scopes_suppressed = self.state.suppression.suppresses(SuppressibleGpuWork::ColorScopes);
        let sample = GpuWindowSample {
            index: self.index,
            stage: self.stage,
            isp_dispatch: latest.and_then(|reading| reading.isp_dispatch),
            frame_total: latest.and_then(|reading| reading.frame_total),
            setting_in_effect: latest
                .is_some_and(|reading| reading.painter == self.state.painter && reading.stages == self.state.stages),
            // Подавленная работа не выполнялась: её скользящее окно в рендере залипло, читать его нельзя.
            ui_overlay: latest.and_then(|reading| if ui_suppressed { None } else { reading.ui_overlay_gpu }),
            scopes: latest.and_then(|reading| if scopes_suppressed { None } else { reading.scopes_gpu }),
        };
        let condition = WindowCondition {
            index: self.index,
            stage: self.stage,
            thermal_start: self.thermal_start,
            thermal_end,
            issued: self.issued,
            display: self.refresh.reading(),
            counters: delta,
            look: latest.map(|reading| LookReport::of_parts(reading.tone_base, &reading.look, self.state.look)),
            refusal,
        };
        Some(ClosedWindow { sample, condition })
    }

    /// Частота показа не совпала с целевой строки `display_120hz` (для других строк целевой нет).
    fn display_refusal(&self) -> Option<WindowRefusal> {
        let expected_hz = f64::from(self.state.display.target_hz()?);
        let observed = self.refresh.mean_hz();
        let reached = observed.is_some_and(|mean| (mean - expected_hz).abs() <= self.refresh_tolerance_hz);
        (!reached).then_some(WindowRefusal::DisplayRefreshNotReached {
            expected_hz,
            observed_mean_hz: observed.map(round2),
            tolerance_hz: self.refresh_tolerance_hz,
        })
    }
}

#[cfg(test)]
#[path = "stage_budget_window_tests.rs"]
mod tests;
