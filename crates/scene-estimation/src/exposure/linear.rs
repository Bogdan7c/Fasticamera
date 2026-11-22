//! Линейный замер кадра для расчёта вспышки (сессия 14d, решение 158).
//!
//! Обычный замер ([`super::measurement`]) отвечает на вопрос «сколько света нужно ВИДОИСКАТЕЛЮ»: он
//! сглажен по пересвету, защищает света и ограничен шагом. Вспышке нужно другое: ЛИНЕЙНАЯ величина
//! одного кадра, чтобы свет сцены и вклад фонарика можно было складывать.
//!
//! # Формула
//! Среднее линейной яркости области `Y` (см. `ExposureMeasurement::mean_luminance`) растёт прямо
//! пропорционально произведению «выдержка × ISO» кадра `P`. Значит произведение, при котором среднее
//! встало бы ровно на цель `T`, равно
//! ```text
//! P_mid_gray = P · T / Y,   T = scene_middle_gray · 2^key_offset_ev
//! ```
//! Здесь НЕТ защиты светов, поправки EV пользователя и ограничения шага — только арифметика над
//! линейными величинами замера. Для непересвеченной сцены без защиты светов это совпадает с
//! `required_product` обычного замера (проверяется тестом).

use camera_contract::auto_exposure::ExposureProduct;

use super::measurement::{ExposureMeasurement, ExposureTarget};

/// Произведение, при котором линейное среднее кадра легло бы на цель (см. модуль). `None` — число вышло
/// за конечные положительные.
pub fn linear_mid_gray_product(
    measurement: &ExposureMeasurement,
    frame_product: ExposureProduct,
    target: ExposureTarget,
    key_offset_ev: f32,
) -> Option<ExposureProduct> {
    let target_luminance = f64::from(target.scene_middle_gray()) * f64::from(key_offset_ev).exp2();
    let mean = f64::from(measurement.mean_luminance);
    ExposureProduct::from_seconds_iso(frame_product.seconds_iso() * target_luminance / mean).ok()
}

#[cfg(test)]
#[path = "linear_tests.rs"]
mod tests;
