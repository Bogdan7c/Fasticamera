//! Подделка камеры для тестов вспышки (14d): совет для Auto и приём команд снимка со вспышкой без настоящего движка.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "в тестах падение — способ сообщить о провале"
)]

use std::sync::{Mutex, MutexGuard, PoisonError};

use camera_contract::capture_result::SensorTimestamp;
use camera_contract::flash::{FlashShotId, FlashUnit};
use camera_core::CommandRejected;
use camera_core::flash_status::{FlashAdvice, FlashStatus, FlashStillObserver, FlashStillReport};
use std::sync::Arc;
use std::time::Duration;

use crate::flash_camera_port::FlashCameraPort;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Камера-подделка: совет и отказ задаёт тест, принятые команды запоминаются.
pub struct FakeFlashCamera {
    advice: Mutex<FlashAdvice>,
    rejection: Mutex<Option<CommandRejected>>,
    shots: Mutex<Vec<FlashShotId>>,
    status_reads: Mutex<u32>,
    /// Наблюдатель, которому камера сразу сообщает `Fired` на каждую принятую команду (как движок после снимка).
    reports_to: Mutex<Option<Arc<dyn FlashStillObserver>>>,
}

impl FakeFlashCamera {
    /// Камера со вспышкой, советом «не нужна» и без отказов.
    pub fn new() -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self {
            advice: Mutex::new(FlashAdvice::NotNeeded),
            rejection: Mutex::new(None),
            shots: Mutex::default(),
            status_reads: Mutex::new(0),
            reports_to: Mutex::new(None),
        })
    }

    /// Камера, которая на каждую принятую команду сообщает наблюдателю итог `Fired` (путь «кадр снят»).
    pub fn reporting_fired_to(observer: Arc<dyn FlashStillObserver>) -> Arc<Self> {
        let camera = Self::new();
        *lock(&camera.reports_to) = Some(observer);
        camera
    }

    pub fn set_advice(&self, advice: FlashAdvice) {
        *lock(&self.advice) = advice;
    }

    /// Следующие команды снимка получат отказ доставки.
    pub fn reject_commands(&self, rejection: CommandRejected) {
        *lock(&self.rejection) = Some(rejection);
    }

    /// Номера снимков со вспышкой, принятые камерой.
    pub fn shots(&self) -> Vec<FlashShotId> {
        lock(&self.shots).clone()
    }

    /// Сколько раз сервис читал состояние камеры (Off/On его не читают).
    pub fn status_reads(&self) -> u32 {
        *lock(&self.status_reads)
    }
}

impl FlashCameraPort for FakeFlashCamera {
    fn flash_status(&self) -> FlashStatus {
        *lock(&self.status_reads) += 1;
        FlashStatus {
            unit: FlashUnit::Present,
            advice: *lock(&self.advice),
            ..FlashStatus::default()
        }
    }

    fn capture_flash_still(&self, shot: FlashShotId) -> Result<(), CommandRejected> {
        if let Some(rejection) = *lock(&self.rejection) {
            return Err(rejection);
        }
        lock(&self.shots).push(shot);
        if let Some(observer) = lock(&self.reports_to).clone() {
            observer.flash_still_finished(&fired_report(shot));
        }
        Ok(())
    }
}

/// Итог камеры «вспышка сработала»: выдержка 12 мс, ISO 400, доля света вспышки 0,6.
pub fn fired_report(shot: FlashShotId) -> FlashStillReport {
    use camera_contract::auto_exposure::ExposureProduct;
    use camera_contract::control::{ExposureTime, Iso};
    use camera_contract::flash::{FlashFramePlan, FlashModeEcho, FlashResult, FlashShare, FlashState};
    FlashStillReport::Fired {
        shot,
        frame: camera_core::flash_status::FlashFrameResult {
            frame_number: camera_contract::capture_result::FrameNumber(1),
            sensor_timestamp: None,
            flash: FlashResult {
                mode: Some(FlashModeEcho::Single),
                state: Some(FlashState::Fired),
            },
        },
        plan: FlashFramePlan {
            shot,
            ambient_frame: SensorTimestamp::from_nanos(1),
            mid_gray_product: ExposureProduct::from_seconds_iso(4.8).unwrap(),
            flash_share: FlashShare::new(0.6),
        },
        exposure_time: ExposureTime::new(Duration::from_millis(12)).unwrap(),
        iso: Iso::new(400).unwrap(),
        elapsed: Duration::from_millis(900),
    }
}

/// Выбор кадра «кадр разового запроса со вспышкой»: метка сенсора — на 700 мс ПОСЛЕ нажатия в шкале BOOTTIME
/// (кадр вспышки снимается после предвспышки и паузы).
pub fn flash_frame_choice(
    shot: FlashShotId,
    pressed_at: shell_contract::clock::MonotonicNanos,
) -> still_contract::frame_choice::FrameChoice {
    use crate::clock_offset::{ClockOffsetSource, SystemClocks};
    let offset = SystemClocks.sample_offset().expect("системные часы читаются");
    still_contract::frame_choice::FrameChoice::FlashFrame {
        sensor_timestamp: SensorTimestamp::from_nanos(
            offset.monotonic_to_boottime(pressed_at).as_nanos() + 700_000_000,
        ),
        shot,
    }
}
