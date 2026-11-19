//! Smoke-сценарии камеры (сессия 04): каталог, подтверждение частоты через NDK, вытеснение.
//!
//! * [`CameraCatalogScenario`] — `camera-catalog`: каталог камер и снимок диагностики бэкенда;
//! * [`CameraStreamScenario`] — `camera-stream`: шаги «открыть → поток → замер частоты по меткам сенсора»;
//! * [`CameraEvictScenario`] — `camera-evict` (только `debug-scenarios`): вытеснение системной камерой;
//! * [`CameraRawStreamScenario`] — `raw-stream`, `raw-stream-endurance` и (только `debug-scenarios`)
//!   `raw-stream-cpu-probe`: поток RAW-кадров камеры до экрана без копий (сессия 05).
//! * сессия 07 (видоискатель с ISP на GPU): [`ViewfinderSmokeScenario`] — `viewfinder`,
//!   [`ViewfinderLatencyScenario`] — `viewfinder-latency`, [`ViewfinderStageBudgetScenario`] —
//!   `viewfinder-stage-budget` (таблица решения 45), [`ViewfinderEnduranceScenario`] —
//!   `viewfinder-endurance` (10 минут), [`VideoHeadroomScenario`] — `video-headroom-bench` (V4).
//!   Общий запуск — `viewfinder_session`, числа — `viewfinder_metrics`.
//! * сессия 08: [`ThreeAScenario`] — `3a` (тап, ручная экспозиция, EV, блокировка, брекетинг; шаги — `three_a_steps*`,
//!   вердикт — `three_a_verdict`).
//! * сессия 08c: [`AeTraceScenario`] — `ae-trace` (ряд нашего AE по ступенькам света; расписание — `ae_trace_plan`,
//!   запись — `ae_trace_run`, разбор — `ae_trace_analysis`/`_steps`/`_stats`, вердикт — `ae_trace_verdict`).
//! * сессия 11: [`GlassScenario`] — `glass` (стекло поверх видоискателя; отчёт и вердикт — `glass_report`).
//! * сессия 12: [`ScreenScenario`] — `screen` (экран камеры iOS, на котором скрипт делает касания; шаги — `screen_steps`,
//!   исполнитель — `screen_runner`, вердикт — `screen_report`). Единственный сценарий с `KeepRunning`.
//!
//! Устройство каждого сценария одинаково: логика движется по тикам маршрутизатора (ничего не
//! блокируется), отчёт — отдельные `Serialize`-структуры (`*_report.rs`), вердикт — чистая функция
//! `evaluate_*` с тестами на каждое правило. Общее — `progress` (дедлайны, журнал событий) и
//! `engine_watch` (что считать отказом движка).

mod ae_trace;
mod ae_trace_analysis;
mod ae_trace_observation;
mod ae_trace_plan;
mod ae_trace_prompts;
mod ae_trace_report;
mod ae_trace_run;
mod ae_trace_series;
mod ae_trace_stats;
mod ae_trace_steps;
#[cfg(test)]
mod ae_trace_test_support;
mod ae_trace_verdict;
mod backend_summary;
mod catalog;
mod catalog_report;
mod detail;
mod engine_watch;
#[cfg(feature = "debug-scenarios")]
mod evict;
#[cfg(any(test, feature = "debug-scenarios"))]
mod evict_report;
// Демо-сцена стекла (сессия 11) и сценарий `glass` на ней. `pub(crate)`: её конфиг — часть
// `camera.scenarios.glass` (`glass_scenario_config`).
mod glass;
mod glass_report;
pub(crate) mod glass_showcase;
mod operator_prompt_log;
mod progress;
mod raw_stream;
mod raw_stream_deltas;
mod raw_stream_report;
mod raw_stream_samples;
mod result_budget;
mod screen;
mod screen_expect;
mod screen_gesture;
mod screen_probe;
mod screen_report;
mod screen_runner;
mod screen_sensor_point;
mod screen_steps;
mod stream;
mod stream_report;
mod three_a;
mod three_a_lock_prompts;
mod three_a_own_ae;
mod three_a_report;
mod three_a_results;
mod three_a_steps;
mod three_a_steps_bracket;
mod three_a_steps_exposure;
mod three_a_steps_focus;
mod three_a_verdict;
mod video_headroom;
mod video_headroom_report;
mod viewfinder_endurance;
mod viewfinder_endurance_report;
mod viewfinder_latency;
mod viewfinder_latency_report;
mod viewfinder_latency_verdict;
mod viewfinder_metrics;
mod viewfinder_minutes;
mod viewfinder_pacing_report;
mod viewfinder_session;
mod viewfinder_smoke;
mod viewfinder_smoke_report;
mod viewfinder_stage_budget;
mod viewfinder_stage_budget_report;
mod viewfinder_tuning_report;
mod white_balance_report;

use super::observing::ObservingScenario;
use crate::raw_stream_scenario_config::RawStreamRunKind;

pub use ae_trace::AeTraceScenario;
pub use catalog::CameraCatalogScenario;
#[cfg(feature = "debug-scenarios")]
pub use evict::CameraEvictScenario;
pub use glass::GlassScenario;
pub use raw_stream::CameraRawStreamScenario;
pub use screen::ScreenScenario;
pub use stream::CameraStreamScenario;
pub use three_a::ThreeAScenario;
pub use video_headroom::VideoHeadroomScenario;
pub use viewfinder_endurance::ViewfinderEnduranceScenario;
pub use viewfinder_latency::ViewfinderLatencyScenario;
pub use viewfinder_smoke::ViewfinderSmokeScenario;
pub use viewfinder_stage_budget::ViewfinderStageBudgetScenario;

/// Какой из сценариев камеры. Реестр сценариев (`Scenario`) знает только этот тип, а не каждый
/// сценарий камеры по отдельности: `scenario/mod.rs` не раздувается с каждым новым сценарием.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraScenario {
    Catalog,
    Stream,
    /// `raw-stream`: минутный замер цепочки «камера -> лизинг -> рендер -> экран».
    RawStream,
    /// `raw-stream-endurance`: то же на пять минут (критерий выхода сессии 05).
    RawStreamEndurance,
    /// Только отладочная сборка: доказывает, что счётчик CPU-копий живой (копирует кадры нарочно).
    #[cfg(feature = "debug-scenarios")]
    RawStreamCpuProbe,
    /// `viewfinder`: минута продуктового видоискателя с ISP (сессия 07).
    Viewfinder,
    /// `viewfinder-latency`: задержка при политиках линии кадра `Preferred` / `NearestFeasible`.
    ViewfinderLatency,
    /// `viewfinder-stage-budget`: таблица «стадия → мс GPU → мА» (решение 45).
    ViewfinderStageBudget,
    /// `viewfinder-endurance`: 10 минут продуктового видоискателя, поиск троттлинга.
    ViewfinderEndurance,
    /// `video-headroom-bench`: стенд запаса под видео (V4), 16:9@60 и экран 120 Гц.
    VideoHeadroomBench,
    /// `glass`: стекло поверх живого видоискателя, GPU-время UI и пропуски кадров (сессия 11).
    Glass,
    /// `screen`: экран камеры iOS, касания по плану скрипта, доказательство «тап дошёл до HAL» (сессия 12).
    Screen,
    /// `3a`: автоматика 3A и ручные параметры на живом видоискателе (сессия 08).
    ThreeA,
    /// `ae-trace`: ряд нашего AE по ступенькам света, подтяжка ISP и мерцание (сессия 08c).
    AeTrace,
    /// Только отладочная сборка: запускает системную камеру и отнимает у нас камеру.
    #[cfg(feature = "debug-scenarios")]
    Evict,
}

impl CameraScenario {
    /// Все сценарии камеры этой сборки.
    pub fn available() -> &'static [CameraScenario] {
        &[
            CameraScenario::Catalog,
            CameraScenario::Stream,
            CameraScenario::RawStream,
            CameraScenario::RawStreamEndurance,
            #[cfg(feature = "debug-scenarios")]
            CameraScenario::RawStreamCpuProbe,
            #[cfg(feature = "debug-scenarios")]
            CameraScenario::Evict,
            CameraScenario::Viewfinder,
            CameraScenario::ViewfinderLatency,
            CameraScenario::ViewfinderStageBudget,
            CameraScenario::ViewfinderEndurance,
            CameraScenario::VideoHeadroomBench,
            CameraScenario::ThreeA,
            CameraScenario::AeTrace,
            CameraScenario::Glass,
            CameraScenario::Screen,
        ]
    }

    /// Те же сценарии в виде записей общего реестра `Scenario`.
    pub fn scenarios() -> impl Iterator<Item = super::Scenario> {
        Self::available().iter().copied().map(super::Scenario::Camera)
    }

    /// Имя для intent-параметра `scenario` и строки результата.
    pub fn name(self) -> &'static str {
        match self {
            CameraScenario::Catalog => "camera-catalog",
            CameraScenario::Stream => "camera-stream",
            CameraScenario::RawStream => "raw-stream",
            CameraScenario::RawStreamEndurance => "raw-stream-endurance",
            #[cfg(feature = "debug-scenarios")]
            CameraScenario::RawStreamCpuProbe => "raw-stream-cpu-probe",
            #[cfg(feature = "debug-scenarios")]
            CameraScenario::Evict => "camera-evict",
            CameraScenario::Viewfinder => "viewfinder",
            CameraScenario::ViewfinderLatency => "viewfinder-latency",
            CameraScenario::ViewfinderStageBudget => "viewfinder-stage-budget",
            CameraScenario::ViewfinderEndurance => "viewfinder-endurance",
            CameraScenario::VideoHeadroomBench => "video-headroom-bench",
            CameraScenario::ThreeA => "3a",
            CameraScenario::AeTrace => "ae-trace",
            CameraScenario::Glass => "glass",
            CameraScenario::Screen => "screen",
        }
    }

    /// Новый экземпляр наблюдающего сценария для маршрутизатора.
    pub fn observing(self) -> Box<dyn ObservingScenario> {
        match self {
            CameraScenario::Catalog => Box::new(CameraCatalogScenario::new()),
            CameraScenario::Stream => Box::new(CameraStreamScenario::new()),
            CameraScenario::RawStream => Box::new(CameraRawStreamScenario::new(RawStreamRunKind::Standard)),
            CameraScenario::RawStreamEndurance => Box::new(CameraRawStreamScenario::new(RawStreamRunKind::Endurance)),
            #[cfg(feature = "debug-scenarios")]
            CameraScenario::RawStreamCpuProbe => Box::new(CameraRawStreamScenario::new(RawStreamRunKind::CpuProbe)),
            #[cfg(feature = "debug-scenarios")]
            CameraScenario::Evict => Box::new(CameraEvictScenario::new()),
            CameraScenario::Viewfinder => Box::new(ViewfinderSmokeScenario::new()),
            CameraScenario::ViewfinderLatency => Box::new(ViewfinderLatencyScenario::new()),
            CameraScenario::ViewfinderStageBudget => Box::new(ViewfinderStageBudgetScenario::new()),
            CameraScenario::ViewfinderEndurance => Box::new(ViewfinderEnduranceScenario::new()),
            CameraScenario::VideoHeadroomBench => Box::new(VideoHeadroomScenario::new()),
            CameraScenario::ThreeA => Box::new(ThreeAScenario::new()),
            CameraScenario::AeTrace => Box::new(AeTraceScenario::new()),
            CameraScenario::Glass => Box::new(GlassScenario::new()),
            CameraScenario::Screen => Box::new(ScreenScenario::new()),
        }
    }
}
