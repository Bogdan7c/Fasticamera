//! Стадии вида над изображениями: тон, цвет вида и LUT (поверх попиксельных формул [`super::LookParams`]).
//!
//! Тип пространства изображения не меняется: вид работает в том же `W`, в котором основа отдала свет экрана
//! (`WorkingDisplay` у AgX, `DisplayLinear` у прежней основы).

use super::LookParams;
use crate::image::RgbImage;

/// Применяет тон, цвет вида и таблицу ко всему изображению. Нейтральный вид возвращает копию без вычислений.
pub fn apply_look<S: Copy>(image: &RgbImage<S>, look: &LookParams) -> RgbImage<S> {
    if look.has_no_display_stages() {
        return image.clone();
    }
    image.map_into(image.space(), |pixel| look.apply_to_pixel(pixel))
}
