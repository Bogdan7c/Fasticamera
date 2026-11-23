//! Видоискатель камеры глазами потока рендера (сессия 07): нейтральные типы управления,
//! диагностики и публикации статистики AWB.
//!
//! Здесь нет Vulkan: это словарь между потоком рендера, маршрутизатором (команды) и будущим
//! владельцем AWB (сессия 08, читает статистику). Рисует видоискатель `vulkan_backend::camera`.
//!
//! * [`control`] — чем рисовать (ISP или отладочная мозаика 05) и типизированные команды на ходу;
//! * [`framing`] — кадрирование + место видоискателя на экране одной командой (решение 93);
//! * [`continuity`] — непрерывность картинки при смене объектива: метки кадров, счётчики удержаний (сессия 13);
//! * [`diagnostics`] — что видоискатель делал: стадии, время GPU, счётчики параметров ISP;
//! * [`headroom`] — стенд «запас под видео» (V4): включение командой и его диагностика;
//! * [`white_balance`] — диагностика нашего AWB: белая, оценка, сравнение с HAL, холодный старт;
//! * [`auto_exposure`] — связи нашего AE с движком камеры (ящики) и его диагностика;
//! * [`awb_feed`] — «последняя статистика AWB» с номером кадра и меткой сенсора (latest-wins).

pub mod auto_exposure;
pub mod awb_feed;
pub mod continuity;
pub mod control;
pub mod diagnostics;
pub mod framing;
pub mod headroom;
pub mod scope_diagnostics;
pub mod white_balance;

pub use auto_exposure::{AutoExposureCounters, AutoExposureDiagnostics, AutoExposureLinks};
pub use awb_feed::{AwbFrameIdentity, AwbPublication, AwbStatisticsFeed, AwbStatisticsReader, PublishedAwbStatistics};
pub use continuity::{
    BlackKind, ContinuityCounters, ContinuityDiagnostics, ContinuityStats, FrameCropReport, FrameLabel, FrameRecord,
    HoldEnd, HoldReason, TransitionKind,
};
pub use control::{
    ViewfinderCommand, ViewfinderCommandLog, ViewfinderCommandOutcome, ViewfinderCommandRecord, ViewfinderPainter,
};
pub use diagnostics::{
    ViewfinderColorSpace, ViewfinderCounters, ViewfinderDiagnostics, ViewfinderExtent, ViewfinderGpuTimes,
    ViewfinderStorage,
};
pub use framing::{
    EffectivePlacement, FramingCrop, FramingDestination, NormalizedScreenRect, ScreenRectError, ViewfinderFraming,
};
pub use headroom::{HeadroomBench, HeadroomBenchDiagnostics, HeadroomPassTime};
pub use scope_diagnostics::{ScopeCounters, ScopeDiagnostics, ScopeEvents, ScopeResidency, ScopeResources};
pub use white_balance::{ColdStartState, EstimateReading, WhiteBalanceDiagnostics, WhiteDelta, WhiteReading};
