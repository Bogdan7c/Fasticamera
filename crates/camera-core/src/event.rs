//! События движка наружу.

use camera_contract::availability::CameraUnavailableReason;
use std::time::Duration;

use camera_contract::ids::{CameraId, LensRole};
use camera_contract::stream::{ConfiguredStream, StreamMode};
use camera_contract::three_a::PointRequestId;
use serde::Serialize;

use crate::backend::ConfigureFailure;
use crate::command::{CommandId, CommandOutcome};
use crate::flash_status::FlashStillReport;
use crate::lens_status::{LensSwitchFailure, LensSwitchMethod};
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
    /// остаётся запрошенным; потолок `fallback` запоминается за этой камерой
    /// (следующая настройка этой камеры сразу идёт на него — событие `StreamLimitedByCamera`);
    /// `requested` и причина — для UI и отчётов.
    StreamDegraded {
        requested: StreamMode,
        fallback: StreamMode,
        failure: ConfigureFailure,
    },
    /// Камера уже показала, что запрошенную частоту не держит (потолок известен по прошлому
    /// `StreamDegraded`): сессия настраивается сразу на `effective`, без повторной неудачной
    /// попытки. Желаемый режим остаётся `requested`.
    StreamLimitedByCamera {
        camera: CameraId,
        requested: StreamMode,
        effective: StreamMode,
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
    /// Итог снимка со вспышкой (14d): ровно один на команду `CaptureFlashStill`.
    FlashStillFinished {
        report: FlashStillReport,
    },
    /// Первый кадр новой камеры пришёл: смена `from → to` завершена (экран проявляет новую
    /// камеру из удержанного размытого кадра). `elapsed` — от намерения или команды, начавших
    /// смену, до этого кадра. `from == to` — возврат на прежний объектив после отменённой или
    /// провалившейся смены: кадры снова идут, проявлять нужно ту же камеру.
    LensSwitched {
        from: LensRole,
        to: LensRole,
        elapsed: Duration,
        /// Как смена исполнена: переоткрытием или повышением тёплой (13b).
        method: LensSwitchMethod,
    },
    /// Цель смены поменялась посреди смены: промежуточная `abandoned` отменена явно, смена
    /// продолжается из прежнего `from` в новую `to` (если `to == from` — это возврат).
    LensSwitchRetargeted {
        from: LensRole,
        abandoned: LensRole,
        to: LensRole,
    },
    /// Новая камера `to` не удалась: желаемой снова стала `from`, а `to` заблокирована для
    /// селектора (см. `LensStatus::blocked`).
    LensSwitchFailed {
        from: LensRole,
        to: LensRole,
        reason: LensSwitchFailure,
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
