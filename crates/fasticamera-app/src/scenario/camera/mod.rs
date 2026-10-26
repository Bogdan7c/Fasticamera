//! Smoke-сценарии камеры (сессия 04): каталог, подтверждение частоты через NDK, вытеснение.
//!
//! * [`CameraCatalogScenario`] — `camera-catalog`: каталог камер и снимок диагностики бэкенда;
//! * [`CameraStreamScenario`] — `camera-stream`: шаги «открыть → поток → замер частоты по меткам сенсора»;
//! * [`CameraEvictScenario`] — `camera-evict` (только `debug-scenarios`): вытеснение системной камерой.
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
mod stream;
mod stream_report;

use super::observing::ObservingScenario;

pub use catalog::CameraCatalogScenario;
#[cfg(feature = "debug-scenarios")]
pub use evict::CameraEvictScenario;
pub use stream::CameraStreamScenario;

/// Какой из сценариев камеры. Реестр сценариев (`Scenario`) знает только этот тип, а не каждый
/// сценарий камеры по отдельности: `scenario/mod.rs` не раздувается с каждым новым сценарием.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraScenario {
    Catalog,
    Stream,
    /// Только отладочная сборка: запускает системную камеру и отнимает у нас камеру.
    #[cfg(feature = "debug-scenarios")]
    Evict,
}

impl CameraScenario {
    /// Имя для intent-параметра `scenario` и строки результата.
    pub fn name(self) -> &'static str {
        match self {
            CameraScenario::Catalog => "camera-catalog",
            CameraScenario::Stream => "camera-stream",
            #[cfg(feature = "debug-scenarios")]
            CameraScenario::Evict => "camera-evict",
        }
    }

    /// Новый экземпляр наблюдающего сценария для маршрутизатора.
    pub fn observing(self) -> Box<dyn ObservingScenario> {
        match self {
            CameraScenario::Catalog => Box::new(CameraCatalogScenario::new()),
            CameraScenario::Stream => Box::new(CameraStreamScenario::new()),
            #[cfg(feature = "debug-scenarios")]
            CameraScenario::Evict => Box::new(CameraEvictScenario::new()),
        }
    }
}
