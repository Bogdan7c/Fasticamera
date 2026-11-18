//! Исполнители 3A сессии 08: вендорский автофокус через HAL и **временный** вендорский AE.
//!
//! * [`VendorFocus`] — остаётся и после 08a (решение 66: вендорский AF проверен и работает);
//! * [`VendorExposure`] — **временная** реализация [`crate::three_a_control::ExposureController`]
//!   до сессии 08a, где её целиком заменит наш AE по статистике RAW (решения 63 и 66). Движок
//!   о ней ничего не знает, кроме трейта, — замена не трогает ни планировщик, ни соседей.
//!
//! Сменные заглушки для тестов движка — в `scripted` (под `fake-backend` и в тестах).

mod exposure;
mod focus;
#[cfg(any(test, feature = "fake-backend"))]
pub mod scripted;

pub use exposure::VendorExposure;
pub use focus::VendorFocus;
