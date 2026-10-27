//! `isp-model` — эталонный ISP на CPU в f32 (сессия 06).
//!
//! # Зачем
//! GPU-ISP (сессии 07 и 14) нельзя проверять «на глаз». Этот crate — медленный, но понятный
//! источник истины: каждая стадия — отдельная чистая функция с формулой и ссылкой на
//! спецификацию или статью. GPU сверяется с ним с явным допуском, стадия за стадией.
//!
//! # Цепочка
//! ```text
//! RawMosaic (u16) ──levels──▶ CameraMosaic ──lens_shading (если вендор не применил)──▶ ShadingCorrectedMosaic
//!   ──white_balance──▶ WhiteBalancedMosaic ──demosaic (Preview 2×2 | Quality Menon 2007)──▶ LinearCameraRgb
//!   ──color: камера → XYZ D50 (матрицы DNG)──▶ XyzD50Image ──Bradford + основные цвета экрана──▶ SceneLinearImage
//!   ──gamut: мягко в охват экрана──▶ SceneInGamutImage ──tone (S-кривая, сохранение оттенка)──▶ DisplayLinearImage
//!   ──transfer (sRGB OETF)──▶ DisplayEncodedImage
//! ```
//! Сбоку от цепочки: [`statistics`] — суммы каналов по плиткам из `CameraMosaic` для нашего
//! AWB (08), целочисленный контракт для точной сверки с GPU-видоискателем.
//!
//! Типы стадий не дают подать изображение не в ту функцию — это проверяется компилятором:
//!
//! ```compile_fail
//! # use isp_model::{color, image::CameraMosaic, params::IspParams};
//! // Мозаику без демозаика нельзя подать в преобразование цвета.
//! fn wrong(mosaic: &CameraMosaic, params: &IspParams) {
//!     let _ = color::stage::camera_rgb_to_xyz_d50(mosaic, params.color.camera_to_xyz_d50());
//! }
//! ```
//!
//! Тот же код с правильным типом компилируется (иначе compile_fail выше ничего бы не доказывал):
//!
//! ```
//! # use isp_model::{color, image::LinearCameraRgb, params::IspParams};
//! fn right(camera_rgb: &LinearCameraRgb, params: &IspParams) {
//!     let _ = color::stage::camera_rgb_to_xyz_d50(camera_rgb, params.color.camera_to_xyz_d50());
//! }
//! ```
//!
//! ```compile_fail
//! # use isp_model::{image::SceneLinearImage, transfer};
//! // Кривую передачи нельзя применить к сцене до тона: сначала тон-кривая.
//! fn wrong(scene: SceneLinearImage) {
//!     let _ = transfer::encode_display(&scene);
//! }
//! ```
//!
//! ```
//! # use isp_model::{image::DisplayLinearImage, transfer};
//! fn right(display: DisplayLinearImage) {
//!     let _ = transfer::encode_display(&display);
//! }
//! ```
//!
//! # Параметры
//! [`params::IspParams`] — разрешённые значения на кадр, одинаковые для CPU и GPU (паритет
//! сравнивает одно и то же). Их собирает [`params::IspParams::resolve`] из описания сенсора
//! ([`sensor::SensorDescription`], из паспорта кадра) и управления
//! ([`params::ProcessingControls`]: баланс белого от 3A, тон, гамма выхода). Вся цветовая
//! математика DNG считается здесь один раз на кадр.
//!
//! # Инварианты
//! 1. Без Android и GPU (guardrail `neutral-no-platform`).
//! 2. Вендорскую обработку не повторяем: виньетирование, применённое HAL, второй раз не
//!    корректируется (README, инвариант 2).
//! 3. Константы алгоритмов — со ссылкой на источник; настраиваемое — в параметрах, не в коде.

pub mod cfa;
pub mod color;
pub mod demosaic;
pub mod gamut;
pub mod image;
pub mod lens_shading;
pub mod levels;
pub mod metrics;
pub mod params;
pub mod pipeline;
pub mod sensor;
pub mod statistics;
pub mod test_patterns;
#[cfg(test)]
mod test_support;
pub mod tone;
pub mod transfer;
pub mod white_balance;
