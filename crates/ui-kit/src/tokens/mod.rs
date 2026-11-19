//! Токены `ui-kit`: числа движения, плотности и хаптики — только здесь (`components.md`, правило 4).
//!
//! Имена `camera_snap` / `camera_sheet` и события хаптики экрана — это дизайн-система экрана камеры
//! (одно место для всех чисел экрана, `components.md` раздел 3); **элементы** (`scale`, `carousel`,
//! `picker`) предметной области не знают и берут токены только по роли ([`SnapRole`]).
//!
//! * [`MotionTokens`] — именованные пружины и сборка конфигов прокрутки по роли;
//! * [`HapticTable`] — таблица «событие → хаптика»;
//! * [`DensityTokens`] — pt на единицу шкалы, радиус магнита, промежуток карусели;
//! * [`UiKitTokens`] — все вместе: serde-конфиг с `Default` и `validate()`.

mod density;
mod error;
mod haptics;
mod motion;

use serde::{Deserialize, Serialize};

pub use density::{DensityTokens, ScaleDensity};
pub use error::TokenError;
pub use haptics::{HapticEvent, HapticSetting, HapticTable};
pub use motion::{MotionToken, MotionTokens, SnapRole, SpringToken};

/// Все токены элементов. Владелец — экран; элементы берут из него копии нужных чисел при создании.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UiKitTokens {
    pub motion: MotionTokens,
    pub density: DensityTokens,
    pub haptics: HapticTable,
}

impl UiKitTokens {
    /// Проверить все группы токенов.
    pub fn validate(&self) -> Result<(), TokenError> {
        self.motion.validate()?;
        self.density.validate()
    }
}
