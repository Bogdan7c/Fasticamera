//! Публичное состояние «тёплых» камер (13b) для снимка и smoke: что открыто, готово ли и сколько
//! раз повышали, ронялись и отбрасывали результаты. Владелец состояния — `engine::standby`.

use camera_contract::ids::CameraId;
use serde::Serialize;

use crate::backend::{ConfigureFailure, DeviceErrorKind, OpenFailure, RequestFailure};
use crate::backend_standby::StandbySlotId;

/// Почему ячейка ушла в `Failed`. Основную камеру отказ тёплой никогда не трогает.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StandbyFailureReason {
    /// Устройство не открылось.
    OpenFailed { failure: OpenFailure },
    /// Сессия не сконфигурировалась.
    ConfigureFailed { failure: ConfigureFailure },
    /// Повторяющийся запрос не принят.
    RequestFailed { failure: RequestFailure },
    /// Тёплую отключили (вытеснение, отзыв разрешения).
    DeviceDisconnected,
    /// Ошибка устройства или службы тёплой.
    DeviceError { error: DeviceErrorKind },
    /// Платформа закрыла сессию тёплой, а движок её не закрывал.
    SessionClosedByPlatform,
}

/// Где ячейка в своём пути «закрыта → стримит».
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StandbySlotState {
    /// Свободна.
    Idle,
    /// Устройство открыто, сессии ещё нет.
    Opening,
    /// Сессия сконфигурирована, запроса ещё нет.
    Configured,
    /// Запрос стоит, первого результата кадра ещё не было: повышать рано.
    Warming,
    /// Первый результат пришёл: тёплую можно повысить обменом ролей.
    Ready,
    /// Отказ: ресурсов нет, повторов до нового спроса нет.
    Failed { reason: StandbyFailureReason },
}

/// Одна ячейка в снимке.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StandbySlotStatus {
    pub slot: StandbySlotId,
    /// Камера ячейки; `None` — ячейка свободна.
    pub camera: Option<CameraId>,
    pub state: StandbySlotState,
}

/// Счётчики тёплых камер за жизнь процесса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct StandbyCounters {
    /// Тёплые, которые дошли до открытого устройства.
    pub opened: u64,
    /// Повышения обменом ролей (без закрытия и открытия).
    pub promotions: u64,
    /// Обмен отвергнут бэкендом: движок пошёл обычным путём.
    pub promotion_fallbacks: u64,
    /// Ячейки, ушедшие в `Failed`.
    pub failures: u64,
    /// Результаты кадров бывшей основной, оставшиеся в ящике после обмена, отброшенные как
    /// «не устаревшие, а тёплые».
    pub results_dropped_after_swap: u64,
    /// Запросы тёплым (первый и пересчёты по заметной смене экспозиции основной).
    pub requests_issued: u64,
}

/// Состояние тёплых камер в снимке [`crate::LensStatus`].
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct StandbyStatus {
    /// Все ячейки (число — рабочая ёмкость; пусто, пока тёплые не понадобились).
    pub slots: Vec<StandbySlotStatus>,
    pub counters: StandbyCounters,
}
