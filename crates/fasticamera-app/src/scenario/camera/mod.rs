//! Smoke-сценарии камеры (сессия 04): каталог, подтверждение частоты через NDK, вытеснение.
//!
//! * [`CameraCatalogScenario`] — `camera-catalog`: каталог камер и снимок диагностики бэкенда;
//! * [`CameraStreamScenario`] — `camera-stream`: шаги «открыть → поток → замер частоты по меткам сенсора»;
//! * [`CameraEvictScenario`] — `camera-evict` (только `debug-scenarios`): вытеснение системной камерой;
//! * [`CameraRawStreamScenario`] — `raw-stream`, `raw-stream-endurance` и (только `debug-scenarios`)
//!   `raw-stream-cpu-probe`: поток RAW-кадров камеры до экрана без копий (сессия 05).
//!
//! Устройство каждого сценария одинаково: логика движется по тикам маршрутизатора (ничего не
//! блокируется), отчёт — отдельные `Serialize`-структуры (`*_report.rs`), вердикт — чистая функция
//! `evaluate_*` с тестами на каждое правило. Общее — `progress` (дедлайны, журнал событий) и
//! `engine_watch` (что считать отказом движка).

mod backend_summary;
mod catalog;
mod catalog_report;
mod detail;
mod engine_watch;
#[cfg(feature = "debug-scenarios")]
mod evict;
#[cfg(any(test, feature = "debug-scenarios"))]
mod evict_report;
mod progress;
mod raw_stream;
mod raw_stream_deltas;
mod raw_stream_report;
mod raw_stream_samples;
mod stream;
mod stream_report;

use super::observing::ObservingScenario;
use crate::raw_stream_scenario_config::RawStreamRunKind;

pub use catalog::CameraCatalogScenario;
#[cfg(feature = "debug-scenarios")]
pub use evict::CameraEvictScenario;
pub use raw_stream::CameraRawStreamScenario;
pub use stream::CameraStreamScenario;

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
        }
    }
}
