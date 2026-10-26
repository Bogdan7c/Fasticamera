//! Назначение продуктовых ролей (`UltraWide` / `Wide` / `Tele` / `Front`) камерам каталога.
//!
//! Чистая функция без ввода-вывода: на входе описания камер от бэкенда и пороги из конфига,
//! на выходе привязки «роль → камера» и диагностика (кому роль не досталась и почему).
//! Ничего не угадывается молча: камера без роли всегда объяснена записью в диагностике
//! или правилом (логическая, системная, внешняя).

use camera_contract::capabilities::{CameraDescriptor, RequestCapability};
use camera_contract::catalog::{CameraCatalog, CatalogError, RoleBinding};
use camera_contract::ids::{CameraId, CameraTopology, LensFacing, LensRole};
use serde::Serialize;

use crate::config::RoleClassifierConfig;

/// Почему камера осталась без роли там, где роль «должна была» быть.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RoleDiagnostic {
    /// Задняя физическая камера, но угол обзора вычислить нельзя (нет фокусного расстояния
    /// или размера сенсора в метаданных).
    FieldOfViewUnknown { camera: CameraId },
    /// Две камеры претендовали на одну роль; выиграла `winner`, `loser` осталась без роли.
    RoleConflict {
        role: LensRole,
        winner: CameraId,
        loser: CameraId,
    },
}

/// Итог назначения ролей.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RoleAssignment {
    /// Привязки в порядке [`LensRole::ALL`] (детерминированно).
    pub bindings: Vec<RoleBinding>,
    pub diagnostics: Vec<RoleDiagnostic>,
}

/// Каталог с ролями и диагностикой назначения.
#[derive(Debug, Clone, PartialEq)]
pub struct ClassifiedCatalog {
    pub catalog: CameraCatalog,
    pub diagnostics: Vec<RoleDiagnostic>,
}

/// Кандидат на роль: камера и вес для разрешения конфликтов.
struct Candidate<'a> {
    role: LensRole,
    camera: &'a CameraDescriptor,
}

/// Назначает роли камерам.
///
/// Правила:
/// * логические камеры и камеры `SYSTEM_CAMERA` ролей не получают;
/// * `Front` → роль `Front`, `External` → без роли;
/// * задняя физическая: угол `>= ultra_wide_min` → `UltraWide`, `<= tele_max` → `Tele`,
///   иначе `Wide`; угла нет → без роли + диагностика;
/// * конфликт за роль выигрывает камера с бо́льшим числом пикселей сенсора, при равенстве —
///   меньший ID по строке; проигравший получает диагностику.
pub fn assign_roles(cameras: &[CameraDescriptor], config: &RoleClassifierConfig) -> RoleAssignment {
    let mut diagnostics = Vec::new();
    let candidates: Vec<Candidate<'_>> = cameras
        .iter()
        .filter_map(|camera| candidate_for(camera, config, &mut diagnostics))
        .collect();

    let mut bindings = Vec::new();
    for role in LensRole::ALL {
        let mut contenders: Vec<&Candidate<'_>> = candidates.iter().filter(|c| c.role == role).collect();
        // Лучший первым: больше пикселей, затем меньший ID.
        contenders.sort_by(|left, right| {
            right
                .camera
                .capabilities
                .pixel_array
                .pixel_count()
                .cmp(&left.camera.capabilities.pixel_array.pixel_count())
                .then_with(|| left.camera.id.as_str().cmp(right.camera.id.as_str()))
        });
        let Some((winner, losers)) = contenders.split_first() else {
            continue;
        };
        bindings.push(RoleBinding {
            role,
            camera: winner.camera.id.clone(),
        });
        for loser in losers {
            diagnostics.push(RoleDiagnostic::RoleConflict {
                role,
                winner: winner.camera.id.clone(),
                loser: loser.camera.id.clone(),
            });
        }
    }
    RoleAssignment { bindings, diagnostics }
}

/// Собирает каталог камер с назначенными ролями.
///
/// Ошибка конструктора каталога (повторный ID камеры от бэкенда) возвращается вариантом:
/// по построению ролей она невозможна, но дубликаты в самом списке камер бэкенд мог прислать.
pub fn classify_catalog(
    cameras: Vec<CameraDescriptor>,
    config: &RoleClassifierConfig,
) -> Result<ClassifiedCatalog, CatalogError> {
    let RoleAssignment { bindings, diagnostics } = assign_roles(&cameras, config);
    let catalog = CameraCatalog::new(cameras, bindings)?;
    Ok(ClassifiedCatalog { catalog, diagnostics })
}

/// Роль-кандидат для камеры либо `None`, если роль ей не положена.
fn candidate_for<'a>(
    camera: &'a CameraDescriptor,
    config: &RoleClassifierConfig,
    diagnostics: &mut Vec<RoleDiagnostic>,
) -> Option<Candidate<'a>> {
    let capabilities = &camera.capabilities;
    if capabilities.topology != CameraTopology::Physical {
        return None;
    }
    if capabilities.has_capability(RequestCapability::SystemCamera) {
        return None;
    }
    match capabilities.facing {
        LensFacing::External => None,
        LensFacing::Front => Some(Candidate {
            role: LensRole::Front,
            camera,
        }),
        LensFacing::Back => match capabilities.optics.widest_field_of_view_degrees() {
            Some(degrees) => Some(Candidate {
                role: back_role_for_angle(degrees, config),
                camera,
            }),
            None => {
                diagnostics.push(RoleDiagnostic::FieldOfViewUnknown {
                    camera: camera.id.clone(),
                });
                None
            }
        },
    }
}

/// Роль задней камеры по углу обзора и порогам из конфига.
fn back_role_for_angle(degrees: f64, config: &RoleClassifierConfig) -> LensRole {
    if degrees >= config.ultra_wide_min_fov_degrees {
        LensRole::UltraWide
    } else if degrees <= config.tele_max_fov_degrees {
        LensRole::Tele
    } else {
        LensRole::Wide
    }
}

#[cfg(test)]
#[path = "role_classifier_tests.rs"]
mod tests;
