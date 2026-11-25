//! `isp-model` — эталонный ISP на CPU в f32 (сессия 06).
//!
//! # Зачем
//! GPU-ISP (сессии 07 и 14) нельзя проверять «на глаз». Этот crate — медленный, но понятный
//! источник истины: каждая стадия — отдельная чистая функция с формулой и ссылкой на
//! спецификацию или статью. GPU сверяется с ним с явным допуском, стадия за стадией.
//!
//! # Цепочка
//! ```text
//! RawMosaic (u16) ──levels──▶ CameraMosaic ──lens_shading (наша калибровка камеры)──▶ ShadingCorrectedMosaic
//!   ──white_balance──▶ WhiteBalancedMosaic ──demosaic (Preview 2×2 | Quality Menon 2007)──▶ LinearCameraRgb
//!   ──color: камера → XYZ D50 (матрицы DNG)──▶ XyzD50Image ──Bradford + основные цвета экрана──▶ SceneLinearImage
//!   ──gamut: мягко в охват экрана──▶ SceneInGamutImage ──tone (S-кривая, сохранение оттенка)──▶ DisplayLinearImage
//!   ──transfer (sRGB OETF)──▶ DisplayEncodedImage
//! ```
//! Это прежняя основа тона ([`tone::base::ToneBase::Legacy`], по умолчанию). Основа AgX (решения 101, 176, 177)
//! идёт иначе: цвет сразу в линейный Rec. 2020, обрезка пересвета — по своему белому каждой точки
//! ([`white_level`]), тон формируется в Rec. 2020, цвета выхода — последним шагом:
//! ```text
//! ShadingCorrectedMosaic ──white_level: min(отсчёт·wb, L(pos))──▶ WhiteBalancedMosaic ──demosaic──▶ LinearCameraRgb
//!   ──камера → XYZ D50──▶ XyzD50Image ──Брэдфорд + Rec. 2020──▶ WorkingSceneImage
//!   ──tone::agx: вход, inset, log2, S-кривая до белого L, outset──▶ WorkingDisplayImage
//!   ──W → цвета выхода, мягкое сжатие охвата, обрезка 0…1──▶ DisplayLinearImage ──transfer──▶ DisplayEncodedImage
//! ```
//! Вид картинки ([`look`], решения 98, 99, 178) — после основы, в том же рабочем пространстве `W`:
//! ```text
//!   ... ──основа (AgX | старая)──▶ свет экрана в W ──look::tone_shape (контраст, средние, тени, света)──▶
//!   ──look::color (насыщенность, сочность, полосы; OKLab)──▶
//!   ──look::lut (задел, тетраэдры)──▶ дальше как выше (AgX: W → цвета выхода; старая: обрезка 0…1)
//! ```
//! Теплота и оттенок сдвигают белую точку только для рисования ([`look::white`]). Нейтральный вид — выход основы бит
//! в бит.
//! Прогон: [`pipeline::run_reference_working`] (прежняя основа — [`pipeline::run_reference`]).
//! Сбоку от цепочки: [`statistics`] — суммы каналов по плиткам из `ShadingCorrectedMosaic` для нашего
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
//! # use isp_model::{image::SceneLinearImage, output::OutputTransfer, transfer};
//! // Кривую передачи нельзя применить к сцене до тона: сначала тон-кривая.
//! fn wrong(scene: SceneLinearImage) {
//!     let _ = transfer::encode_display(&scene, OutputTransfer::Srgb);
//! }
//! ```
//!
//! ```
//! # use isp_model::{image::DisplayLinearImage, output::OutputTransfer, transfer};
//! fn right(display: DisplayLinearImage) {
//!     let _ = transfer::encode_display(&display, OutputTransfer::Srgb);
//! }
//! ```
//!
//! ```compile_fail
//! # use isp_model::{image::WorkingSceneImage, output::OutputTransfer, transfer};
//! // Сцену в Rec. 2020 нельзя кодировать кривой передачи до основы тона и перевода в цвета выхода.
//! fn wrong(scene: WorkingSceneImage) {
//!     let _ = transfer::encode_display(&scene, OutputTransfer::Srgb);
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
//! 2. Виньетирование HAL частичное (`LENS_SHADING_APPLIED = true` значит «не полностью»): углы RAW
//!    на 2,2–3 EV темнее центра, поэтому стадия 2 корректирует по нашей статичной калибровке
//!    камеры; карту HAL не используем. Усиление в углах поднимает и шум — см. [`shading_noise`].
//! 3. Константы алгоритмов — со ссылкой на источник; настраиваемое — в параметрах, не в коде.

pub mod cfa;
pub mod color;
pub mod demosaic;
pub mod exposure;
pub mod gamut;
pub mod image;
pub mod lens_shading;
pub mod levels;
pub mod look;
pub mod metrics;
pub mod output;
pub mod params;
pub mod pipeline;
pub mod sensor;
pub mod shading_noise;
#[cfg(test)]
mod shading_pipeline_tests;
pub mod statistics;
pub mod test_patterns;
#[cfg(test)]
mod test_support;
pub mod tone;
pub mod transfer;
pub mod white_balance;
pub mod white_level;
