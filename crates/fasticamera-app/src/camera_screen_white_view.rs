//! Белое глазами отчёта экрана: что экран держит о белом в своих фактах (white-balance 02).
//!
//! Сценарий `screen` сверяет этим снимком, что число и происхождение, которые показывает экран, — те же, что
//! держит движок: так проверяется весь путь «движок → факты → экран» (пакет D5), а не только то, что движок принял
//! команду. Сам рисунок числа здесь не проверяется: это дело виджета панели.

use camera_contract::white::WhiteOrigin;
use camera_screen::ScreenState;
use serde::Serialize;

/// Снимок белого, как его видит экран. `Eq`: Кельвины округлены до целого, чтобы снимок оставался сравнимым.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WhiteView {
    /// Кто поставил белое, по фактам экрана.
    pub origin: WhiteOrigin,
    /// Цветовая температура в фактах экрана, К, округлённая до целого.
    pub kelvin: u32,
    /// Сколько раз экран отметил отказ команды «взять» (тряска и вибрация «ошибка»).
    pub refusals: u64,
}

impl WhiteView {
    /// Снимок по состоянию экрана.
    pub fn observe(state: &ScreenState) -> Self {
        let held = state.camera_facts().white.held;
        Self {
            origin: held.origin,
            kelvin: kelvin_rounded(held.setting.temperature.kelvin()),
            refusals: state.white().refusal_serial(),
        }
    }

    /// Белое до первых фактов: запасной дневной свет (для тестов, где снимок собирают руками).
    pub const fn starting() -> Self {
        Self {
            origin: WhiteOrigin::Fallback,
            kelvin: 5_600,
            refusals: 0,
        }
    }
}

/// Кельвины, округлённые до целого. Единственное место округления: движок и экран сравниваются одним правилом.
pub(crate) fn kelvin_rounded(kelvin: f64) -> u32 {
    // Кельвины конечны и лежат в пределах движка (тысячи): приведение не обрезает значащих разрядов.
    kelvin.round().max(0.0) as u32
}
