//! Качественный полнокадровый конвейер для стенда «запас под видео» (V4, сессия 07, пакет P6).
//!
//! Это НЕ продуктовый видоискатель ([`crate::viewfinder`]) и от него не зависит: отдельный многопроходный
//! конвейер полного разрешения, чтобы измерить цену видео-ISP на кадре 4096×2304@60 (цифра для плана
//! `video`). Выход — через нейтральный приёмник кадра `gpu_vulkan::frame_sink` (V5): позже туда
//! подключится входная surface кодера без переделки конвейера.
//!
//! # Проходы (каждый — отдельный dispatch с меткой времени GPU)
//! * уровни и баланс: кадр камеры (RAW16/RAW10) → мозаика f32 (`isp-model` levels + white_balance);
//! * [`QualityDemosaic`] — Menon 2007 с шагом уточнения (`isp-model` `DemosaicMethod::Quality`), 8 dispatch:
//!   шаг решения разбит на «разности» и «решение», чтобы не пересчитывать каждую разность 8 раз;
//! * [`TemporalDenoise`] — временной шумодав «на пробу» (без компенсации движения), можно выключить;
//! * выход: две матрицы цвета → сжатие охвата → тон → sRGB OETF → цель приёмника.
//!
//! Все промежуточные данные — f32 в device-local буферах ([`QualityPlanes`]): паритет с эталоном CPU
//! (f32) важнее экономии полосы на стенде; fp16-хранение — кандидат оптимизации в плане `video`.
//!
//! Сборка всего вместе — [`VideoHeadroomWorkload`].

pub(crate) mod config;
pub(crate) mod demosaic;
mod error;
pub(crate) mod frame_params;
pub(crate) mod input;
pub(crate) mod pass;
pub(crate) mod planes;
mod stages;
mod temporal;
#[cfg(test)]
mod test_params;
mod timing;
mod workload;

pub use self::config::{QualityOutputFormat, TemporalDenoiseConfig, VideoHeadroomConfig, WorkgroupSizeConfig};
pub use self::demosaic::{MenonPass, QualityDemosaic};
pub use self::error::QualityError;
pub use self::frame_params::CfaCode;
pub use self::input::{StreamGeneration, VideoFrameInput};
pub use self::planes::{LinearRgbPlanes, QualityPlanes};
pub use self::temporal::{HistoryState, HistoryUse, TemporalDenoise};
pub use self::timing::{HeadroomPass, HeadroomTimingCollect, HeadroomTimings, PassTiming};
pub use self::workload::{HeadroomRecordOutcome, VideoHeadroomWorkload};
