//! Каталог видимых камер и привязка продуктовых ролей к ID.

use serde::Serialize;

use crate::capabilities::CameraDescriptor;
use crate::ids::{CameraId, CameraTarget, LensRole};

/// Роль объектива, привязанная к конкретной камере.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoleBinding {
    pub role: LensRole,
    pub camera: CameraId,
}

/// Каталог: все камеры, которые видит приложение, и какая из них играет какую роль.
///
/// Инварианты (проверяет [`CameraCatalog::new`]): ID не повторяются, каждая роль привязана
/// не больше одного раза и только к камере из каталога. Камеры, закрытые фреймворком
/// (на OnePlus 11 — телевик 4 и логическая 5, `SYSTEM_CAMERA`), в каталог не попадают:
/// их роль честно отсутствует (инвариант 6 README).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CameraCatalog {
    cameras: Vec<CameraDescriptor>,
    roles: Vec<RoleBinding>,
}

/// Каталог собран с нарушением инварианта.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CatalogError {
    #[error("камера {0} встречается в каталоге дважды")]
    DuplicateCamera(CameraId),
    #[error("роль {} привязана дважды", .0.name())]
    DuplicateRole(LensRole),
    #[error("роль {} привязана к камере {camera}, которой нет в каталоге", role.name())]
    RoleBoundToUnknownCamera { role: LensRole, camera: CameraId },
}

/// Почему цель открытия не нашлась в каталоге.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case", tag = "kind", content = "value")]
pub enum TargetUnavailable {
    /// Такой роли на телефоне нет (например, телевик без `SYSTEM_CAMERA`).
    #[error("роли {} нет среди доступных камер", .0.name())]
    RoleNotPresent(LensRole),
    #[error("камеры {0} нет в каталоге")]
    UnknownCamera(CameraId),
}

impl CameraCatalog {
    pub fn new(cameras: Vec<CameraDescriptor>, roles: Vec<RoleBinding>) -> Result<Self, CatalogError> {
        for (index, descriptor) in cameras.iter().enumerate() {
            if cameras[..index].iter().any(|earlier| earlier.id == descriptor.id) {
                return Err(CatalogError::DuplicateCamera(descriptor.id.clone()));
            }
        }
        for (index, binding) in roles.iter().enumerate() {
            if roles[..index].iter().any(|earlier| earlier.role == binding.role) {
                return Err(CatalogError::DuplicateRole(binding.role));
            }
            if !cameras.iter().any(|descriptor| descriptor.id == binding.camera) {
                return Err(CatalogError::RoleBoundToUnknownCamera {
                    role: binding.role,
                    camera: binding.camera.clone(),
                });
            }
        }
        Ok(Self { cameras, roles })
    }

    /// Пустой каталог (камер нет или они ещё не прочитаны).
    pub fn empty() -> Self {
        Self {
            cameras: Vec::new(),
            roles: Vec::new(),
        }
    }

    pub fn cameras(&self) -> &[CameraDescriptor] {
        &self.cameras
    }

    pub fn roles(&self) -> &[RoleBinding] {
        &self.roles
    }

    pub fn camera(&self, id: &CameraId) -> Option<&CameraDescriptor> {
        self.cameras.iter().find(|descriptor| &descriptor.id == id)
    }

    /// Роль, которую играет камера, если играет.
    pub fn role_of(&self, id: &CameraId) -> Option<LensRole> {
        self.roles
            .iter()
            .find(|binding| &binding.camera == id)
            .map(|binding| binding.role)
    }

    /// Камера для цели открытия: по роли или по ID.
    pub fn resolve(&self, target: &CameraTarget) -> Result<&CameraDescriptor, TargetUnavailable> {
        match target {
            CameraTarget::Role(role) => self
                .roles
                .iter()
                .find(|binding| binding.role == *role)
                .and_then(|binding| self.camera(&binding.camera))
                .ok_or(TargetUnavailable::RoleNotPresent(*role)),
            CameraTarget::Camera(id) => self
                .camera(id)
                .ok_or_else(|| TargetUnavailable::UnknownCamera(id.clone())),
        }
    }
}

#[cfg(test)]
#[path = "catalog_tests.rs"]
mod tests;
