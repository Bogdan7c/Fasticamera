//! Ожидание каталога камер перед сборкой входов стенда: корень «запасного сенсора» на телефоне.
//!
//! # Причина (найдена по коду)
//! Движок камеры читает каталог камер в СВОЁМ потоке сразу после запуска (`CameraEngine::load_catalog`), а до
//! первой публикации снимок статуса несёт ПУСТОЙ каталог (`EngineOutput::new` → `CameraCatalog::empty()`).
//! Сценарий стартует из намерения запуска приложения почти сразу и брал снимок статуса в `start()`: если поток
//! движка ещё не опубликовал каталог, роль `wide` «не находилась», и стенд брал запасную калибровку, хотя камера
//! у телефона есть. Закрытие камеры тут ни при чём: каталог движка после `close` остаётся.
//!
//! # Что делаем
//! Сценарий не берёт входы, пока каталог пуст: он ждёт публикации (опрос на тиках, срок — из конфига
//! `catalog_wait_ms`). Вышел срок (каталог не читается: движок в фатале, камер нет) — идём дальше с тем, что есть;
//! запасной набор тогда объясняет причину ([`describe_catalog`]: сколько камер и какие роли), а не молчит.

use std::time::{Duration, Instant};

use camera_contract::catalog::CameraCatalog;
use camera_core::status::CameraStatus;

/// Каталог уже опубликован движком (в нём есть хотя бы одна камера).
pub fn catalog_published(status: &CameraStatus) -> bool {
    !status.catalog.cameras().is_empty()
}

/// Что показал очередной опрос статуса движка.
#[derive(Debug, Clone, PartialEq)]
pub enum CatalogPoll {
    /// Каталог есть: можно собирать входы по этому снимку.
    Ready(CameraStatus),
    /// Каталога ещё нет, срок не вышел: ждём следующего тика.
    Waiting,
    /// Срок вышел, каталога нет: собираем входы с тем, что есть (сенсор будет запасным с объяснением причины).
    TimedOut(CameraStatus),
}

/// Ожидание каталога с предельным сроком.
#[derive(Debug, Clone, Copy)]
pub struct CatalogWait {
    deadline: Instant,
}

impl CatalogWait {
    /// Ожидание, которое началось `now` и длится не дольше `limit`.
    pub fn starting_at(now: Instant, limit: Duration) -> Self {
        Self { deadline: now + limit }
    }

    /// Один опрос: снимок статуса движка и текущий момент.
    pub fn poll(&self, status: CameraStatus, now: Instant) -> CatalogPoll {
        if catalog_published(&status) {
            CatalogPoll::Ready(status)
        } else if now >= self.deadline {
            CatalogPoll::TimedOut(status)
        } else {
            CatalogPoll::Waiting
        }
    }
}

/// Кратко о каталоге для объяснения запасного сенсора: сколько камер и какие роли к каким камерам привязаны.
pub fn describe_catalog(catalog: &CameraCatalog) -> String {
    if catalog.cameras().is_empty() {
        return "каталог камер пуст: движок ещё не прочитал его или чтение не удалось".to_owned();
    }
    let roles: Vec<String> = catalog
        .roles()
        .iter()
        .map(|binding| format!("{}={}", binding.role.name(), binding.camera))
        .collect();
    let cameras: Vec<String> = catalog
        .cameras()
        .iter()
        .map(|descriptor| descriptor.id.to_string())
        .collect();
    format!(
        "в каталоге {} камер ({}), роли: {}",
        cameras.len(),
        cameras.join(", "),
        if roles.is_empty() {
            "нет".to_owned()
        } else {
            roles.join(", ")
        }
    )
}

#[cfg(test)]
#[path = "catalog_wait_tests.rs"]
mod tests;
