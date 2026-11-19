//! Настраиваемые параметры стекла (в pt и долях).

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Параметры стекла заданы неверно.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum GlassStyleError {
    #[error("параметр «{field}» не число: {value}")]
    NotFinite { field: &'static str, value: f32 },
    #[error("параметр «{field}» не может быть отрицательным: {value}")]
    Negative { field: &'static str, value: f32 },
    #[error("параметр «{field}» вне диапазона {min}…{max}: {value}")]
    OutOfRange {
        field: &'static str,
        value: f32,
        min: f32,
        max: f32,
    },
    #[error("параметр «{field}»: нижняя граница {low} не меньше верхней {high}")]
    InvertedRange { field: &'static str, low: f32, high: f32 },
}

/// Проверка «конечное и ≥ 0».
pub(crate) fn check_non_negative(field: &'static str, value: f32) -> Result<(), GlassStyleError> {
    if !value.is_finite() {
        Err(GlassStyleError::NotFinite { field, value })
    } else if value < 0.0 {
        Err(GlassStyleError::Negative { field, value })
    } else {
        Ok(())
    }
}

/// Проверка «конечное и в диапазоне».
pub(crate) fn check_range(field: &'static str, value: f32, min: f32, max: f32) -> Result<(), GlassStyleError> {
    if !value.is_finite() {
        Err(GlassStyleError::NotFinite { field, value })
    } else if value < min || value > max {
        Err(GlassStyleError::OutOfRange { field, value, min, max })
    } else {
        Ok(())
    }
}

/// Размер, зависящий либо от формы (доля), либо абсолютный.
///
/// Доля считается от МЕНЬШЕЙ стороны формы («высота» кнопки-капсулы; диаметр круга): так толщина
/// стекла растёт вместе с формой (Apple 219, 7:02), а у вытянутой панели не раздувается по длине.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Extent {
    /// Доля меньшей стороны формы.
    FractionOfHeight(f32),
    /// Абсолютный размер в pt.
    Points(f32),
}

impl Extent {
    /// Размер в пикселях для формы с меньшей стороной `reference_side_px`.
    pub fn resolve_px(self, reference_side_px: f32, px_per_pt: f32) -> f32 {
        match self {
            Extent::FractionOfHeight(fraction) => fraction * reference_side_px,
            Extent::Points(points) => points * px_per_pt,
        }
    }

    pub(crate) fn validate(self, field: &'static str) -> Result<(), GlassStyleError> {
        match self {
            // Больше 4 высот — заведомо опечатка; верхний предел ловит «50» вместо «0,5».
            Extent::FractionOfHeight(fraction) => check_range(field, fraction, 0.0, 4.0),
            Extent::Points(points) => check_non_negative(field, points),
        }
    }
}

/// Профиль нормированного смещения от края к центру зоны линзы (1 у края, 0 в конце зоны).
///
/// Обе формулы одного семейства `1 − (1 − uⁿ)^(1/n)`, где `u = 1 − depth/zone`: `u = 1` на краю
/// формы, `u = 0` в конце зоны.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeProfile {
    /// «Четверть окружности» (Kyant0/AndroidLiquidGlass, `[reverse]`, Apache-2.0):
    /// `x = 1 − depth/zone`, `offset = 1 − √(1 − x²)`, то есть `n = 2`.
    QuarterCircle,
    /// «Выпуклый суперэллипс» — по умолчанию. Профиль высоты края у kube.io
    /// `y = (1 − (1 − x)⁴)^(1/4)`, по словам автора, ближе к Apple (`[reverse]`; идея, не код).
    /// Нормированное смещение — единица минус эта высота, считая от края: `n = 4`,
    /// `offset = 1 − (1 − u⁴)^(1/4)`. Монотонно, 1 у края, 0 в конце зоны; концентрируется
    /// у самого края сильнее, чем окружность. Подгонка, не Apple.
    ConvexSuperellipse,
}

/// Блик-«ободок»: белая полоса вдоль края, яркость от направления света.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SpecularStyle {
    /// Максимальная прозрачность белого блика (сложение). `[reverse]` Kyant: 0,5.
    pub alpha: f32,
    /// Ширина ободка от края. `[estimate]`: подгонка, не Apple.
    pub rim_width: Extent,
    /// Степень в `|dot(нормаль, свет)|^exponent`. `[reverse]` Kyant: 1.
    pub exponent: f32,
    /// Угол света в покое, градусы; по часовой стрелке от оси x экрана (y вниз). `[reverse]` Kyant: 45.
    pub rest_angle_deg: f32,
}

impl Default for SpecularStyle {
    fn default() -> Self {
        Self {
            alpha: 0.5,
            rim_width: Extent::Points(2.0),
            exponent: 1.0,
            rest_angle_deg: 45.0,
        }
    }
}

/// Тёмный край iOS 27 (WWDC26 102, 22:43): затемнение у самой границы. Числа неизвестны.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DarkEdgeStyle {
    /// Прозрачность чёрного у самого края. `[estimate]`: подгонка, не Apple.
    pub strength: f32,
    /// Ширина тёмной полосы. `[estimate]`: подгонка, не Apple.
    pub width: Extent,
}

impl Default for DarkEdgeStyle {
    fn default() -> Self {
        Self {
            strength: 0.15,
            width: Extent::Points(3.0),
        }
    }
}

/// Адаптивное затемнение: тон темнеет над ярким фоном (`glass.md`, раздел 3).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DimmingStyle {
    /// Потолок затемнения. `[doc]` HIG Materials: 35 % для яркого фона.
    pub max_alpha: f32,
    /// Яркость фона (Rec.709 по закодированным), до которой затемнения нет. `[estimate]`.
    pub luminance_low: f32,
    /// Яркость фона, с которой затемнение полное. `[estimate]`.
    pub luminance_high: f32,
}

impl Default for DimmingStyle {
    fn default() -> Self {
        Self {
            max_alpha: 0.35,
            luminance_low: 0.35,
            luminance_high: 0.85,
        }
    }
}

/// Тень под стеклом.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShadowStyle {
    /// Радиус размытия тени, pt. `[reverse]` Kyant: 24 dp; dp приравниваем к pt, как все числа
    /// Kyant в `glass.md` раздел 6. Подгонка, не Apple.
    pub radius_pt: f32,
    /// Прозрачность чёрного. `[reverse]` Kyant: 0,1.
    pub alpha: f32,
    /// Смещение вниз как доля радиуса. `[reverse]` Kyant: 1/6.
    pub offset_ratio: f32,
}

impl Default for ShadowStyle {
    fn default() -> Self {
        Self {
            radius_pt: 24.0,
            alpha: 0.1,
            offset_ratio: 1.0 / 6.0,
        }
    }
}

/// Реакция на нажатие.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PressStyle {
    /// Прирост масштаба при полном нажатии: масштаб = `1 + scale_gain · amount`. `[estimate]`:
    /// «~1,1» из `glass.md` раздел 6. Масштаб запекает тот, кто строит список рисования.
    pub scale_gain: f32,
    /// Во сколько раз сильнее преломление при полном нажатии: `1 + refraction_boost · amount`.
    /// `[estimate]`, подгонка.
    pub refraction_boost: f32,
    /// Радиус свечения от точки касания, в долях меньшей стороны формы. `[reverse]` Kyant: 1,5.
    pub glow_radius_ratio: f32,
    /// Прозрачность белого свечения при полном нажатии. `[estimate]`, подгонка.
    pub glow_alpha: f32,
}

impl Default for PressStyle {
    fn default() -> Self {
        Self {
            scale_gain: 0.1,
            refraction_boost: 0.5,
            glow_radius_ratio: 1.5,
            glow_alpha: 0.2,
        }
    }
}

/// Все параметры материала. Значения по умолчанию — пресет «кнопка» (`glass.md`, раздел 6);
/// пресеты кнопки и панели лежат в токенах `ui-kit`.
///
/// Появление «материализацией» (интенсивность эффекта 0..1, Apple 219, 2:55) сюда НЕ входит: это
/// параметр элемента списка рисования, меняющийся во времени, а не свойство материала.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct GlassStyle {
    pub edge_profile: EdgeProfile,
    /// Ширина зоны линзы от края. `[reverse]` Kyant: 12 dp на кнопке 48 dp = 25 % высоты.
    pub lens_zone: Extent,
    /// Максимальное смещение выборки фона у края. `[reverse]` Kyant: 24 dp на 48 dp = 50 % высоты.
    pub refraction_strength: Extent,
    /// Радиус размытия фона, pt (его выполняет источник фона, не формула). `[reverse]` Kyant: 2 dp.
    pub blur_radius_pt: f32,
    /// Насыщенность фона под стеклом (1 — без изменений). `[reverse]` Kyant: 1,5.
    pub saturation: f32,
    /// Дисперсия δ: каналы R и B смещаются в `1 ∓ δ` раз. `[reverse]`: 0,02, «почти нет радуги».
    pub dispersion: f32,
    pub specular: SpecularStyle,
    pub dark_edge: DarkEdgeStyle,
    pub dimming: DimmingStyle,
    /// Ползунок iOS 27 «ультрапрозрачное … полностью тонированное», 0…1 (`glass.md`, раздел 4).
    /// Целимся в Default, а он «более тонирован, чем Clear iOS 26» — `[estimate]`.
    pub tint_level: f32,
    /// Прозрачность плёнки тона при `tint_level = 1`. `[estimate]`, подгонка.
    pub tint_alpha_at_full: f32,
    pub shadow: ShadowStyle,
    pub press: PressStyle,
}

impl Default for GlassStyle {
    fn default() -> Self {
        Self {
            edge_profile: EdgeProfile::ConvexSuperellipse,
            lens_zone: Extent::FractionOfHeight(0.25),
            refraction_strength: Extent::FractionOfHeight(0.5),
            blur_radius_pt: 2.0,
            saturation: 1.5,
            dispersion: 0.02,
            specular: SpecularStyle::default(),
            dark_edge: DarkEdgeStyle::default(),
            dimming: DimmingStyle::default(),
            tint_level: 0.3,
            tint_alpha_at_full: 0.5,
            shadow: ShadowStyle::default(),
            press: PressStyle::default(),
        }
    }
}

impl GlassStyle {
    /// Проверить числа; вызывается один раз при загрузке конфига.
    pub fn validate(&self) -> Result<(), GlassStyleError> {
        self.lens_zone.validate("lens_zone")?;
        self.refraction_strength.validate("refraction_strength")?;
        check_non_negative("blur_radius_pt", self.blur_radius_pt)?;
        check_range("saturation", self.saturation, 0.0, 4.0)?;
        check_range("dispersion", self.dispersion, 0.0, 0.5)?;
        check_range("specular.alpha", self.specular.alpha, 0.0, 1.0)?;
        self.specular.rim_width.validate("specular.rim_width")?;
        check_range("specular.exponent", self.specular.exponent, 0.1, 16.0)?;
        check_range("specular.rest_angle_deg", self.specular.rest_angle_deg, -360.0, 360.0)?;
        check_range("dark_edge.strength", self.dark_edge.strength, 0.0, 1.0)?;
        self.dark_edge.width.validate("dark_edge.width")?;
        check_range("dimming.max_alpha", self.dimming.max_alpha, 0.0, 1.0)?;
        check_range("dimming.luminance_low", self.dimming.luminance_low, 0.0, 1.0)?;
        check_range("dimming.luminance_high", self.dimming.luminance_high, 0.0, 1.0)?;
        if self.dimming.luminance_low >= self.dimming.luminance_high {
            return Err(GlassStyleError::InvertedRange {
                field: "dimming.luminance",
                low: self.dimming.luminance_low,
                high: self.dimming.luminance_high,
            });
        }
        check_range("tint_level", self.tint_level, 0.0, 1.0)?;
        check_range("tint_alpha_at_full", self.tint_alpha_at_full, 0.0, 1.0)?;
        check_non_negative("shadow.radius_pt", self.shadow.radius_pt)?;
        check_range("shadow.alpha", self.shadow.alpha, 0.0, 1.0)?;
        check_range("shadow.offset_ratio", self.shadow.offset_ratio, 0.0, 2.0)?;
        check_range("press.scale_gain", self.press.scale_gain, 0.0, 1.0)?;
        check_range("press.refraction_boost", self.press.refraction_boost, 0.0, 4.0)?;
        check_range("press.glow_radius_ratio", self.press.glow_radius_ratio, 0.0, 8.0)?;
        check_range("press.glow_alpha", self.press.glow_alpha, 0.0, 1.0)?;
        Ok(())
    }
}
