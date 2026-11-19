//! K8 «Плашка» (`components.md`): AE/AF LOCK, отсчёт таймера; дальше — FOCUS TRACKING, подсказки.
//!
//! Плашка не знает текста: вызывающий даёт ключ строки ([`crate::picker::LabelId`]), текст и
//! перевод — забота экрана. Время жизни — [`BannerLifetime`]: «до команды скрыть» или «столько-то».
//! Вида нет: элемент отдаёт [`BannerSnapshot`] (прозрачность и масштаб), рисует
//! [`crate::view::banner_items`].
//!
//! Появление и скрытие — одна пружина `smooth` (`motion.md` 4.2: «растворение» без перелёта).
//! Скрытие по истечению срока стартует **в момент истечения**, а не в момент кадра, который его
//! заметил: поэтому положение в заданный момент не зависит от частоты кадров.

mod element;
mod error;

pub use element::{
    Banner, BannerEvent, BannerHideOutcome, BannerLifetime, BannerPhase, BannerShowOutcome, BannerSnapshot, HideCause,
};
pub use error::BannerError;
