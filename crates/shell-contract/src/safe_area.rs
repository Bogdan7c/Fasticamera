//! Безопасная зона: где на экране можно ставить то, что нажимают и читают.
//!
//! # Зачем
//! Приложение рисует под вырезом камеры и под скрытыми системными панелями (полный экран), но управление там
//! ставить нельзя: вырез его закрывает, а касание, начатое в полосе обязательного системного жеста, уходит
//! системе (решение 95). Раздел калибровки стоял с нулевого пикселя, и палец до переключателя основы не доходил
//! (сессия 14k, решение 226).
//!
//! # Что здесь
//! Только данные: отступы от четырёх краёв surface, ОДНО объединение всего, что мешает (вырез, системные панели,
//! полосы обязательных жестов). Объединяет платформа: она знает все виды отступов. Что с отступами делать, решает
//! раскладка экрана. Отличие от [`crate::system_gesture::SystemGestureZones`]: те — фильтр касаний (чей палец), эти —
//! геометрия (где ставить).

use serde::Serialize;

use crate::edge_insets::{EdgeInsetsError, EdgeInsetsPx, RawEdgeInsetsPx, ScreenEdge};

/// Отступы безопасной зоны от краёв surface, пиксели. По умолчанию отступов нет ([`SafeAreaInsets::NONE`]):
/// безопасен весь экран.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(transparent)]
pub struct SafeAreaInsets {
    edges: EdgeInsetsPx,
}

impl SafeAreaInsets {
    /// Отступов нет: безопасен весь экран.
    pub const NONE: Self = Self {
        edges: EdgeInsetsPx::ZERO,
    };

    /// Проверяет отступы платформы: отрицательные — ошибка.
    pub fn try_from_insets(insets: RawEdgeInsetsPx) -> Result<Self, EdgeInsetsError> {
        Ok(Self {
            edges: EdgeInsetsPx::try_from_raw(insets)?,
        })
    }

    /// Отступ от края, пиксели.
    pub fn thickness_px(&self, edge: ScreenEdge) -> u32 {
        self.edges.thickness_px(edge)
    }
}

#[cfg(test)]
#[path = "safe_area_tests.rs"]
mod tests;
