//! Перенос «памяти экспозиции» прошлого потока на новую камеру (решения 118 и 126).
//!
//! Исполнитель экспозиции помнит цель прошлого потока в произведении «выдержка × ISO» **той
//! камеры**, на которой она набрана. Исполнитель не знает каталога, поэтому камеру помнит
//! движок: при старте потока на другой камере он считает множитель через
//! [`crate::exposure_transfer`] и отдаёт его исполнителю.
//!
//! Тут же движок объявляет рендеру, какую экспозицию ожидать от нового потока (`stream_start` в
//! указании): указание уходит ДО смены потока, и рендер открывает первый кадр без удержания (13b).

use camera_contract::auto_exposure::CarriedSceneExposure;
use camera_contract::ids::CameraId;

use super::super::{CameraEngine, LOG_TARGET};
use crate::exposure_transfer::{ExposureTransfer, exposure_transfer};

impl CameraEngine {
    /// Новый поток стартует на `camera` (обычная настройка сессии или повышение тёплой).
    ///
    /// Порядок важен: сначала исполнитель узнаёт камеру и получает пересчитанную память, и только потом
    /// рендеру объявляется ожидаемая экспозиция. Если память пересчитать нельзя (камеры нет в каталоге),
    /// ожидание НЕ объявляется: неверное произведение хуже отсутствующего (рендер просто дождётся замера).
    pub(in crate::engine) fn carry_exposure_memory_to(&mut self, camera: &CameraId) {
        self.three_a.exposure.begin_stream_on_camera(camera);
        let previous = self.three_a.exposure_camera.replace(camera.clone());
        if let Some(previous) = previous
            && previous != *camera
        {
            let Some(transfer) = self.transfer_between(&previous, camera) else {
                // Память осталась в произведении прошлой камеры: объявлять её новой нельзя.
                self.three_a.exposure_directive.announce_stream_start(None);
                return;
            };
            self.three_a.exposure.carry_memory_to_new_camera(&transfer);
        }
        self.announce_expected_exposure(camera);
    }

    /// Объявить рендеру ожидаемую экспозицию камеры `camera` ЗАРАНЕЕ, до обмена тёплой камеры: когда рендер
    /// увидит первый кадр новой основной, указание уже на месте. Состояние исполнителя не меняется; сам
    /// перенос памяти и повторное (то же) объявление сделает [`Self::carry_exposure_memory_to`] после обмена.
    pub(in crate::engine) fn announce_expected_exposure_before_swap(&mut self, camera: &CameraId) {
        let Some(previous) = self.three_a.exposure_camera.clone() else {
            return;
        };
        let transfer = if previous == *camera {
            Some(ExposureTransfer::IDENTITY)
        } else {
            self.transfer_between(&previous, camera)
        };
        let expected = transfer
            .and_then(|transfer| self.three_a.exposure.scene_target_after_transfer(&transfer))
            .map(|target| CarriedSceneExposure::for_camera(camera, target));
        self.three_a.exposure_directive.announce_stream_start(expected);
    }

    /// Обмен не состоялся: рендеру больше нечего ждать от нового потока.
    pub(in crate::engine) fn withdraw_expected_exposure(&mut self) {
        self.three_a.exposure_directive.announce_stream_start(None);
    }

    /// Множитель переноса произведения с камеры `from` на `to`; `None`, если камеры нет в каталоге.
    fn transfer_between(&self, from: &CameraId, to: &CameraId) -> Option<ExposureTransfer> {
        let (Some(from_camera), Some(to_camera)) = (self.catalog.camera(from), self.catalog.camera(to)) else {
            tracing::warn!(target: LOG_TARGET, previous = %from, camera = %to, "камеры нет в каталоге: память экспозиции не пересчитана");
            return None;
        };
        let transfer = exposure_transfer(&from_camera.capabilities, &to_camera.capabilities);
        if !transfer.is_fully_measured() {
            tracing::warn!(target: LOG_TARGET, ?transfer, "перенос экспозиции без части данных: этот член принят за 1");
        }
        Some(transfer)
    }

    /// Объявить рендеру ожидаемую экспозицию нового потока камеры `camera` (или «ожидать нечего»).
    fn announce_expected_exposure(&mut self, camera: &CameraId) {
        let expected = self
            .three_a
            .exposure
            .carried_scene_target()
            .map(|target| CarriedSceneExposure::for_camera(camera, target));
        self.three_a.exposure_directive.announce_stream_start(expected);
    }
}
