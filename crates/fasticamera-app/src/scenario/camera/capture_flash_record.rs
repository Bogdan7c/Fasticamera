//! Запись о вспышке в снимке сценария `capture` (14d): какой маршрут выбрал сервис снимка и что сообщила камера.
//!
//! Чистые функции над журналом маршрутов сервиса и журналом итогов камеры: никакого ввода-вывода. Номер снимка
//! службы совпадает с номером снимка со вспышкой камеры (`flash_camera_port::flash_shot_of`).

use camera_core::flash_status::FlashStillReport;
use serde::Serialize;
use still_contract::plan::StillId;

use super::capture_observation::PhaseBook;
use crate::flash_camera_port::flash_shot_of;

/// Что камера сообщила об итоге снимка со вспышкой.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CameraFlashRecord {
    /// Кадр со вспышкой снят: выдержка, ISO, доля света вспышки по плану движка и срок от команды до результата.
    Fired {
        exposure_ms: f64,
        iso: u32,
        flash_share: f64,
        elapsed_ms: u64,
    },
    /// Камера не сняла кадр со вспышкой.
    Failed { failure: String },
}

impl CameraFlashRecord {
    fn of(report: &FlashStillReport) -> Self {
        match report {
            FlashStillReport::Fired {
                plan,
                exposure_time,
                iso,
                elapsed,
                ..
            } => Self::Fired {
                exposure_ms: exposure_time.duration().as_secs_f64() * 1_000.0,
                iso: iso.value(),
                flash_share: plan.flash_share.value(),
                elapsed_ms: u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX),
            },
            FlashStillReport::Failed { failure, .. } => Self::Failed {
                failure: format!("{failure:?}"),
            },
        }
    }
}

/// Вспышка снимка: решение сервиса и итог камеры. `None` в полях — журнал о снимке не знает (вытеснен/не было).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FlashShotRecord {
    /// Выбрал ли сервис маршрут со вспышкой.
    pub route_uses_flash: Option<bool>,
    /// Причина маршрута словами сервиса.
    pub route_why: Option<&'static str>,
    pub camera: Option<CameraFlashRecord>,
}

impl FlashShotRecord {
    /// Запись по номеру снимка службы; `None`, если ни маршрута, ни итога камеры нет.
    pub fn of(still: Option<StillId>, book: &PhaseBook) -> Option<Self> {
        let still = still?;
        let route = book.flash_route_of(still);
        let camera = book.flash_report_of(flash_shot_of(still)).map(CameraFlashRecord::of);
        if route.is_none() && camera.is_none() {
            return None;
        }
        Some(Self {
            route_uses_flash: route.map(|entry| entry.uses_flash),
            route_why: route.map(|entry| entry.why),
            camera,
        })
    }

    /// Сработала ли вспышка по итогу камеры; `None` — итога нет (проверки тестов; в отчёт идёт сам итог камеры).
    #[cfg(test)]
    pub fn fired(&self) -> Option<bool> {
        self.camera
            .as_ref()
            .map(|camera| matches!(camera, CameraFlashRecord::Fired { .. }))
    }
}
