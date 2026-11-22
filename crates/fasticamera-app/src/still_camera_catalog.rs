//! Описание камер для тегов файла: `CameraCatalog` службы записи поверх каталога камер движка.
//!
//! Службе нужно, чтобы записать честный Exif и DNG: производитель и модель телефона, уникальное имя камеры
//! (различает объективы одного телефона), наше ПО и оптика (`FocalLength`, `FNumber`). Первое приходит паспортом
//! телефона из `initialize`, оптика — из характеристик камеры движка (`Optics`), роль — из каталога ролей.
//!
//! Каталог движка читается на КАЖДЫЙ запрос: на старте он пуст (читается позже, после открытия камер), а роли
//! и камеры могут смениться. Камеры нет в каталоге — `None`, и служба срывает снимок: без модели нельзя честно
//! записать DNG.

use std::sync::Arc;

use camera_contract::catalog::CameraCatalog as EngineCatalog;
use camera_contract::ids::CameraId;
use camera_core::handle::CameraHandle;
use photo_output::exif::{CameraDescription, LensDescription};
use photo_output::service::CameraCatalog;

use crate::device_config::DeviceIdentity;

/// Откуда брать текущий каталог камер (ручка движка в приложении, подделка в тестах).
pub trait EngineCatalogSource: Send + Sync {
    /// Текущий каталог; пустой, пока не прочитан.
    fn current_catalog(&self) -> Arc<EngineCatalog>;
}

/// Каталог из ручки движка камеры.
pub struct HandleCatalogSource {
    handle: CameraHandle,
}

impl HandleCatalogSource {
    pub fn new(handle: CameraHandle) -> Self {
        Self { handle }
    }
}

impl EngineCatalogSource for HandleCatalogSource {
    fn current_catalog(&self) -> Arc<EngineCatalog> {
        self.handle.status().catalog
    }
}

/// Строка `Software` файлов: «Fasticamera <версия crate>».
pub fn software_description() -> String {
    format!("Fasticamera {}", env!("CARGO_PKG_VERSION"))
}

/// Описание камер службы записи.
pub struct EngineCameraCatalog {
    source: Arc<dyn EngineCatalogSource>,
    device: DeviceIdentity,
    software: String,
}

impl EngineCameraCatalog {
    pub fn new(source: Arc<dyn EngineCatalogSource>, device: DeviceIdentity, software: String) -> Self {
        Self {
            source,
            device,
            software,
        }
    }
}

impl CameraCatalog for EngineCameraCatalog {
    fn describe_camera(&self, camera: &CameraId) -> Option<CameraDescription> {
        let catalog = self.source.current_catalog();
        let descriptor = catalog.camera(camera)?;
        let optics = &descriptor.capabilities.optics;
        // Основной объектив — первый в списках (у камер телефона по одному фокусному и одной диафрагме); тег
        // объектива пишется только когда есть и то и другое, иначе честно «объектив неизвестен».
        let lens = match (optics.focal_lengths_mm.first(), optics.apertures_f_number.first()) {
            (Some(&focal_length_mm), Some(&f_number)) => Some(LensDescription {
                focal_length_mm,
                f_number,
            }),
            _ => None,
        };
        // Уникальное имя: модель телефона + роль объектива (если камера играет роль) или её номер.
        let camera_label = catalog
            .role_of(camera)
            .map_or_else(|| format!("camera {camera}"), |role| role.name().to_owned());
        Some(CameraDescription {
            make: self.device.make().to_owned(),
            model: self.device.model().to_owned(),
            unique_camera_model: format!("{} {} {}", self.device.make(), self.device.model(), camera_label),
            software: self.software.clone(),
            lens,
        })
    }
}

#[cfg(test)]
#[path = "still_camera_catalog_tests.rs"]
mod tests;
