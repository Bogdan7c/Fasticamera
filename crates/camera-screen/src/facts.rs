//! Факты камеры — нейтральный снимок того, что экран должен знать о камере.
//!
//! Экран не зависит от `camera-core`: приложение само переводит статус движка в [`CameraFacts`]
//! и шлёт действие `CameraFactsChanged`. Так экран тестируется без камеры, а движок можно менять,
//! не трогая экран.

use camera_contract::three_a::{AeAfLock, PointOfInterest};

use crate::exposure::ExposureRangeFact;
use crate::viewfinder_map::StreamGeometry;

/// Почему камера недоступна (экран покажет текст — виджеты P5b).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnavailableReason {
    /// Нет разрешения на камеру.
    PermissionDenied,
    /// Камеру занимает другое приложение или система.
    InUse,
    /// Отключена политикой устройства.
    DisabledByPolicy,
    /// Сбой камеры.
    Failure,
}

/// Присутствие камеры.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CameraPresence {
    /// Камера открывается, кадров ещё нет.
    #[default]
    Starting,
    /// Камера работает.
    Live,
    /// Камеры нет.
    Unavailable { reason: UnavailableReason },
}

/// Что делает автофокус с точки зрения экрана (стиль квадрата фокуса).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPhase {
    /// Непрерывный фокус по кадру, точки нет.
    #[default]
    Continuous,
    /// Тап отправлен, скан идёт.
    Scanning,
    /// Сфокусировано.
    Focused,
    /// Камера честно не нашла фокус.
    NotFocused,
    /// Итога нет: время ожидания вышло или тап не дошёл до камеры.
    NoResult,
}

/// Снимок фактов камеры. Отправляется целиком при каждом изменении; редьюсер сам находит разницу.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraFacts {
    /// Присутствие камеры.
    pub presence: CameraPresence,
    /// Куда камера сейчас смотрит автоматикой (весь кадр или точка с номером запроса).
    /// Номер запроса отличает новую точку от уже учтённой.
    pub point_of_interest: PointOfInterest,
    /// Блокировка AE/AF: плашка «AE/AF LOCK» видна, пока камера говорит `Locked`.
    pub lock: AeAfLock,
    /// Прогресс фокуса.
    pub focus: FocusPhase,
    /// Диапазон компенсации экспозиции камеры.
    pub exposure_range: ExposureRangeFact,
    /// Геометрия потока для перевода координат; `None` — потока ещё нет.
    pub stream: Option<StreamGeometry>,
}

impl CameraFacts {
    /// Факты до первого сообщения камеры: камера стартует, всё по умолчанию.
    pub const STARTING: CameraFacts = CameraFacts {
        presence: CameraPresence::Starting,
        point_of_interest: PointOfInterest::WholeFrame,
        lock: AeAfLock::Unlocked,
        focus: FocusPhase::Continuous,
        exposure_range: ExposureRangeFact::NotReported,
        stream: None,
    };
}

impl Default for CameraFacts {
    fn default() -> Self {
        Self::STARTING
    }
}
