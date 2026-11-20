//! Размеры и параметры вида элементов K7–K10: панель, плашка, рамка фокуса, затвор.
//!
//! Числа замеров — `layout.md` раздел 2.2 (видео A, `[measured]`, погрешность ±1 pt); всё, чего
//! замеры не дают, — `[estimate]`, «подгонка, не Apple». Единицы — pt (решение 82), доли — 0..1.

use serde::{Deserialize, Serialize};
use ui_draw::color::DisplayColor;

use super::error::TokenError;

/// Проверка «конечно и строго положительно» для именованного токена.
pub(super) fn positive(name: &'static str, value: f64) -> Result<(), TokenError> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(TokenError::NonPositiveSize { name, value })
    }
}

/// Проверка «конечно и в 0..=1» для именованного токена.
fn unit(name: &'static str, value: f64) -> Result<(), TokenError> {
    if value.is_finite() && (0.0..=1.0).contains(&value) {
        Ok(())
    } else {
        Err(TokenError::OutOfRange { name, value })
    }
}

/// K7 «Панель-морф».
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PanelTokens {
    /// На сколько pt пункты «въезжают снизу» (`motion.md` 4.1: сдвиг 55 → 0 px видео, масштаб видео
    /// не установлен). `[estimate]` — подгонка, не Apple.
    pub content_slide_distance_pt: f64,
    /// С какой доли раскрытия (0..1) начинает проявляться содержимое: пока стекло материализуется,
    /// пункты ещё не видны (`motion.md` 4.1: пункты не масштабируются, а въезжают). `[estimate]`.
    pub content_reveal_start: f64,
}

impl Default for PanelTokens {
    fn default() -> Self {
        Self {
            content_slide_distance_pt: 24.0,
            content_reveal_start: 0.35,
        }
    }
}

impl PanelTokens {
    pub fn validate(&self) -> Result<(), TokenError> {
        positive("panel.content_slide_distance_pt", self.content_slide_distance_pt)?;
        // Строго меньше 1: на 1 доля проявления делилась бы на ноль.
        if self.content_reveal_start >= 1.0 {
            return Err(TokenError::OutOfRange {
                name: "panel.content_reveal_start",
                value: self.content_reveal_start,
            });
        }
        unit("panel.content_reveal_start", self.content_reveal_start)
    }
}

/// K8 «Плашка».
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BannerTokens {
    /// Высота. `[measured]` `layout.md` 2.2: плашка AE/AF LOCK ≈ 117 × 30.
    pub height_pt: f64,
    /// Минимальная ширина. `[measured]` там же: 117.
    pub min_width_pt: f64,
    /// Поле слева и справа от текста. `[estimate]` — подгонка, не Apple.
    pub horizontal_padding_pt: f64,
    /// Масштаб в начале появления (растёт к 1). `[estimate]` — подгонка, не Apple.
    pub appear_scale: f64,
}

impl Default for BannerTokens {
    fn default() -> Self {
        Self {
            height_pt: 30.0,
            min_width_pt: 117.0,
            horizontal_padding_pt: 14.0,
            appear_scale: 0.9,
        }
    }
}

impl BannerTokens {
    pub fn validate(&self) -> Result<(), TokenError> {
        positive("banner.height_pt", self.height_pt)?;
        positive("banner.min_width_pt", self.min_width_pt)?;
        positive("banner.horizontal_padding_pt", self.horizontal_padding_pt)?;
        positive("banner.appear_scale", self.appear_scale)
    }
}

/// K9 «Рамка фокуса» и её «солнце».
///
/// Числа с меткой `[measured: R00 t=…]` — с записи iOS 26 (3 px = 1 pt), решение 102.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FocusTokens {
    /// Ширина квадрата. `[measured: R00 t=99.05–109.8]`: 74,7.
    pub frame_width_pt: f64,
    /// Высота квадрата. `[measured: R00 t=99.05–109.8]`: 74,7 (квадрат).
    pub frame_height_pt: f64,
    /// Толщина линии квадрата и тиков. `[measured: R00 t=99.05–109.8]`: 1,0.
    pub stroke_width_pt: f64,
    /// Длина тика внутрь от стороны. `[measured: R00 t=99.05–109.8]`: 5,5.
    pub tick_length_pt: f64,
    /// Диаметр диска «солнца». `[measured: R00 t=99.05–109.8]`: 9.
    pub sun_disc_diameter_pt: f64,
    /// Число лучей «солнца», равномерно по кругу. `[measured: R00 t=99.05–109.8]`: 8.
    pub sun_ray_count: u32,
    /// Ширина луча. `[measured: R00 t=99.05–109.8]`: 1,0.
    pub sun_ray_width_pt: f64,
    /// Радиус начала луча от центра «солнца». `[measured: R00 t=99.05–109.8]`: 7,8.
    pub sun_ray_inner_radius_pt: f64,
    /// Радиус конца луча от центра «солнца» (весь значок ≈ 25 pt). `[measured: R00 t=99.05–109.8]`: 12,5.
    pub sun_ray_outer_radius_pt: f64,
    /// Центр «солнца» правее правой стороны квадрата. `[measured: R00 t=99.05–109.8]`: 17,8.
    pub sun_offset_right_of_frame_pt: f64,
    /// Центр «солнца» выше центра квадрата. `[measured: R00 t=99.05–109.8]`: 0.
    pub sun_offset_above_center_pt: f64,
    /// Толщина вертикальной направляющей. `[measured: R00 t=99.05–109.8]`: 1,0.
    pub guide_line_width_pt: f64,
    /// Полная длина направляющей (с разрывом у «солнца»), центр — исходная точка «солнца».
    /// `[measured: R00 t=99.05–109.8]`: 143.
    pub guide_line_length_pt: f64,
    /// Радиус разрыва направляющей вокруг «солнца». `[measured: R00 t=99.05–109.8]`: 15,5.
    pub guide_gap_radius_pt: f64,
    /// Масштаб рамки в начале появления (сжимается к 1). `[measured: R00 t=99.05–99.17]`: 1,7.
    pub appear_scale: f64,
    /// Длительность появления (масштаб и линейное проявление), секунды; без перелёта.
    /// `[measured: R00 t=99.05–99.17]`: 0,12.
    pub appear_duration_s: f64,
    /// Прозрачность «притихшей» рамки перед исчезновением. `[estimate]` — подгонка, не Apple.
    pub dimmed_opacity: f64,
    /// Через сколько секунд после тапа рамка тускнеет, если она не заблокирована. `[estimate]`:
    /// `motion.md` 4.2 — «через несколько секунд».
    pub dim_after_s: f64,
    /// Через сколько секунд после тапа рамка исчезает. `[estimate]` — `motion.md` 4.2.
    pub hide_after_s: f64,
}

impl Default for FocusTokens {
    fn default() -> Self {
        Self {
            frame_width_pt: 74.7,
            frame_height_pt: 74.7,
            stroke_width_pt: 1.0,
            tick_length_pt: 5.5,
            sun_disc_diameter_pt: 9.0,
            sun_ray_count: 8,
            sun_ray_width_pt: 1.0,
            sun_ray_inner_radius_pt: 7.8,
            sun_ray_outer_radius_pt: 12.5,
            sun_offset_right_of_frame_pt: 17.8,
            sun_offset_above_center_pt: 0.0,
            guide_line_width_pt: 1.0,
            guide_line_length_pt: 143.0,
            guide_gap_radius_pt: 15.5,
            appear_scale: 1.7,
            appear_duration_s: 0.12,
            dimmed_opacity: 0.5,
            dim_after_s: 3.0,
            hide_after_s: 5.0,
        }
    }
}

impl FocusTokens {
    pub fn validate(&self) -> Result<(), TokenError> {
        positive("focus.frame_width_pt", self.frame_width_pt)?;
        positive("focus.frame_height_pt", self.frame_height_pt)?;
        positive("focus.stroke_width_pt", self.stroke_width_pt)?;
        positive("focus.tick_length_pt", self.tick_length_pt)?;
        positive("focus.sun_disc_diameter_pt", self.sun_disc_diameter_pt)?;
        if self.sun_ray_count == 0 {
            return Err(TokenError::OutOfRange {
                name: "focus.sun_ray_count",
                value: 0.0,
            });
        }
        positive("focus.sun_ray_width_pt", self.sun_ray_width_pt)?;
        positive("focus.sun_ray_inner_radius_pt", self.sun_ray_inner_radius_pt)?;
        positive("focus.sun_ray_outer_radius_pt", self.sun_ray_outer_radius_pt)?;
        if self.sun_ray_outer_radius_pt <= self.sun_ray_inner_radius_pt {
            return Err(TokenError::OutOfRange {
                name: "focus.sun_ray_outer_radius_pt",
                value: self.sun_ray_outer_radius_pt,
            });
        }
        positive("focus.sun_offset_right_of_frame_pt", self.sun_offset_right_of_frame_pt)?;
        if !self.sun_offset_above_center_pt.is_finite() {
            return Err(TokenError::OutOfRange {
                name: "focus.sun_offset_above_center_pt",
                value: self.sun_offset_above_center_pt,
            });
        }
        positive("focus.guide_line_width_pt", self.guide_line_width_pt)?;
        positive("focus.guide_line_length_pt", self.guide_line_length_pt)?;
        positive("focus.guide_gap_radius_pt", self.guide_gap_radius_pt)?;
        positive("focus.appear_scale", self.appear_scale)?;
        positive("focus.appear_duration_s", self.appear_duration_s)?;
        unit("focus.dimmed_opacity", self.dimmed_opacity)?;
        positive("focus.dim_after_s", self.dim_after_s)?;
        positive("focus.hide_after_s", self.hide_after_s)?;
        if self.hide_after_s <= self.dim_after_s {
            return Err(TokenError::OutOfRange {
                name: "focus.hide_after_s",
                value: self.hide_after_s,
            });
        }
        Ok(())
    }
}

/// K10 «Затвор».
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ShutterTokens {
    /// Внешний диаметр (край стеклянного кольца). `[measured]` `layout.md` 2.2: Ø 80,1.
    pub outer_diameter_pt: f64,
    /// Диаметр белого диска. `[measured]` там же: Ø 67,9 (кольцо-промежуток ≈ 6,1).
    pub disc_diameter_pt: f64,
    /// Заливка кольца между диском и внешним краем: серая полупрозрачная, на тёмном ≈ #313131.
    /// `[measured: R00 t=10.027]`; выражена белым с альфой, чтобы над тёмной сценой давать этот серый.
    pub ring_fill: DisplayColor,
    /// Цвет светлого ободка по внешнему краю кольца. `[estimate]` — светлее заливки, точный цвет
    /// не измерен; `[measured: R00 t=10.027]` — только его наличие и толщина.
    pub ring_rim_color: DisplayColor,
    /// Толщина ободка по внешнему краю (внутрь от Ø 80). `[measured: R00 t=10.027]`: 1,0.
    pub ring_rim_width_pt: f64,
    /// Масштаб диска при полном нажатии (сжатие). `[estimate]` — подгонка, не Apple.
    pub pressed_disc_scale: f64,
    /// Прозрачность диска в состоянии «занят» (идёт съёмка/обработка). `[estimate]` — подгонка.
    pub busy_disc_opacity: f64,
    /// Прозрачность всего затвора в состоянии «недоступен». `[estimate]` — подгонка, не Apple.
    pub disabled_opacity: f64,
}

impl Default for ShutterTokens {
    fn default() -> Self {
        Self {
            outer_diameter_pt: 80.1,
            disc_diameter_pt: 67.9,
            ring_fill: DisplayColor::WHITE.with_alpha(0.19),
            ring_rim_color: DisplayColor::WHITE.with_alpha(0.35),
            ring_rim_width_pt: 1.0,
            pressed_disc_scale: 0.9,
            busy_disc_opacity: 0.6,
            disabled_opacity: 0.4,
        }
    }
}

impl ShutterTokens {
    pub fn validate(&self) -> Result<(), TokenError> {
        positive("shutter.outer_diameter_pt", self.outer_diameter_pt)?;
        positive("shutter.disc_diameter_pt", self.disc_diameter_pt)?;
        if self.disc_diameter_pt >= self.outer_diameter_pt {
            return Err(TokenError::OutOfRange {
                name: "shutter.disc_diameter_pt",
                value: self.disc_diameter_pt,
            });
        }
        for (name, color) in [
            ("shutter.ring_fill", self.ring_fill),
            ("shutter.ring_rim_color", self.ring_rim_color),
        ] {
            if !color.is_valid() {
                return Err(TokenError::InvalidColor { name });
            }
        }
        positive("shutter.ring_rim_width_pt", self.ring_rim_width_pt)?;
        positive("shutter.pressed_disc_scale", self.pressed_disc_scale)?;
        unit("shutter.busy_disc_opacity", self.busy_disc_opacity)?;
        unit("shutter.disabled_opacity", self.disabled_opacity)
    }
}
