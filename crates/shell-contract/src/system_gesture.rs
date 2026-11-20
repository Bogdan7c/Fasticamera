//! Зоны обязательных системных жестов в нейтральном виде.
//!
//! # Зачем
//! Когда панель навигации Android скрыта (`BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE`), окно остаётся
//! интерактивным: свайп от края, которым владелец вытягивает панель, приходит в приложение как
//! обычные касания. Экран камеры принял бы его за свой жест (например, «свайп вверх по видоискателю»
//! открыл бы меню Controls одновременно с системной панелью). Android сообщает полосы по краям, где
//! системный жест **обязателен** и приложение его перехватить не может
//! (`WindowInsets.Type.mandatorySystemGestures()`): касание, начатое в такой полосе, принадлежит системе.
//!
//! # Что здесь
//! Только данные и геометрия: толщины полос от краёв surface в пикселях и проверка «точка в зоне».
//! Решение, что делать с касанием в зоне, принимает потребитель (экран камеры). Kotlin лишь пересылает
//! четыре числа; разбор и проверка — в Rust.
//!
//! # Система координат
//! Толщины отсчитываются от краёв **окна**; предполагается, что `SurfaceView` занимает окно целиком
//! (полноэкранный режим `MainActivity`), поэтому они же — толщины от краёв surface.

use serde::Serialize;

use crate::surface::SurfaceSize;
use crate::touch::SurfacePoint;

/// Край экрана.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenEdge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Толщины полос ровно так, как их сообщает платформа (`android.graphics.Insets`): знаковые целые,
/// ещё не проверенные. Именованные поля исключают путаницу порядка `left/top/right/bottom`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawEdgeInsetsPx {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

/// Платформа сообщила невозможные толщины полос.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SystemGestureZonesError {
    #[error("отрицательная толщина зоны системного жеста {thickness_px} px у края {edge:?}")]
    NegativeThickness { edge: ScreenEdge, thickness_px: i32 },
}

/// Полосы обязательных системных жестов по краям surface, пиксели. Нулевая толщина — у этого края
/// системного жеста нет. По умолчанию зон нет ([`SystemGestureZones::NONE`]): поведение «как раньше».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct SystemGestureZones {
    top_px: u32,
    bottom_px: u32,
    left_px: u32,
    right_px: u32,
}

impl SystemGestureZones {
    /// Зон нет: ни одно касание не принадлежит системе.
    pub const NONE: Self = Self {
        top_px: 0,
        bottom_px: 0,
        left_px: 0,
        right_px: 0,
    };

    /// Проверяет толщины платформы: отрицательные — ошибка (не «обрезаем до нуля молча»).
    pub fn try_from_insets(insets: RawEdgeInsetsPx) -> Result<Self, SystemGestureZonesError> {
        let thickness = |edge: ScreenEdge, thickness_px: i32| {
            u32::try_from(thickness_px).map_err(|_| SystemGestureZonesError::NegativeThickness { edge, thickness_px })
        };
        Ok(Self {
            top_px: thickness(ScreenEdge::Top, insets.top)?,
            bottom_px: thickness(ScreenEdge::Bottom, insets.bottom)?,
            left_px: thickness(ScreenEdge::Left, insets.left)?,
            right_px: thickness(ScreenEdge::Right, insets.right)?,
        })
    }

    /// Толщина полосы у края, пиксели.
    pub fn thickness_px(&self, edge: ScreenEdge) -> u32 {
        match edge {
            ScreenEdge::Top => self.top_px,
            ScreenEdge::Bottom => self.bottom_px,
            ScreenEdge::Left => self.left_px,
            ScreenEdge::Right => self.right_px,
        }
    }

    /// Нет ни одной полосы.
    pub fn is_none(&self) -> bool {
        *self == Self::NONE
    }

    /// Лежит ли точка в какой-либо полосе surface заданного размера.
    ///
    /// Полосы полуоткрытые со стороны внутренней границы: нижняя `[height - bottom, height)`, верхняя
    /// `[0, top)`. Точка с нечисловой координатой (NaN) ни в какой зоне не лежит — касание не
    /// «присваивается» системе по мусорным данным.
    pub fn contains(&self, surface: SurfaceSize, point: SurfacePoint) -> bool {
        let (x, y) = (f64::from(point.x_px), f64::from(point.y_px));
        let (width, height) = (f64::from(surface.width_px), f64::from(surface.height_px));
        y < f64::from(self.top_px)
            || y >= height - f64::from(self.bottom_px)
            || x < f64::from(self.left_px)
            || x >= width - f64::from(self.right_px)
    }
}

#[cfg(test)]
#[path = "system_gesture_tests.rs"]
mod tests;
