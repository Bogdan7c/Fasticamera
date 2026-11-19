//! K10 «Затвор» (`components.md`): Photo (12); дальше — запись, серия, ночь.
//!
//! Элемент отвечает за **касание**: механика нажатия — тот же [`PressTracker`](crate::button::PressTracker),
//! что у любой кнопки (K2), плюс анимация нажатия. Машину состояний затвора (М2 из `feature-map.md`:
//! готов, идёт снимок, недоступен) держит экран: элемент получает от неё только вид
//! [`ShutterLook`] и сам ничего не решает о съёмке.
//!
//! Вид нажатия — пружина `smooth` без перелёта (`glass.md`: «пружина без перелёта»): диск сжимается
//! до `shutter.pressed_disc_scale`, стеклянное кольцо берёт масштаб нажатия из пресета стекла.
//! Рисует [`crate::view::shutter_items`].

mod element;
mod error;

pub use element::{ShutterButton, ShutterLook, ShutterSnapshot};
pub use error::ShutterError;
