//! Граница движка для вспышки (сессия 14d, решения 153, 158, 159): итог снимка со вспышкой,
//! совет для Auto и состояние последовательности в статусе.
//!
//! Последовательность (исполняет `engine/flash`):
//! `Idle → Preflash (фонарик в повторяющемся запросе, замер) → Gap (фонарик выключен, пауза)
//! → Fired (разовый запрос отправлен, ждём результат) → Idle`.
//! На каждую команду `CaptureFlashStill`, дошедшую до движка, — РОВНО одно событие
//! [`crate::event::CameraEvent::FlashStillFinished`] (успех или отказ с причиной), как правило
//! «ровно один итог» снимка в рендере (`mem:conventions`, «Снимок»).
//!
//! Инвариант: фонарик горит в повторяющемся запросе тогда и только тогда, когда
//! последовательность в фазе [`FlashPhase::Preflash`]. Любой выход из неё (успех, отказ,
//! потеря сессии, смена объектива, остановка) — повторяющийся запрос без фонарика.

use std::time::Duration;

use camera_contract::capture_result::{FrameNumber, SensorTimestamp};
use camera_contract::control::{ExposureTime, Iso};
use camera_contract::flash::{FlashFramePlan, FlashResult, FlashShotId, FlashStillFailure, FlashUnit};
use serde::Serialize;

/// Итог одного снимка со вспышкой.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FlashStillReport {
    /// Разовый запрос со вспышкой ушёл в HAL, и его результат пришёл.
    /// Освещён ли кадр на самом деле, решает рендер по пикселям: `FIRED` в результате —
    /// не доказательство (факт 14d: вспышка сразу после фонарика не горит, но сообщает `FIRED`).
    Fired {
        shot: FlashShotId,
        frame: FlashFrameResult,
        /// План, по которому выбраны выдержка и ISO (его же получил рендер в указании AE).
        plan: FlashFramePlan,
        exposure_time: ExposureTime,
        iso: Iso,
        /// От принятия команды до результата кадра со вспышкой.
        elapsed: Duration,
    },
    /// Снимок со вспышкой не снят.
    Failed {
        shot: FlashShotId,
        failure: FlashStillFailure,
    },
}

impl FlashStillReport {
    /// Номер снимка, к которому относится итог.
    pub const fn shot(&self) -> FlashShotId {
        match self {
            FlashStillReport::Fired { shot, .. } | FlashStillReport::Failed { shot, .. } => *shot,
        }
    }
}

/// Наблюдатель итогов снимков со вспышкой (14d), необязательный.
///
/// Зачем рядом с событием [`crate::event::CameraEvent::FlashStillFinished`]: в проде исходящую
/// очередь событий никто постоянно не разбирает (её разбирают дымовые сценарии), и она вытесняет
/// старое после 128 событий. Приложение вешает наблюдателя при сборке, и итог не теряется.
///
/// Контракт: зовётся на потоке камеры, ровно один раз на итог, в тот же момент, когда итог
/// кладётся в события, в том числе из пути восстановления после паники колбэка. Реализация НЕ должна
/// паниковать и НЕ должна блокировать (никаких ожиданий замков, ввода-вывода, каналов с ожиданием места):
/// она только кладёт итог в ограниченную очередь или ящик без ожидания. Блокировка остановила бы поток
/// камеры (кадры и сроки всего движка), паника на пути восстановления оборвала бы само восстановление.
pub trait FlashStillObserver: Send + Sync {
    fn flash_still_finished(&self, report: &FlashStillReport);
}

/// Что HAL сообщил о кадре разового запроса со вспышкой.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FlashFrameResult {
    pub frame_number: FrameNumber,
    /// `None` — HAL не отдал метку; кадр в кольце тогда не найти (рендер ищет по метке запроса,
    /// а метка запроса есть у кадра и без этого поля).
    pub sensor_timestamp: Option<SensorTimestamp>,
    pub flash: FlashResult,
}

/// Совет для режима Auto: вспыхнуть ли, если нажать сейчас (решение 159). Решает наш AE,
/// а не экран: экран и приложение только читают совет из статуса.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FlashAdvice {
    /// Сцена темнее порога `three_a.flash.auto_fire_min_mid_gray_log2_seconds_iso`.
    Fire,
    /// Света хватает: снимок без вспышки (из кольца, без задержки затвора).
    NotNeeded,
    /// Совета нет — Auto снимает без вспышки (честно, с причиной).
    Unavailable { reason: FlashAdviceUnavailable },
}

impl Default for FlashAdvice {
    /// До первого потока совета нет.
    fn default() -> Self {
        FlashAdvice::Unavailable {
            reason: FlashAdviceUnavailable::NotStreaming,
        }
    }
}

/// Почему совета для Auto нет.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FlashAdviceUnavailable {
    /// У активной камеры нет вспышки.
    NoFlashUnit,
    /// Экспозицию ведёт не наш AE.
    NotOwnExposure,
    /// Наш AE ещё не принял ни одного замера этого потока.
    NoMeasurement,
    /// Поток не идёт.
    NotStreaming,
}

/// Фаза последовательности вспышки (для статуса и тестов).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FlashPhase {
    #[default]
    Idle,
    /// Фонарик горит, ждём линейные замеры кадров без фонарика и с ним.
    Preflash { shot: FlashShotId, remeasures: u32 },
    /// Фонарик выключен, выдерживаем паузу перед разовой вспышкой (факт 14d: без паузы
    /// вспышка не горит).
    Gap { shot: FlashShotId },
    /// Разовый запрос отправлен, ждём его результат.
    Fired { shot: FlashShotId },
}

/// Учёт снимков со вспышкой с запуска движка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct FlashCounters {
    /// Команд `CaptureFlashStill`, дошедших до движка.
    pub requested: u64,
    /// Итогов `Fired`.
    pub fired: u64,
    /// Итогов `Failed`.
    pub failed: u64,
    /// Повторных замеров предвспышки (замер с фонариком пересвечен).
    pub preflash_remeasures: u64,
    /// Кадров разовой вспышки, у которых HAL не сообщил `FIRED` (рендер их не проявит).
    pub results_not_fired: u64,
    /// Планов, посчитанных по ПЕРЕСВЕЧЕННОМУ замеру с фонариком (перемеры исчерпаны или экспозицию уже
    /// некуда уменьшать). Известное ограничение: вклад фонарика занижен, кадр может выйти ярче цели;
    /// владелец предпочитает снимок отказу.
    pub planned_from_clipped_torch: u64,
}

/// Вспышка в статусе движка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct FlashStatus {
    /// Вспышка активной камеры; `Absent`, пока камеры нет.
    pub unit: FlashUnit,
    pub advice: FlashAdvice,
    pub phase: FlashPhase,
    pub counters: FlashCounters,
}
