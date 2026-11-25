//! Команды движку (intent-методы [`crate::CameraHandle`]) и их явные итоги.

use camera_contract::catalog::TargetUnavailable;
use camera_contract::control::{CaptureIntent, ExposureBias, ManualControl, WhiteBalanceSetting};
use camera_contract::flash::FlashShotId;
use camera_contract::ids::{CameraTarget, LensRole};
use camera_contract::stream::StreamMode;
use camera_contract::three_a::{ExposureBracket, SensorPoint, ThreeAProfile};
use camera_contract::white::{WhiteAdjustment, WhiteSourceId, WhiteTransition};
use serde::Serialize;

use crate::backend::UnsupportedStream;

/// Номер команды: выдаётся при отправке, по нему итог находится в событиях.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct CommandId(pub u64);

/// Команда движку. Каждая меняет **желаемое** состояние; движок сам доводит до него
/// реальное, в том числе после вытеснения (решение 39).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CameraCommand {
    /// Хотим эту камеру открытой.
    Open {
        target: CameraTarget,
    },
    /// Хотим этот режим потока на желаемой камере.
    Configure {
        mode: StreamMode,
    },
    /// Хотим, чтобы шёл повторяющийся запрос.
    StartStreaming {
        intent: CaptureIntent,
    },
    StopStreaming,
    /// Перейти на камеру другой роли, сохранив режим и поток.
    SwitchTo {
        role: LensRole,
    },
    /// Ручные параметры целиком (домен 08). Диапазоны проверяются по текущей камере; поправки —
    /// событием `ControlsAdjusted` и в снимке, а не молча.
    ApplyControls {
        controls: ManualControl,
    },
    /// Тап: однократный фокус и замер в точке (iOS: жёлтый квадрат). Номер тапа — номер
    /// этой команды (`PointRequestId` = `CommandId`). Снимает блокировку AE/AF.
    FocusAndExposeAt {
        point: SensorPoint,
    },
    /// Долгий тап: фокус и замер в точке, затем блокировка AE/AF до снятия или смены камеры.
    LockFocusAndExposureAt {
        point: SensorPoint,
    },
    /// Вернуться к непрерывному фокусу и замеру по всему кадру, сняв блокировку.
    ResetPointOfInterest,
    /// Компенсация экспозиции (iOS: ползунок «солнце» рядом с точкой).
    SetExposureBias {
        bias: ExposureBias,
    },
    /// Профиль автоматики (V6).
    SetThreeAProfile {
        profile: ThreeAProfile,
    },
    /// Серия с брекетингом поверх идущего потока (V10). Итог — событие `BracketCaptured`.
    CaptureBracket {
        bracket: ExposureBracket,
    },
    /// Снимок со вспышкой `shot` (14d): предвспышка, пауза, разовый запрос со вспышкой.
    /// Итог — ровно одно событие `FlashStillFinished` на каждую команду, дошедшую до движка
    /// (в том числе на отказ: он приходит и как `CommandHandled`, и как итог снимка).
    CaptureFlashStill {
        shot: FlashShotId,
    },
    /// Белая рукой (кнопка света, шкала). Рука сильнее источника: ожидающая заявка отменяется. Белая живёт
    /// вне желаемого состояния: `Close`, `ApplyControls` и `SwitchTo` её не трогают. Прижим к пределам виден
    /// событием `ControlsAdjusted` и статусом.
    SetWhite {
        setting: WhiteBalanceSetting,
        transition: WhiteTransition,
    },
    /// Правка ОДНОЙ оси белой рукой (шкала под пальцем, WB31). Вторую ось владелец берёт у **своей** действующей
    /// белой в момент применения, а не у экрана: экран, чьи факты отстали на кадр, не затрёт её старым числом.
    /// Дальше всё как у `SetWhite`: происхождение «ручное», прижим к пределам, отмена ожидающей заявки.
    AdjustWhite {
        adjustment: WhiteAdjustment,
        transition: WhiteTransition,
    },
    /// Взять белую у источника (долгое нажатие, кнопка «Телефон»). Нужен идущий поток. Итог заявки — событие
    /// `WhiteRequestFinished`, белая прежняя до принятия ответа.
    TakeWhite {
        source: WhiteSourceId,
        transition: WhiteTransition,
    },
    /// Камера не нужна: закрыть и забыть желаемое.
    Close,
    /// Приложение ушло с переднего плана: отпустить камеру, желаемое сохранить.
    EnterBackground,
    /// Приложение снова на переднем плане: восстановить желаемое, счётчик попыток с нуля.
    EnterForeground,
}

impl CameraCommand {
    pub fn name(&self) -> &'static str {
        match self {
            CameraCommand::Open { .. } => "open",
            CameraCommand::Configure { .. } => "configure",
            CameraCommand::StartStreaming { .. } => "start_streaming",
            CameraCommand::StopStreaming => "stop_streaming",
            CameraCommand::SwitchTo { .. } => "switch_to",
            CameraCommand::ApplyControls { .. } => "apply_controls",
            CameraCommand::FocusAndExposeAt { .. } => "focus_and_expose_at",
            CameraCommand::LockFocusAndExposureAt { .. } => "lock_focus_and_exposure_at",
            CameraCommand::ResetPointOfInterest => "reset_point_of_interest",
            CameraCommand::SetExposureBias { .. } => "set_exposure_bias",
            CameraCommand::SetThreeAProfile { .. } => "set_three_a_profile",
            CameraCommand::CaptureBracket { .. } => "capture_bracket",
            CameraCommand::CaptureFlashStill { .. } => "capture_flash_still",
            CameraCommand::SetWhite { .. } => "set_white",
            CameraCommand::AdjustWhite { .. } => "adjust_white",
            CameraCommand::TakeWhite { .. } => "take_white",
            CameraCommand::Close => "close",
            CameraCommand::EnterBackground => "enter_background",
            CameraCommand::EnterForeground => "enter_foreground",
        }
    }
}

/// Почему команда противоречит текущему желаемому состоянию.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidStateReason {
    /// Нужна желаемая камера (`open`), а её нет.
    NoCameraRequested,
    /// Нужен режим потока (`configure`), а его нет.
    NoStreamModeRequested,
    /// Движок в окончательной ошибке и команд не исполняет.
    EngineFailed,
    /// Нужен идущий поток (тап, серия), а его нет.
    NotStreaming,
    /// Предыдущая серия ещё не завершена.
    BracketInProgress,
    /// Предыдущий снимок со вспышкой ещё не завершён.
    FlashInProgress,
}

/// Почему команду нельзя исполнить на этом телефоне.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum UnsupportedCommand {
    Target(TargetUnavailable),
    Stream(UnsupportedStream),
    /// Камера не умеет то, что нужно команде (фиксированный фокус для тапа, нет ручного
    /// сенсора для серии, нет блокировки AE…).
    ThreeA(UnsupportedThreeA),
}

/// Какой возможности 3A у текущей камеры нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UnsupportedThreeA {
    /// Фокус фиксированный: тап меняет только замер, но фокусироваться нечем.
    FixedFocusLens,
    /// Нет `AF_MODE_AUTO` — однократный фокус по тапу невозможен.
    SingleShotFocus,
    /// Нет `aeLockAvailable`.
    ExposureLock,
    /// Нет ручного сенсора (`MANUAL_SENSOR`) — серия с экспозициями невозможна.
    ManualSensor,
    /// Полуавтомат (одна из ISO/выдержки ручная, другая авто) требует нашего AE — сессия 08a
    /// (решения 63, 66). До неё честный отказ, а не подмена «всё авто» или «всё вручную».
    SemiAutomaticExposure,
    /// В серии больше кадров, чем разрешает конфиг (`three_a.max_bracket_frames` — бюджет пула
    /// RAW-кадров). Добавлено сессией 08 к границе: честного другого варианта в итогах нет.
    BracketTooLong,
    /// У активной камеры нет вспышки (14d).
    FlashUnit,
    /// Снимок со вспышкой считается только нашим AE (решения 63, 158); при вендорском AE — отказ.
    FlashNeedsOwnExposure,
}

/// Итог команды. Не `bool`: «уже так», «невозможно» и «не в том состоянии» —
/// разные ответы для вызывающего (правило 6 `AGENTS.md`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum CommandOutcome {
    /// Желаемое состояние изменено; движок доводит до него реальное.
    Accepted,
    /// Желаемое уже такое (например, повторный `open` той же камеры) — ничего не менялось.
    AlreadyInState,
    /// На этом телефоне невозможно (нет роли, формат не предлагается…). Желаемое не менялось.
    Unsupported(UnsupportedCommand),
    /// Противоречит текущему желаемому состоянию. Желаемое не менялось.
    InvalidState(InvalidStateReason),
}

/// Команду не удалось даже отправить. Тоже не `bool`: очередь полна (подождать)
/// и движок остановлен (бесполезно ждать) — разные ситуации.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CommandRejected {
    #[error("очередь команд камеры полна")]
    QueueFull,
    #[error("движок камеры остановлен")]
    EngineStopped,
}
