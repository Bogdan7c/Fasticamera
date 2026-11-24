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
//! Форма чисел и система координат — [`crate::edge_insets`].

use serde::Serialize;

pub use crate::edge_insets::{EdgeInsetsError, RawEdgeInsetsPx, ScreenEdge};

use crate::edge_insets::EdgeInsetsPx;
use crate::surface::SurfaceSize;
use crate::touch::SurfacePoint;

/// Полосы обязательных системных жестов по краям surface, пиксели. Нулевая толщина — у этого края
/// системного жеста нет. По умолчанию зон нет ([`SystemGestureZones::NONE`]): поведение «как раньше».
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(transparent)]
pub struct SystemGestureZones {
    edges: EdgeInsetsPx,
}

impl SystemGestureZones {
    /// Зон нет: ни одно касание не принадлежит системе.
    pub const NONE: Self = Self {
        edges: EdgeInsetsPx::ZERO,
    };

    /// Проверяет толщины платформы: отрицательные — ошибка (не «обрезаем до нуля молча»).
    pub fn try_from_insets(insets: RawEdgeInsetsPx) -> Result<Self, EdgeInsetsError> {
        Ok(Self {
            edges: EdgeInsetsPx::try_from_raw(insets)?,
        })
    }

    /// Толщина полосы у края, пиксели.
    pub fn thickness_px(&self, edge: ScreenEdge) -> u32 {
        self.edges.thickness_px(edge)
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
        y < f64::from(self.thickness_px(ScreenEdge::Top))
            || y >= height - f64::from(self.thickness_px(ScreenEdge::Bottom))
            || x < f64::from(self.thickness_px(ScreenEdge::Left))
            || x >= width - f64::from(self.thickness_px(ScreenEdge::Right))
    }
}

#[cfg(test)]
#[path = "system_gesture_tests.rs"]
mod tests;
