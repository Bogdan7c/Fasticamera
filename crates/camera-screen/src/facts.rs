//! Факты камеры — нейтральный снимок того, что экран должен знать о камере.
//!
//! Экран не зависит от `camera-core`: приложение само переводит статус движка в [`CameraFacts`]
//! и шлёт действие `CameraFactsChanged`. Так экран тестируется без камеры, а движок можно менять,
//! не трогая экран.

use std::sync::Arc;

use camera_contract::ids::{CameraId, LensFacing, LensRole};
use camera_contract::three_a::{AeAfLock, PointOfInterest};
use camera_contract::zoom::ZoomCatalog;

use crate::exposure::ExposureRangeFact;
use crate::viewfinder_map::StreamGeometry;
use crate::white_facts::WhiteFacts;

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

/// Какая камера отдаёт кадры прямо сейчас (приложение берёт из статуса движка).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveLens {
    /// Камера активного потока; по ней кадрирование находит свою точку в каталоге зума.
    pub camera: CameraId,
    pub role: LensRole,
    pub facing: LensFacing,
}

/// Идёт ли переключение объектива (сессия 13). Размытие экран рисует по `BackdropContinuity` рендера; это
/// факт для логики («нельзя ли сейчас…»), а не источник картинки.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LensSwitch {
    /// Объектив не меняется.
    #[default]
    Steady,
    /// Движок переходит с `from` на `to`.
    Switching { from: CameraId, to: CameraId },
}

/// Последняя неудачная смена объектива. `sequence` растёт с каждой новой неудачей: по нему экран отличает
/// новую неудачу от уже учтённой (повторная доставка того же снимка фактов откат не повторяет).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensSwitchFailure {
    /// Номер неудачи, строго возрастающий в пределах процесса.
    pub sequence: u64,
    /// Камера, на которую не удалось перейти.
    pub target: CameraId,
}

/// Факты об объективах (сессия 13). Заполняет приложение из статуса движка; экран о движке не знает.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LensFacts {
    /// Шкалы зума по направлениям; `None` — движок каталога ещё не опубликовал.
    pub catalog: Option<Arc<ZoomCatalog>>,
    /// Активная камера; `None` — потока нет.
    pub active: Option<ActiveLens>,
    pub switch: LensSwitch,
    /// Последняя неудача смены объектива; `None` — неудач не было.
    pub last_failure: Option<LensSwitchFailure>,
}

impl LensFacts {
    /// Факты до первого сообщения: ни каталога, ни камеры.
    pub const NONE: LensFacts = LensFacts {
        catalog: None,
        active: None,
        switch: LensSwitch::Steady,
        last_failure: None,
    };
}

/// Есть ли у камеры вспышка, которой экран может пользоваться (инвариант 6: нет возможности — нет и значка).
///
/// Вспышка «вкл/выкл» появится в сессии 14d (решение 138); до неё приложение всегда сообщает `Unavailable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlashAvailability {
    /// Вспышки нет (или приложение ещё не умеет ей снимать): значок и пункт меню не показываются.
    #[default]
    Unavailable,
    /// Вспышка доступна: значок и пункт меню показываются, режим попадает в запрос снимка.
    Available,
}

/// Снимок фактов камеры. Отправляется целиком при каждом изменении; редьюсер сам находит разницу.
///
/// С сессии 13 не `Copy`: несёт общий каталог зума (`Arc`).
#[derive(Debug, Clone, PartialEq)]
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
    /// Объективы: каталог зума, активная камера, смена, неудача смены.
    pub lens: LensFacts,
    /// Доступна ли вспышка. Не доступна — экран её не показывает и снимает с `FlashMode::Off`.
    pub flash: FlashAvailability,
    /// Белое: действующее значение, пределы, ожидание и исход команды «взять» (white-balance 02).
    pub white: WhiteFacts,
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
        lens: LensFacts::NONE,
        flash: FlashAvailability::Unavailable,
        white: WhiteFacts::STARTING,
    };
}

impl Default for CameraFacts {
    fn default() -> Self {
        Self::STARTING
    }
}
