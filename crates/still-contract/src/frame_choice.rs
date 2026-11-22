//! Какой кадр сенсора выбран для снимка и почему.
//!
//! Правило сессии 14: «время нажатия → кадр, который был на экране; кольцо пустое → явная ошибка;
//! кадр уже вытеснен → ближайший по правилу с пометкой». Пометка не прячется: она идёт в отчёт снимка
//! и в счётчики, чтобы «снимок без задержки» был доказан, а не предположен.

use camera_contract::capture_result::SensorTimestamp;
use serde::Serialize;
use shell_contract::clock::MonotonicNanos;

/// Как выбран кадр снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FrameChoice {
    /// Ровно тот кадр, что был на экране в момент нажатия.
    ShownAtPress {
        sensor_timestamp: SensorTimestamp,
        /// Когда этот кадр появился на экране (часы `MONOTONIC`, по записи показа драйвера).
        shown_at: MonotonicNanos,
    },
    /// Того кадра взять нельзя; взят ближайший к нему годный кадр кольца.
    Substituted {
        sensor_timestamp: SensorTimestamp,
        /// Метка кадра, который хотели (если она известна).
        wanted: Option<SensorTimestamp>,
        reason: SubstitutionReason,
    },
}

impl FrameChoice {
    /// Метка сенсора выбранного кадра.
    pub fn sensor_timestamp(&self) -> SensorTimestamp {
        match self {
            Self::ShownAtPress { sensor_timestamp, .. } | Self::Substituted { sensor_timestamp, .. } => {
                *sensor_timestamp
            }
        }
    }

    /// Снят ли ровно показанный кадр (без замены).
    pub fn is_exact(&self) -> bool {
        matches!(self, Self::ShownAtPress { .. })
    }
}

/// Почему взят не показанный кадр.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubstitutionReason {
    /// Показанный кадр уже вытеснен из кольца (нажатие обработано слишком поздно).
    ShownFrameEvicted,
    /// У показанного кадра нет метаданных (решение 42: снимок вправе такой кадр отвергнуть).
    ShownFrameWithoutMetadata,
    /// Показанный кадр — не обычный кадр потока (кадр серии/брекетинга), снимать его нельзя.
    ShownFrameNotRegular,
    /// Время показа для момента нажатия неизвестно (записей показа нет или они не дошли за срок):
    /// взят кадр, ближайший к оценке.
    PresentationTimingUnknown,
}

/// Почему кадра для снимка нет совсем.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, thiserror::Error)]
#[serde(rename_all = "snake_case")]
pub enum NoFrameReason {
    /// Кольцо снимков выключено (ёмкость 0).
    #[error("кольцо снимков выключено")]
    RingDisabled,
    /// В кольце нет ни одного годного кадра (поток только начался или остановлен).
    #[error("в кольце нет годного кадра")]
    RingEmpty,
    /// Камера не показывает кадры (нет потока, удержанный кадр смены объектива, экран без камеры).
    #[error("на экране нет живого кадра камеры")]
    NoLiveCameraFrame,
    /// Очередь снимков рендера полна (решение 140: экран и служба ограничивают раньше — это страховка).
    #[error("очередь снимков рендера полна")]
    RendererBusy,
    /// Выбранный кадр нельзя проявить: нет параметров ISP этого потока, метаданных или калибровки.
    #[error("выбранный кадр нельзя проявить")]
    FrameUnusable,
    /// GPU снимков недоступен или копия кадра сорвалась.
    #[error("копия кадра на GPU не удалась")]
    CaptureFailed,
    /// Рендер остановился или отпустил экран раньше, чем кадр был захвачен.
    #[error("рендер остановлен до захвата кадра")]
    RendererStopped,
}
