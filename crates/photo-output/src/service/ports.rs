//! Порты службы: что она получает снаружи (приложение, платформа), не зная реализации.

use camera_contract::ids::CameraId;
use still_contract::develop::PrimaryTargetOffer;
use still_contract::phase::StillPhase;

use crate::exif::{CameraDescription, CaptureMoment};

/// Приёмник фаз снимка (экран). `Secured` приходит с потока, который вызвал `submit_develop_event`
/// (рендер), итоги — с рабочего потока службы. Реализация обязана быть короткой и неблокирующей
/// (только короткие замки и `try_send`): `Secured` вызывается под замком учёта допуска. Обратно в
/// службу звать нельзя.
pub trait StillPhaseSink: Send + Sync {
    fn report_phase(&self, phase: StillPhase);
}

/// Канал к рендеру: сюда служба отдаёт цель для основной картинки. Реализует приложение.
pub trait PrimaryTargetSink: Send + Sync {
    /// Передаёт предложение рендеру. Не блокирует. `Err` — рендера нет (предложение пропало вместе
    /// с поверхностью кодера); служба тогда срывает снимок.
    fn offer_primary_target(&self, offer: PrimaryTargetOffer) -> Result<(), RenderUnavailable>;
}

/// Рендер не принял предложение цели (остановлен или канал закрыт).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("рендер недоступен")]
pub struct RenderUnavailable;

/// Настенные часы (инъекция ради тестов).
pub trait WallClock: Send + Sync {
    /// Текущий момент: UTC и смещение местного времени.
    fn now(&self) -> CaptureMoment;
}

/// Описания камер по `CameraId` (приложение заполняет из каталога камер).
pub trait CameraCatalog: Send + Sync {
    /// Описание камеры; `None` — камера неизвестна (снимок тогда срывается: без модели камеры
    /// нельзя честно записать DNG и Exif).
    fn describe_camera(&self, camera: &CameraId) -> Option<CameraDescription>;
}
