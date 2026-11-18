//! События движка наружу.

use camera_contract::availability::CameraUnavailableReason;
use camera_contract::stream::{ConfiguredStream, StreamMode};
use camera_contract::three_a::PointRequestId;
use serde::Serialize;

use crate::backend::ConfigureFailure;
use crate::command::{CommandId, CommandOutcome};
use crate::state::CameraState;
use crate::three_a_status::{BracketReport, ControlAdjustment, FocusOutcome};

/// Событие движка. Поток событий ограничен: при переполнении старые вытесняются со счётчиком
/// ([`crate::CameraCounters::dropped_outward_events`]), а актуальное состояние всегда есть
/// в снимке [`crate::CameraStatus`] — потеря события не теряет правду о камере.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CameraEvent {
    /// Итог команды с номером `command`.
    CommandHandled {
        command: CommandId,
        name: &'static str,
        outcome: CommandOutcome,
    },
    StateChanged {
        state: CameraState,
    },
    /// Камера пропала у приложения (вытеснение, сбой, разрешение…).
    Unavailable {
        reason: CameraUnavailableReason,
    },
    /// После недоступности поток снова идёт.
    Recovered {
        reason: CameraUnavailableReason,
        attempts: u32,
    },
    /// Сессия сконфигурирована: какой поток и каким рычагом частоты.
    StreamConfigured {
        stream: ConfiguredStream,
    },
    /// Режим не сконфигурировался на этой камере; желаемый режим и поток сброшены,
    /// устройство остаётся открытым.
    StreamConfigurationFailed {
        mode: StreamMode,
        failure: ConfigureFailure,
    },
    /// Запрошенную частоту не достичь (например, на камере нет вендорских ключей 4:3@60),
    /// и движок честно откатился на запасную частоту из конфига (решение 23). Желаемый режим
    /// запоминает именно `fallback`; `requested` и причина — для UI и отчётов.
    StreamDegraded {
        requested: StreamMode,
        fallback: StreamMode,
        failure: ConfigureFailure,
    },
    /// Тап `request` завершился (сошёлся, не сошёлся, вышло время).
    FocusSettled {
        request: PointRequestId,
        outcome: FocusOutcome,
    },
    /// Ручные параметры применены с поправками на этой камере и потоке (после команды или
    /// смены камеры). Пустого списка в событии не бывает.
    ControlsAdjusted {
        adjustments: Vec<ControlAdjustment>,
    },
    /// Итог серии с брекетингом.
    BracketCaptured {
        report: BracketReport,
    },
}

impl CameraEvent {
    /// Итог команды, если это он (для поиска по номеру).
    pub fn command_outcome(&self, id: CommandId) -> Option<&CommandOutcome> {
        match self {
            CameraEvent::CommandHandled { command, outcome, .. } if *command == id => Some(outcome),
            _ => None,
        }
    }
}
