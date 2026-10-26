//! Кто есть кто среди камер: ID, роль объектива, направление и устройство камеры.

use serde::Serialize;

/// ID камеры в терминах Camera2 (строка вроде `"2"`).
///
/// Не число: Camera2 и NDK отдают ID строками, а внешние камеры могут иметь любые имена.
/// Пустой ID бессмыслен, поэтому конструктор его отвергает.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CameraId(String);

/// ID камеры не может быть пустым.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("ID камеры не может быть пустым")]
pub struct EmptyCameraId;

impl CameraId {
    pub fn new(id: impl Into<String>) -> Result<Self, EmptyCameraId> {
        let id = id.into();
        if id.is_empty() {
            return Err(EmptyCameraId);
        }
        Ok(Self(id))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for CameraId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Куда смотрит камера (`ACAMERA_LENS_FACING`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LensFacing {
    Back,
    Front,
    External,
}

/// Роль объектива в интерфейсе камеры (как кнопки зума в iOS: 0.5×, 1×, 2×, фронт).
///
/// Роль — понятие продукта, а не Camera2: её вычисляет `camera-core` по углу обзора
/// и направлению, а привязку «роль → ID» хранит [`crate::catalog::CameraCatalog`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LensRole {
    UltraWide,
    Wide,
    Tele,
    Front,
}

impl LensRole {
    /// Все роли в порядке зума (для отчётов и перебора в тестах).
    pub const ALL: [LensRole; 4] = [LensRole::UltraWide, LensRole::Wide, LensRole::Tele, LensRole::Front];

    /// Имя для логов и отчётов (совпадает с serde).
    pub fn name(self) -> &'static str {
        match self {
            LensRole::UltraWide => "ultra_wide",
            LensRole::Wide => "wide",
            LensRole::Tele => "tele",
            LensRole::Front => "front",
        }
    }
}

/// Устройство камеры: один сенсор или логическая камера из нескольких
/// (`ACAMERA_REQUEST_AVAILABLE_CAPABILITIES_LOGICAL_MULTI_CAMERA`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CameraTopology {
    Physical,
    /// Логическая камера; `physical_ids` — сенсоры за ней. Среди них могут быть и скрытые
    /// от приложения камеры (на OnePlus 11 логическая 5 включает закрытый телевик 4).
    Logical {
        physical_ids: Vec<CameraId>,
    },
}

/// Что именно открыть: камеру продуктовой роли или конкретный ID.
///
/// Продукт открывает по роли. ID нужен диагностике (smoke открывает каждую видимую камеру,
/// включая логическую 0, у которой роли нет).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum CameraTarget {
    Role(LensRole),
    Camera(CameraId),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_camera_id_is_rejected_and_non_empty_is_kept_verbatim() {
        assert_eq!(CameraId::new(""), Err(EmptyCameraId));
        let id = CameraId::new("2").expect("непустой ID");
        assert_eq!(id.as_str(), "2");
        assert_eq!(id.to_string(), "2");
    }

    #[test]
    fn role_name_matches_serde() {
        for role in LensRole::ALL {
            assert_eq!(
                serde_json::to_value(role).expect("json"),
                serde_json::json!(role.name())
            );
        }
    }
}
