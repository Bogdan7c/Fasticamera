//! План управляющей части запроса HAL — граница движка и бэкенда (сессия 08).
//!
//! Движок по намерениям (ручные параметры, точка интереса, блокировка, профиль) чистой
//! функцией строит [`RequestControls`], а бэкенд переводит его в теги Camera2. Бэкенд ничего
//! не решает сам: какой режим AF, вендорский или ручной AE, какие регионы — всё здесь.
//! Рычаг частоты кадров и вендорские ключи потока к плану не относятся: бэкенд обязан ставить
//! их в **каждый** запрос (повторяющийся, разовый, серию) — иначе разовый запрос может сменить
//! режим сенсора, а вендорские ключи CamX «липкие» (зонд 08).

use camera_contract::control::{ExposureTime, FocusDistance, Iso};
use camera_contract::three_a::{AeAfLock, MeteringRegion};
use serde::Serialize;

/// Экспозиция в запросе.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum ExposurePlan {
    /// **Временный** вендорский AE (решение 66, до 08a): `AE_MODE_ON` с компенсацией в шагах
    /// HAL, блокировкой и регионом замера. В 08a план больше не строится.
    VendorAuto {
        compensation_steps: i32,
        lock: AeAfLock,
        region: Option<MeteringRegion>,
    },
    /// Сенсор вручную (`AE_MODE_OFF`): выдержка и ISO заданы — пользователем или нашим AE.
    /// Длительность кадра бэкенд берёт из потока (не короче выдержки).
    Sensor { exposure_time: ExposureTime, iso: Iso },
}

/// Фокус в запросе.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FocusPlan {
    /// У камеры фиксированный фокус — ключи AF в запрос не ставятся.
    FixedLens,
    /// Непрерывный AF (`CONTINUOUS_PICTURE` для видоискателя, `CONTINUOUS_VIDEO` для записи —
    /// по намерению потока), регион — если задан.
    Continuous { region: Option<MeteringRegion> },
    /// Однократный AF (`AF_MODE_AUTO`): объектив двигается только по триггеру и потом
    /// держится. Так работают тап и блокировка AE/AF.
    SingleShot { region: MeteringRegion },
    /// Ручная дистанция (`AF_MODE_OFF` + `LENS_FOCUS_DISTANCE`).
    Manual { distance: FocusDistance },
}

/// Управляющая часть одного запроса.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct RequestControls {
    pub exposure: ExposurePlan,
    pub focus: FocusPlan,
}

/// Разовое действие автофокуса (`ACAMERA_CONTROL_AF_TRIGGER`): ставится ровно в один запрос.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AfTrigger {
    /// Начать скан (тап).
    Start,
    /// Отменить скан/блокировку и вернуть AF в начальное состояние.
    Cancel,
}
