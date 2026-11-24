//! Толщины полос по краям окна: общий словарь для всех отступов, которые сообщает платформа.
//!
//! Android отдаёт отступы разных видов (полосы системных жестов, вырез камеры, системные панели) одной формой —
//! четыре числа от краёв окна (`android.graphics.Insets`). Здесь эта форма и её единственная проверка; смысл
//! чисел («касание принадлежит системе», «здесь нельзя ставить управление») задают типы-потребители
//! ([`crate::system_gesture::SystemGestureZones`], [`crate::safe_area::SafeAreaInsets`]).
//!
//! # Система координат
//! Толщины отсчитываются от краёв **окна**; `SurfaceView` занимает окно целиком (полноэкранный режим
//! `MainActivity`), поэтому они же — толщины от краёв surface, в её пикселях.

use serde::Serialize;

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
pub enum EdgeInsetsError {
    #[error("отрицательная толщина полосы {thickness_px} px у края {edge:?}")]
    NegativeThickness { edge: ScreenEdge, thickness_px: i32 },
}

/// Проверенные толщины полос по краям surface, пиксели. Ноль — у этого края полосы нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct EdgeInsetsPx {
    top_px: u32,
    bottom_px: u32,
    left_px: u32,
    right_px: u32,
}

impl EdgeInsetsPx {
    /// Полос нет ни у одного края.
    pub const ZERO: Self = Self {
        top_px: 0,
        bottom_px: 0,
        left_px: 0,
        right_px: 0,
    };

    /// Проверяет толщины платформы: отрицательные — ошибка (не «обрезаем до нуля молча»).
    pub fn try_from_raw(insets: RawEdgeInsetsPx) -> Result<Self, EdgeInsetsError> {
        let thickness = |edge: ScreenEdge, thickness_px: i32| {
            u32::try_from(thickness_px).map_err(|_| EdgeInsetsError::NegativeThickness { edge, thickness_px })
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
}

#[cfg(test)]
#[path = "edge_insets_tests.rs"]
mod tests;
