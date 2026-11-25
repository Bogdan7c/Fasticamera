//! Правило удержания точки тапа (решение 241): когда точка замера и фокуса отпускается сама.
//!
//! Владелец точки — движок камеры. Раньше точка жила до смены камеры или перезапуска, и замер с фокусом
//! тянулись за местом, где человека уже не интересует ничего. Теперь по умолчанию точка снимается, когда
//! сцена под ней сменилась (наблюдение рендера `PointObsolete`) и когда приложение вернулось из фона.

use camera_contract::scene_change::PointObsoleteCause;
use serde::{Deserialize, Serialize};

/// Когда движок отпускает незаблокированную точку тапа.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PointHold {
    /// Точка держится, пока сцена под ней та же: смена сцены, уход региона за край кадра при зуме и возврат
    /// из фона её снимают (замер и фокус снова по всему кадру). Блокировка AE/AF точку держит всегда.
    #[default]
    UntilSceneChange,
    /// Прежнее поведение: точка живёт до нового тапа, сброса, смены камеры или режима. Нужна для сравнения
    /// на телефоне и сценариев, которым точка не должна пропадать посреди шагов.
    UntilReplaced,
}

/// Разбор поля `point_hold` с именем поля в тексте ошибки: голый ответ serde («unknown variant …») не говорит,
/// в каком поле опечатка (образец — `resume_rule` у белой).
pub(crate) fn deserialize_point_hold_field<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<PointHold, D::Error> {
    PointHold::deserialize(deserializer)
        .map_err(|error| serde::de::Error::custom(format!("three_a.point_hold: {error}")))
}

/// Почему движок отпустил точку тапа (событие `PointOfInterestReleased`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PointReleaseCause {
    /// Рендер увидел, что сцена под точкой сменилась.
    SceneChanged,
    /// Регион точки ушёл за край кадра (зум без смены объектива).
    RegionOutsideFrame,
    /// Приложение вернулось из фона: тап до ухода в фон уже не относится к тому, что видно сейчас.
    ReturnedToApp,
}

impl From<PointObsoleteCause> for PointReleaseCause {
    fn from(cause: PointObsoleteCause) -> Self {
        match cause {
            PointObsoleteCause::SceneChanged => Self::SceneChanged,
            PointObsoleteCause::RegionOutsideFrame => Self::RegionOutsideFrame,
        }
    }
}
