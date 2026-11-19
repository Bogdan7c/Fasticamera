//! М2 «машина состояний затвора» (`docs/ios-camera/feature-map.md`).
//!
//! Состояние — не `bool`, а `enum`: в v1 переходы есть только у `Idle`, `Countdown` и `Capturing`;
//! остальные варианты (`CapturingLong`, `Burst`, `Recording`, `RecordingLocked`) — места под ночной
//! режим, серию и видео, без переходов. Редьюсер уже умеет отвечать на них явным отказом `ShutterBusy`.

use camera_contract::viewfinder_crop::CropAspect;
use serde::{Deserialize, Serialize};
use shell_contract::clock::MonotonicNanos;
use thiserror::Error;

use crate::settings::{AspectRatio, FlashMode, RawCapture, TimerSetting};

/// Номер запроса снимка. Выдаёт экран (по возрастанию), по нему сервис снимка отвечает
/// `CaptureFinished`, а экран отличает свежий ответ от устаревшего.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ShotRequestId(pub u64);

/// Можно ли отменить длинную съёмку (ночной режим).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LongCaptureCancel {
    Allowed,
    NotAllowed,
}

/// Состояние затвора.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShutterState {
    /// Затвор свободен.
    #[default]
    Idle,
    /// Идёт отсчёт таймера; снимок запросится, когда время `ends_at` наступит.
    Countdown { ends_at: MonotonicNanos },
    /// Снимок запрошен, ждём `CaptureFinished` с этим номером.
    Capturing { request: ShotRequestId },
    /// Задел: длинная съёмка (ночь) с прогрессом в процентах (кольцо вокруг затвора).
    CapturingLong {
        request: ShotRequestId,
        progress_percent: u8,
        cancel: LongCaptureCancel,
    },
    /// Задел: серия, сделано кадров.
    Burst { request: ShotRequestId, shots_taken: u32 },
    /// Задел: запись видео с момента `since`.
    Recording { since: MonotonicNanos },
    /// Задел: запись на замке (без пальца).
    RecordingLocked { since: MonotonicNanos },
}

impl ShutterState {
    /// Идёт ли отсчёт таймера (затвор в этот момент отменяет его).
    pub fn is_counting_down(self) -> bool {
        matches!(self, ShutterState::Countdown { .. })
    }
}

/// Запрос снимка сервису съёмки. Не путать с `CaptureIntent` из `camera-contract` — тот шаблон
/// потока камеры, а это «сделай снимок с такими настройками».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShotRequest {
    pub request: ShotRequestId,
    pub flash: FlashMode,
    pub raw: RawCapture,
    pub aspect: AspectRatio,
}

impl ShotRequest {
    /// Соотношение запроса на языке кадрирования сенсора (удобство для сервиса снимка).
    pub fn crop_aspect(&self) -> CropAspect {
        self.aspect.crop_aspect()
    }
}

/// Почему снимок не получился (стиль хаптики один — `Error`; причина для диагностики).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureFailureReason {
    /// Сбой камеры.
    Camera,
    /// Сбой обработки (ISP).
    Processing,
    /// Файл не записан.
    Storage,
}

/// Итог снимка от сервиса съёмки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureOutcome {
    /// Снимок сохранён.
    Saved,
    /// Снимок не получился.
    Failed(CaptureFailureReason),
}

/// Последний снимок для миниатюры (картинку даст сервис снимка позже, сессия 14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThumbnailState {
    /// Снимков за эту сессию ещё нет.
    #[default]
    Empty,
    /// Есть последний снимок.
    Last { shot: ShotRequestId },
}

/// Ожидание итога снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureConfig {
    /// Сколько ждать `CaptureFinished` после запроса снимка, мс. `[estimate]` 15 с — запас на
    /// RAW + обработку одного кадра (подгонка, не Apple); по истечении снимок объявляется потерянным
    /// и затвор освобождается (иначе умерший сервис снимка заблокировал бы затвор навсегда).
    pub result_timeout_ms: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            result_timeout_ms: 15_000,
        }
    }
}

/// Ошибка конфига ожидания снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("время ожидания итога снимка должно быть больше нуля")]
pub struct CaptureConfigError;

impl CaptureConfig {
    pub fn validate(&self) -> Result<(), CaptureConfigError> {
        if self.result_timeout_ms == 0 {
            return Err(CaptureConfigError);
        }
        Ok(())
    }
}

/// Длительности таймера по слотам.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimerConfig {
    /// Секунд в слоте `Seconds3`. `[doc]` 3 (UG/TOOLS: «Tap Timer, then tap 3, 5, or 10»).
    pub short_seconds: u32,
    /// Секунд в слоте `Seconds5`. `[doc]` 5.
    pub medium_seconds: u32,
    /// Секунд в слоте `Seconds10`. `[doc]` 10.
    pub long_seconds: u32,
}

impl Default for TimerConfig {
    fn default() -> Self {
        Self {
            short_seconds: 3,
            medium_seconds: 5,
            long_seconds: 10,
        }
    }
}

/// Ошибка конфига таймера.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("длительности таймера должны быть > 0 и строго возрастать: {short}, {medium}, {long}")]
pub struct TimerConfigError {
    pub short: u32,
    pub medium: u32,
    pub long: u32,
}

impl TimerConfig {
    pub fn validate(&self) -> Result<(), TimerConfigError> {
        let valid = self.short_seconds > 0
            && self.short_seconds < self.medium_seconds
            && self.medium_seconds < self.long_seconds;
        if valid {
            Ok(())
        } else {
            Err(TimerConfigError {
                short: self.short_seconds,
                medium: self.medium_seconds,
                long: self.long_seconds,
            })
        }
    }

    /// Длительность отсчёта; `None` — таймер выключен.
    pub fn seconds_for(&self, setting: TimerSetting) -> Option<u32> {
        match setting {
            TimerSetting::Off => None,
            TimerSetting::Seconds3 => Some(self.short_seconds),
            TimerSetting::Seconds5 => Some(self.medium_seconds),
            TimerSetting::Seconds10 => Some(self.long_seconds),
        }
    }
}
