//! `camera-contract` — нейтральный словарь камеры Fasticamera.
//!
//! Этими типами обмениваются `camera-core` (движок), `camera-android` (Camera2 NDK), а позже
//! ISP и UI. Здесь только данные и их инварианты, никакого исполнения: кто что открывает и когда,
//! решает `camera-core`, как это сделать на телефоне — `camera-android`.
//!
//! Инвариант: crate не зависит ни от Android, ни от Vulkan, ни от других crate-ов workspace —
//! это проверяет `scripts/check-crate-boundaries.py` (правила `neutral-no-platform`
//! и `contract-is-leaf` в `scripts/crate-boundaries.toml`).
//!
//! Раскладка:
//! * [`ids`] — ID камеры, роль объектива, направление, логическая или физическая камера;
//! * [`geometry`] — размеры и прямоугольники в пикселях сенсора;
//! * [`raw_format`] — формат RAW-буфера и мозаика (CFA, Bayer / Quad Bayer / RGBW, решение 10);
//! * [`capabilities`] — что камера умеет: RAW-потоки, экспозиция, фокус, оптика, цвет,
//!   уровни сенсора и вендорская обработка (инвариант 2 README);
//! * [`catalog`] — каталог видимых камер и привязка ролей;
//! * [`stream`] — режим потока (формат, вырезка, частота) и то, как частота была достигнута;
//! * [`control`] — намерения управления: шаблон запроса, ручная экспозиция и фокус;
//! * [`capture_result`] — метаданные результата запроса, нужные движку и smoke;
//! * [`frame_passport`] — паспорт RAW-кадра: постоянная часть потока и покадровые метаданные;
//! * [`auto_exposure_carry`] — отпечаток камеры и ожидаемая экспозиция нового потока (13b);
//! * [`auto_exposure`] — словарь нашего AE (08a): произведение экспозиции, предложение замера
//!   (рендер → движок) с поздно привязываемым толчком, указание AE (движок → рендер);
//! * [`flash`] — вспышка: возможность камеры, ключ запроса, состояние в результате, замеры
//!   предвспышки и план кадра со вспышкой (14d, решения 153, 158);
//! * [`lens_calibration`] — оптическая ось и положение объектива, проверка правдоподобия (13);
//! * [`lens_shading`] — калибровка виньетирования: сетка усилений, привязка потока к активной области;
//! * [`white`] — словарь белого: указание рендеру, заявка источнику, исходы, ящик ответов (white-balance 01);
//! * [`zoom`] — шкала зума всех камер направления, намерение экрана, формула кадрирования (13);
//! * [`availability`] — почему камера недоступна (вытеснение, ошибка HAL, разрешение…);
//! * `fixtures` (фича `fixtures`, только тесты) — образцы камер OnePlus 11 по фактам 00.

pub mod auto_exposure;
pub mod auto_exposure_carry;
pub mod availability;
pub mod capabilities;
pub mod capture_result;
pub mod catalog;
pub mod control;
#[cfg(any(test, feature = "fixtures"))]
pub mod fixtures;
pub mod flash;
pub mod frame_passport;
pub mod geometry;
pub mod ids;
pub mod lens_calibration;
pub mod lens_shading;
pub mod raw_format;
pub mod stream;
pub mod three_a;
pub mod viewfinder_crop;
pub mod white;
pub mod zoom;
