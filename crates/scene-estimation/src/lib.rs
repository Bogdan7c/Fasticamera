//! Оценка света сцены по статистике RAW (AWB, сессия 08; решения 62–68).
//!
//! Crate нейтральный (без Vulkan и Android): его гоняют тесты на ПК, а исполняет поток рендера.
//! Между модулями ходит **точка белого сцены** ([`Chromaticity`](isp_model::color::chromaticity::Chromaticity)
//! или «температура + Duv»); в нейтраль конкретной камеры её переводит `DngColorProfile` потока камеры.
//!
//! # Состав
//! * [`config`] — все числа (serde, `deny_unknown_fields`, `validate()`), у каждого значения по умолчанию — источник;
//! * [`locus_table`] — таблица кривой Планка в пространстве конкретной камеры (считается на CPU **один раз
//!   на поток** в f64, дальше — плоские f32-массивы `#[repr(C)]`, готовые для uniform/storage-буфера шейдера);
//! * [`estimator`] — оценщик «в форме шейдера» (f32, фиксированные циклы, без итераций): плитки статистики →
//!   [`RawEstimate`](estimate::RawEstimate); отображение на GLSL описано в доке модуля;
//! * [`executor`] — граница исполнителя (решение 68): трейс [`WhiteBalanceEstimator`](executor::WhiteBalanceEstimator)
//!   в форме «отправить / забрать позже», эталон на CPU — [`CpuWhiteBalanceEstimator`](executor::CpuWhiteBalanceEstimator);
//!   реализация на compute-шейдере встанет за тот же трейс без правки вызывающих;
//! * [`smoother`], [`controller`] — скалярное решение на CPU: сглаживание по времени кадров сенсора,
//!   режимы «авто / блокировка / вручную», отчёт о смене белой только при заметном сдвиге.
//!
//! * [`exposure`] — наш AE (08a, решения 63, 66, 68): та же статистика и та же форма исполнителя. Замер по плиткам
//!   (матричный и точечный), контроллер цели, предложение движку камеры и остаточная подтяжка ISP;
//!   порядок вызовов на кадр — в доке модуля.

pub mod config;
pub mod controller;
pub mod error;
pub mod estimate;
pub mod estimator;
pub mod executor;
pub mod exposure;
pub mod locus_table;
pub mod smoother;
