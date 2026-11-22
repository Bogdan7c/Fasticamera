//! Расчёт кадра со вспышкой: чистая функция без состояния (14d, решение 158).
//!
//! Единственный источник формулы: её используют и движок при выборе выдержки и ISO, и (через
//! `FlashFramePlan`) рендер при проявлении. Другой копии формулы быть не должно.
//!
//! Физика. Сенсор линейный: яркость кадра ∝ свету сцены × выдержка × ISO. Назовём «светом» L
//! обратную величину произведения «выдержка × ISO», при котором средний серый встаёт в цель:
//! `L = 1 / P`. Свет складывается:
//! * кадр без фонарика даёт `L_amb = 1 / P_amb`;
//! * кадр с фонариком — `L_torch = 1 / P_torch`, то есть вклад фонарика равен `L_torch − L_amb`;
//! * разовая вспышка ярче фонарика в `R` раз при равных выдержке и ISO (замер зонда: 1,87), а
//!   по факту зонда вспышка действует как постоянный свет на весь кадр, и вклад ∝ выдержке —
//!   поэтому его можно складывать с светом сцены в тех же единицах «на секунду·ISO».
//!
//! Итог: `L_flash = L_amb + R · max(0, L_torch − L_amb)`, `P_flash = 1 / L_flash`. Если замер с
//! фонариком не ярче замера без него (шум, дальний объект), вклад фонарика считается нулевым.

use std::time::Duration;

use camera_contract::auto_exposure::ExposureProduct;
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::control::{ExposureTime, Iso};
use camera_contract::flash::{FlashFramePlan, FlashShare, FlashShotId};

use crate::three_a_own::{Anchor, SplitLimits, split};

/// Всё, что нужно, чтобы выбрать кадр со вспышкой.
#[derive(Debug, Clone, Copy)]
pub(super) struct FlashPlanInputs {
    pub(super) shot: FlashShotId,
    /// Кадр без светодиода, из замера которого взят `ambient` (уйдёт в план для рендера).
    pub(super) ambient_frame: SensorTimestamp,
    /// Среднее серое сцены без вспышки: замер кадра без фонарика.
    pub(super) ambient: ExposureProduct,
    /// Среднее серое сцены с фонариком: замер кадра с фонариком.
    pub(super) torch: ExposureProduct,
    /// Во сколько раз разовая вспышка ярче фонарика (`three_a.flash.torch_to_flash_ratio`).
    pub(super) torch_to_flash_ratio: f64,
    /// Компенсация экспозиции пользователя, EV: применяется к итоговому произведению.
    pub(super) bias_ev: f64,
    /// Пределы камеры и потока: тот же делёж, что у нашего AE.
    pub(super) limits: SplitLimits,
}

/// Выбранный кадр со вспышкой: план для рендера и значения запроса.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::engine) struct PlannedShot {
    pub(super) plan: FlashFramePlan,
    pub(super) exposure_time: ExposureTime,
    pub(super) iso: Iso,
}

/// Свет сцены под вспышкой и доля вспышки в нём: `(P_flash, flash_share)`.
/// `None`, если вышло не конечное положительное число.
pub(super) fn combine_light(
    ambient: ExposureProduct,
    torch: ExposureProduct,
    torch_to_flash_ratio: f64,
) -> Option<(ExposureProduct, f64)> {
    let ambient_light = 1.0 / ambient.seconds_iso();
    let torch_light = 1.0 / torch.seconds_iso();
    // Вклад фонарика не бывает отрицательным: темнее фона замер с фонариком — это шум.
    let torch_contribution = (torch_light - ambient_light).max(0.0);
    let flash_light = torch_to_flash_ratio * torch_contribution;
    let total_light = ambient_light + flash_light;
    let product = ExposureProduct::from_seconds_iso(1.0 / total_light).ok()?;
    Some((product, flash_light / total_light))
}

/// Выбрать кадр со вспышкой; `None` — числа вышли негодными (нулевые границы камеры и т. п.).
pub(super) fn plan_flash_frame(inputs: &FlashPlanInputs) -> Option<PlannedShot> {
    let (scene_product, flash_share) = combine_light(inputs.ambient, inputs.torch, inputs.torch_to_flash_ratio)?;
    // Компенсация пользователя двигает цель яркости, а не долю вспышки в свете.
    let requested = scene_product.scaled_by_ev(inputs.bias_ev)?;
    // Свободный делёж без защиты от мерцания: вспышка короткая, а лампы мерцают независимо от неё.
    let divided = split(requested.seconds_iso(), Anchor::Free, inputs.limits, None::<Duration>);
    let exposure_time = ExposureTime::new(divided.shutter).ok()?;
    let iso = Iso::new(divided.iso).ok()?;
    Some(PlannedShot {
        plan: FlashFramePlan {
            shot: inputs.shot,
            ambient_frame: inputs.ambient_frame,
            mid_gray_product: requested,
            flash_share: FlashShare::new(flash_share),
        },
        exposure_time,
        iso,
    })
}

#[cfg(test)]
#[path = "plan_tests.rs"]
mod tests;
