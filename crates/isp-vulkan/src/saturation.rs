//! Адаптеры работ `isp-vulkan` под стенд насыщения `gpu_vulkan::saturation` (сессия 16a, пакет P4-isp).
//!
//! # Зачем
//! На Adreno миллисекунды GPU в темпе vsync — не цена работы (драйвер сам тактует GPU по нагрузке). Цена
//! кадра работы = 1 / пропускная способность при кадрах подряд. Стенд меряет её, не зная, что внутри;
//! здесь четыре работы — те же вызовы, что у продукта:
//! * [`ViewfinderSaturation`] — слитый проход видоискателя (`ViewfinderIsp::record`) на RAW10 продуктового размера;
//! * [`ScopesSaturation`] — цветовые приборы (`ColorScopes::record`) по готовой картинке видоискателя;
//! * [`HeadroomSaturation`] — цепочка стенда V4 (`VideoHeadroomWorkload`) на кадре 4096×2304;
//! * [`StillSaturation`] — проявление одного снимка целиком (`PhotoDeveloper`).
//!
//! # Общие правила адаптеров
//! * ресурсы на слот создаются в конструкторе, в `record_frame` ничего не выделяется (исключения — в доке работы);
//! * первая типизированная ошибка остаётся у адаптера (`take_failure`), стенду уходит `GpuError::InvalidState`;
//! * времена проходов — по timestamp-запросам самой работы в [`gpu_vulkan::saturation::PassTimeLog`]
//!   (прогрев и хвост стенда в журнал тоже попадают: работа не отличает их от окна замера);
//! * «нет поддержки timestamp» — не ошибка, а кадр без замера;
//! * память RAW-кадра принадлежит вызывающему ([`RawFrameHolder`]); синтетический RAW10 — `HostRawFrame` (без `test-support`, работает в release).
//!   Под `test-support` остались только блокирующие readback-помощники тестов.

mod blocking;
mod error;
mod headroom;
mod params;
mod scene;
mod scene_probe;
mod scopes;
mod scopes_diagnostic;
mod source;
mod still;
mod timing;
mod viewfinder;

pub use self::error::SaturationWorkError;
pub use self::headroom::{HEADROOM_FRAME_SIZE, HeadroomCounters, HeadroomSaturation, HeadroomSaturationConfig};
pub use self::params::ProductParamsRecipe;
pub use self::scene::{
    DEFAULT_GRAY_CODE, Raw10Frame, SENSOR_BLACK_CODE, SENSOR_WHITE_CODE, SceneContent, synthesize_raw10,
};
pub use self::scopes::{DEFAULT_PICTURE_SIZE_PX, ScopesCounters, ScopesSaturation, ScopesSaturationConfig};
pub use self::scopes_diagnostic::AccumulateDiagnostic;
pub use self::source::{RawFrameHolder, synthetic_raw10_holder};
pub use self::still::{STILL_FRAME_SIZE, StillCounters, StillSaturation, StillSaturationConfig};
pub use self::viewfinder::{
    PASS_LOG_CAPACITY_FRAMES, PRODUCT_AWB_STATISTICS_EVERY_N_FRAMES, PRODUCT_FRAME_SIZE, ViewfinderCounters,
    ViewfinderSaturation, ViewfinderSaturationConfig,
};
