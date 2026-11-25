//! «Сцена под точкой тапа уже другая»: наблюдение рендера для владельца точки (рендер → движок).
//!
//! Владелец точки замера и фокуса — движок камеры. Рендер видит статистику кадров и лишь сообщает, что кадр больше не
//! похож на тот, для которого человек ставил точку; снять её или оставить (блокировка AE/AF, снимок со вспышкой)
//! решает движок.
//!
//! Наблюдение опознаётся номером тапа: опоздавшее сообщение о прошлом тапе новую точку не снимает.

use serde::Serialize;

use crate::auto_exposure::CameraTag;
use crate::capture_result::SensorTimestamp;
use crate::three_a::PointRequestId;

/// Почему точка тапа больше не годится.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PointObsoleteCause {
    /// Большая часть кадра устойчиво не похожа на кадр, для которого ставили точку.
    SceneChanged,
    /// Регион точки ушёл за край кадра (зум): замеру по нему взяться неоткуда, экспозиция стояла бы на прежней цели.
    RegionOutsideFrame,
}

/// Наблюдение: точка тапа `request` устарела.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PointObsolete {
    /// Тап, для которого сделано наблюдение.
    pub request: PointRequestId,
    /// Камера кадра наблюдения: движок не принимает наблюдение с другой камеры.
    pub measured_on: CameraTag,
    /// Кадр, на котором наблюдение подтвердилось.
    pub frame: SensorTimestamp,
    pub cause: PointObsoleteCause,
}
