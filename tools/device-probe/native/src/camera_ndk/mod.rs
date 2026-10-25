//! Разведка Camera2 NDK: каталог камер (`ACameraManager`).
//! `manager` — тонкие RAII-обёртки над FFI (весь unsafe здесь), `catalog` — сбор отчёта.

mod catalog;
mod manager;

pub use catalog::build_catalog_json;
