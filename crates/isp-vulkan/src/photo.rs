//! Проявление снимка на GPU (сессия 14, пакет P3): продуктовый конвейер «сырые данные кадра → картинка снимка»
//! поверх частей стенда V4 ([`crate::quality`]), в две стадии, обе не должны тормозить видоискатель.
//!
//! # Стадия 1 — захват ([`capture`], ≈ один dispatch)
//! Сырые отсчёты полезной области кадра камеры копируются в буфер снимка, видимый процессору (u16
//! little-endian): [`RawCapturer::record_capture`] → fence → [`RawCapturer::complete_after_fence`] →
//! [`RawCapture`]. После этого кадр можно вернуть камере; байты уходят в DNG как `RawSamples16`.
//!
//! # Стадия 2 — проявление по полосам ([`developer`])
//! Quality-цепочка (уровни+баланс → Menon 2007 → матрицы, охват, тон, OETF, квантование в sRGB 8 бит) делается
//! полосами по строкам между кадрами видоискателя: [`PhotoDeveloper::begin`], затем по полосе
//! [`PhotoDeveloper::record_next_slice`] + [`PhotoDeveloper::slice_completed`], в конце
//! [`PhotoDeveloper::finish`] → [`DevelopedPhoto`]. Каждой полосе нужен ореол ([`slicing::DEMOSAIC_HALO`]),
//! чтобы результат был бит-в-бит равен проявлению целого кадра. Правило квантования — [`quantize`].
//!
//! # Миниатюра (14g, [`thumbnail`])
//! В ПОСЛЕДНЕЙ полосе, в том же командном буфере, готовая картинка уменьшается на GPU до `thumbnail_long_side_px`
//! (усреднение по площади в линейном свете, поворот «как покажет галерея») и копируется в буфер хоста; печатается
//! тем же доказательством, что и снимок. [`DevelopedPhoto::take_thumbnail`] отдаёт байты в обычной памяти. Сбой
//! миниатюры проявку не ломает ([`ThumbnailUnavailable`]).
//!
//! # Владельцы и границы
//! * кадр камеры: рендер (лизинг); этот модуль только читает его в `record_capture`;
//! * буфер сырых отсчётов: [`RawCapture`] (общий владелец, держит буфер живым);
//! * плоскости, пайплайны: [`PhotoDeveloper`]; картинка снимка: [`DevelopedPhoto`];
//! * fence-ы и сабмиты: рендер. Этот модуль не ждёт GPU и не создаёт fence-ов.

pub mod capture;
mod config;
pub mod developed;
pub mod developer;
mod error;
mod finishing;
mod pixels;
pub mod quantize;
mod slice_pushes;
pub mod slicing;
mod targets;
pub mod thumbnail;
mod thumbnail_pass;
pub mod timing;

pub use self::capture::{PendingRawCapture, RawCapture, RawCapturer};
pub use self::config::PhotoConfig;
pub use self::developed::{DevelopRequest, DevelopedPhoto, DevelopmentStart, RgbaReadback, SliceOutcome};
pub use self::developer::PhotoDeveloper;
pub use self::error::PhotoError;
pub use self::quantize::quantize_display_encoded;
pub use self::slicing::{DEMOSAIC_HALO, DevelopmentPlan, SlicePlan, max_region_rows, plan_development};
pub use self::targets::DevelopTargets;
pub use self::thumbnail::{ThumbnailOutcome, ThumbnailPlan, ThumbnailUnavailable, reference_thumbnail, rotated_size};
pub use self::thumbnail_pass::ThumbnailTargets;
pub use self::timing::{PhotoGpuPoint, PhotoGpuTimingCollect, PhotoGpuTimings};
