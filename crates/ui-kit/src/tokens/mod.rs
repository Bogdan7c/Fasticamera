//! Токены `ui-kit`: числа движения, плотности и хаптики — только здесь (`components.md`, правило 4).
//!
//! Имена `camera_snap` / `camera_sheet` и события хаптики экрана — это дизайн-система экрана камеры
//! (одно место для всех чисел экрана, `components.md` раздел 3); **элементы** (`scale`, `carousel`,
//! `picker`) предметной области не знают и берут токены только по роли ([`SnapRole`]).
//!
//! * [`MotionTokens`] — именованные пружины и сборка конфигов прокрутки по роли;
//! * [`HapticTable`] — таблица «событие → хаптика»;
//! * [`GlassTokens`] — пресеты материала стекла «кнопка» и «панель»;
//! * [`DensityTokens`] — pt на единицу шкалы, радиус магнита, промежуток карусели;
//! * [`TypographyTokens`] — роли текста (начертание, кегль, трекинг, цифры);
//! * [`ColorTokens`] — цвета фокуса, плашки, текста, затемнения;
//! * [`ScaleTickTokens`] — размеры рисок и отметки текущего значения шкалы;
//! * [`ArcScaleTokens`] — вид дуговой шкалы (диск, кольцо рисок, затухание);
//! * [`DiscPadTokens`] — ведение и вид «точки в круге» (K12);
//! * [`PanelTokens`], [`BannerTokens`], [`FocusTokens`], [`ShutterTokens`] — размеры и параметры K7–K10;
//! * [`UiKitTokens`] — все вместе: serde-конфиг с `Default` и `validate()`.

mod arc_scale;
mod colors;
mod density;
mod disc_pad;
mod elements;
mod error;
mod glass;
mod haptics;
mod motion;
mod ruler;
mod scale_view;
mod typography;

use serde::{Deserialize, Serialize};

pub use crate::haptic_throttle::HapticThrottleTokens;
pub use arc_scale::ArcScaleTokens;
pub use colors::ColorTokens;
pub use density::{DensityTokens, ScaleDensity};
pub use disc_pad::DiscPadTokens;
pub use elements::{BannerTokens, FocusTokens, PanelTokens, ShutterTokens};
pub use error::TokenError;
pub use glass::{GlassRole, GlassTokens};
pub use haptics::{HapticEvent, HapticSetting, HapticTable};
pub use motion::{MotionToken, MotionTokens, SnapRole, SpringToken};
pub use ruler::RulerTokens;
pub use scale_view::ScaleTickTokens;
pub use typography::{DigitStyle, TextRole, TextRoleKind, TextWeight, Tracking, TypographyTokens};

/// Все токены элементов. Владелец — экран; элементы берут из него копии нужных чисел при создании.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UiKitTokens {
    pub motion: MotionTokens,
    pub density: DensityTokens,
    pub haptics: HapticTable,
    /// Ограничитель частоты вибраций (колесо и щипок зума).
    pub haptic_throttle: HapticThrottleTokens,
    pub glass: GlassTokens,
    pub typography: TypographyTokens,
    pub colors: ColorTokens,
    pub panel: PanelTokens,
    pub banner: BannerTokens,
    pub focus: FocusTokens,
    pub shutter: ShutterTokens,
    pub scale_ticks: ScaleTickTokens,
    pub arc_scale: ArcScaleTokens,
    pub ruler: RulerTokens,
    pub disc_pad: DiscPadTokens,
}

impl UiKitTokens {
    /// Проверить все группы токенов.
    pub fn validate(&self) -> Result<(), TokenError> {
        self.motion.validate()?;
        self.density.validate()?;
        self.haptic_throttle.validate()?;
        self.glass.validate()?;
        self.typography.validate()?;
        self.colors.validate()?;
        self.panel.validate()?;
        self.banner.validate()?;
        self.focus.validate()?;
        self.shutter.validate()?;
        self.scale_ticks.validate()?;
        self.arc_scale.validate()?;
        self.ruler.validate()?;
        self.disc_pad.validate()
    }
}
