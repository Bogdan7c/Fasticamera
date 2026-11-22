//! М2 «машина состояний затвора» (`docs/ios-camera/feature-map.md`).
//!
//! Состояние — не `bool`, а `enum`: в v1 переходы есть только у `Idle`, `Countdown` и `Capturing`;
//! остальные варианты (`CapturingLong`, `Burst`, `Recording`, `RecordingLocked`) — места под ночной
//! режим, серию и видео, без переходов. Редьюсер уже умеет отвечать на них явным отказом `ShutterBusy`.

use camera_contract::viewfinder_crop::CropAspect;
use camera_contract::zoom::ZoomFraming;
use serde::{Deserialize, Serialize};
use shell_contract::clock::MonotonicNanos;
use shell_contract::orientation::DeviceOrientation;
// Переэкспорт: `PrimaryFormat` — тип поля `ShotRequest::primary_format`, приложению не нужна своя зависимость от словаря снимка.
pub use still_contract::plan::PrimaryFormat;
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

/// Запрос снимка сервису съёмки: всё, что экран знал **в момент нажатия** (решения 137–143).
/// Не путать с `CaptureIntent` из `camera-contract` — тот шаблон потока камеры, а это «сделай снимок
/// с такими настройками». Не `Copy`: кадрирование несёт общий каталог зума (`Arc`).
#[derive(Debug, Clone, PartialEq)]
pub struct ShotRequest {
    pub request: ShotRequestId,
    /// Момент нажатия на часах оболочки (`MonotonicNanos`, та же шкала, что у меток касаний и кадров).
    /// Откуда берётся:
    /// * тап по затвору — метка касания (`ShutterTapped { at }`);
    /// * клавиша громкости — метка нажатия (`VolumeKey { at }`);
    /// * таймер — момент, когда отсчёт кончился и снимок делается (метка `Tick`, на котором затвор сработал),
    ///   а НЕ момент нажатия кнопки таймера: кадр для снимка выбирают «на момент решения снимать».
    pub pressed_at: MonotonicNanos,
    /// Кадрирование экрана в этот момент: та же формула, что у видоискателя (`FramingIntent::zoom`),
    /// поэтому снимок = то, что было в кадре.
    pub framing: ZoomFraming,
    /// Как держали телефон. Пока датчик ничего не сообщил, экран считает телефон вертикальным
    /// (`DeviceOrientation::Portrait`) — обычное положение при съёмке и честное умолчание.
    pub orientation: DeviceOrientation,
    /// Формат основной картинки из конфига экрана (`capture.primary_format`).
    pub primary_format: PrimaryFormat,
    /// Вспышка. Пока камера её не поддерживает (`FlashAvailability::Unavailable`), всегда `Off`.
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

/// Итог снимка от сервиса съёмки (завершает снимок; фаза «кадр захвачен» — отдельное действие
/// `ScreenAction::CaptureSecured`, решение 140).
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

/// Ожидание и ёмкость двух фаз снимка (решение 140) и формат основной картинки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CaptureConfig {
    /// Сколько затвор может быть занят: от запроса снимка до «кадр захвачен» (`CaptureSecured`), мс.
    /// `[estimate]` 2000 мс — кадр берётся из кольца уже готовых кадров, захват занимает десятки мс, 2 с —
    /// большой запас на сбой (подгонка, не Apple). Чем короче срок, тем быстрее умерший сервис отпускает
    /// затвор; прежние 15 с держали бы пользователя без съёмки. Обязан быть длиннее срока захвата службы
    /// (`photo.service.capture_timeout_ms` 1 500): экран — страховка. По истечении снимок объявляется потерянным
    /// и затвор освобождается.
    pub capture_timeout_ms: u32,
    /// Сколько ждать сохранения уже захваченного снимка (`CaptureFinished`), мс. `[derived]` 45 с = срок
    /// проявления службы (30 с) + завершение кодера HEIF (5 с) + запас 10 с: срок экрана — страховка и обязан
    /// быть длиннее полного срока службы, иначе медленный, но живой снимок объявляется потерянным, а файл
    /// потом появляется в галерее без миниатюры (согласование проверяет приложение при старте). По
    /// истечении снимок считается потерянным (затвор при этом не трогается: он давно свободен).
    pub saving_timeout_ms: u32,
    /// Сколько снимков могут сохраняться одновременно. `[doc]` решение 140: 10 (≈ 45 МБ на снимок). Нажатие
    /// сверх ёмкости получает явный отказ `Rejection::SavingQueueFull`; уже снятое не теряется.
    pub max_saving_shots: usize,
    /// Формат основной картинки. `[doc]` решение 11: HEIF по умолчанию; выбор в настройках — сессия 16.
    pub primary_format: PrimaryFormat,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            capture_timeout_ms: 2_000,
            saving_timeout_ms: 45_000,
            max_saving_shots: 10,
            primary_format: PrimaryFormat::Heif,
        }
    }
}

/// Ошибка конфига ожидания снимка.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CaptureConfigError {
    #[error("срок захвата кадра должен быть больше нуля")]
    ZeroCaptureTimeout,
    #[error("срок сохранения снимка должен быть больше нуля")]
    ZeroSavingTimeout,
    #[error("ёмкость набора сохраняемых снимков должна быть не меньше 1")]
    ZeroSavingCapacity,
}

impl CaptureConfig {
    pub fn validate(&self) -> Result<(), CaptureConfigError> {
        if self.capture_timeout_ms == 0 {
            return Err(CaptureConfigError::ZeroCaptureTimeout);
        }
        if self.saving_timeout_ms == 0 {
            return Err(CaptureConfigError::ZeroSavingTimeout);
        }
        if self.max_saving_shots == 0 {
            return Err(CaptureConfigError::ZeroSavingCapacity);
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
