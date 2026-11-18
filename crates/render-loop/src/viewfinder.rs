//! Видоискатель камеры глазами потока рендера (сессия 07): нейтральные типы управления,
//! диагностики и публикации статистики AWB.
//!
//! Здесь нет Vulkan: это словарь между потоком рендера, маршрутизатором (команды) и будущим
//! владельцем AWB (сессия 08, читает статистику). Рисует видоискатель `vulkan_backend::camera`.
//!
//! * [`control`] — чем рисовать (ISP или отладочная мозаика 05) и типизированные команды на ходу;
//! * [`diagnostics`] — что видоискатель делал: стадии, время GPU, счётчики параметров ISP;
//! * [`headroom`] — стенд «запас под видео» (V4): включение командой и его диагностика;
//! * [`white_balance`] — диагностика нашего AWB: белая, оценка, сравнение с HAL, холодный старт;
//! * [`awb_feed`] — «последняя статистика AWB» с номером кадра и меткой сенсора (latest-wins).

pub mod awb_feed;
pub mod control;
pub mod diagnostics;
pub mod headroom;
pub mod white_balance;

pub use awb_feed::{AwbFrameIdentity, AwbPublication, AwbStatisticsFeed, AwbStatisticsReader, PublishedAwbStatistics};
pub use control::{
    ViewfinderCommand, ViewfinderCommandLog, ViewfinderCommandOutcome, ViewfinderCommandRecord, ViewfinderPainter,
};
pub use diagnostics::{
    ViewfinderColorSpace, ViewfinderCounters, ViewfinderDiagnostics, ViewfinderExtent, ViewfinderGpuTimes,
    ViewfinderStorage,
};
pub use headroom::{HeadroomBench, HeadroomBenchDiagnostics, HeadroomPassTime};
pub use white_balance::{ColdStartState, EstimateReading, WhiteBalanceDiagnostics, WhiteDelta, WhiteReading};
