//! Настройки экрана камеры в приложении (сессия 12): секция `camera_screen` конфига нативной части.
//!
//! Здесь живут только параметры ОБВЯЗКИ экрана (вкл/выкл, ёмкость канала входа, прореживание гравитации,
//! задержка фейк-снимка). Числа самого экрана (раскладка, шкала EV, таймер…) принадлежат
//! `camera_screen::ScreenConfig` и лежат внутри секции как `camera_screen.screen`.
//!
//! Kotlin секцию не присылает: действуют значения по умолчанию, у каждого помечен источник
//! (`[estimate]` — оценка, подгонка под телефон, не Apple). Частичная секция допустима, опечатка в имени
//! поля — ошибка (`deny_unknown_fields`).

use std::time::Duration;

use camera_screen::{ScreenConfig, ScreenConfigError};
use serde::{Deserialize, Serialize};
use ui_core::time::MotionPreference;

use crate::camera_screen_capture::FakeCaptureTiming;
use crate::look_facts::{BandSwatchStyle, BandSwatchStyleError};

/// Предел задержки фейк-снимка: дольше минуты ждать ответ фейка нет смысла (скорее всего опечатка в единицах).
const MAX_FAKE_CAPTURE_DELAY_MS: u64 = 60_000;

/// Предел ёмкости канала входа: больше — уже не «ограниченная очередь», а скрытая утечка памяти.
const MAX_INPUT_QUEUE_CAPACITY: usize = 65_536;

/// Предел паузы между попытками переустановки экрана: дольше минуты экран «пропал» для владельца.
const MAX_REINSTALL_PAUSE_MS: u64 = 60_000;

/// Предел числа попыток переустановки: больше — уже не «ограниченные попытки», а вечный цикл падений.
const MAX_REINSTALL_ATTEMPTS: u32 = 100;

/// Предел ёмкости журнала фейк-снимков: больше — скрытая утечка памяти.
const MAX_FAKE_CAPTURE_JOURNAL_CAPACITY: usize = 65_536;

/// Предел паузы между публикациями отчёта экрана: дольше сценарий видел бы устаревшее состояние.
const MAX_REPORT_INTERVAL_MS: u64 = 5_000;

/// Что делать, когда РЕНДЕР сам снял источник экрана (паника источника, отказ создания рендера UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct ScreenReinstallConfig {
    /// Сколько раз подряд пробуем поставить свежий экран, прежде чем признать отказ
    /// (`CameraScreenStatus::ReinstallExhausted`). [estimate] 3: сбой, который не лечится тремя свежими
    /// экранами подряд, сам не пройдёт, и вечные падения лишь жгут батарею.
    pub max_attempts: u32,
    /// Пауза перед каждой попыткой, мс. [estimate] 500: не крутить цикл «падение — установка» на частоте тиков.
    pub pause_ms: u64,
    /// Если экран прожил столько мс и только потом был снят, счёт попыток начинается заново: это новый,
    /// а не повторный сбой. [estimate] 10 с.
    pub healthy_after_ms: u64,
}

impl Default for ScreenReinstallConfig {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            pause_ms: 500,
            healthy_after_ms: 10_000,
        }
    }
}

impl ScreenReinstallConfig {
    /// Пауза перед попыткой как `Duration`.
    pub fn pause(&self) -> Duration {
        Duration::from_millis(self.pause_ms)
    }

    /// Срок «здоровой» жизни экрана как `Duration`.
    pub fn healthy_after(&self) -> Duration {
        Duration::from_millis(self.healthy_after_ms)
    }
}

/// Секция `camera_screen`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CameraScreenConfig {
    /// Ставить ли экран камеры поверх видоискателя. Выключенный экран — явный итог в диагностике
    /// (`CameraScreenStatus::Disabled`), касания при этом отбрасываются со счётчиком.
    /// [estimate] по умолчанию включён: экран — продукт сессии 12.
    pub enabled: bool,
    /// Числа самого экрана (раскладка, EV, таймер, виджеты) — владелец `camera-screen`.
    pub screen: ScreenConfig,
    /// Предпочтение движения (Reduce Motion). В v1 источник — конфиг; системную настройку читает сессия 16.
    pub motion: MotionPreference,
    /// Ёмкость канала «вход экрана» (касания, клавиши, ориентация, итоги снимка) между потоком
    /// маршрутизатора и потоком рендера. [estimate] 256: касания идут ~240 Гц, кадр рендера — 60–120 Гц,
    /// то есть запас больше полусекунды простоя рендера до первого переполнения.
    pub input_queue_capacity: usize,
    /// Гравитацию отдаём экрану раз в N кадров рендера (датчик быстрее глаза, уровню хватает 30 Гц).
    /// [estimate] 4: 120 Гц → 30 Гц.
    pub gravity_every_n_frames: u32,
    /// Через сколько мс фейк-сервис снимка (настоящий — сессия 14) отвечает `CaptureFinished(Saved)` (вторая
    /// фаза: сохранён). [estimate] 400 мс: заметная для затемнения экрана пауза. 0 — ответ сразу (тесты).
    pub fake_capture_delay_ms: u64,
    /// Через сколько мс фейк отвечает `CaptureSecured` (первая фаза: кадр захвачен, затвор свободен, решение 140).
    /// [estimate] 60 мс (≈ 4 кадра): настоящий захват берёт готовый кадр из кольца за десятки мс. Больше
    /// `fake_capture_delay_ms` не бывает: фейк зажимает срок до срока сохранения. 0 — захват сразу.
    pub fake_capture_secured_delay_ms: u64,
    /// Сколько последних запросов снимка помнит журнал фейка; старые вытесняются со счётчиком. [estimate] 64:
    /// сценарию `screen` нужны единицы, а процесс живёт неделями.
    pub fake_capture_journal_capacity: usize,
    /// Переустановка экрана после того, как рендер снял его сам.
    pub reinstall: ScreenReinstallConfig,
    /// Не чаще чем раз в столько мс источник публикует отчёт на доску (вход и эффекты кадра публикуют сразу).
    /// [estimate] 50 мс (~20 Гц): сценарий меряет секундами, а клон диагностики на каждом кадре 120 Гц — лишний
    /// мусор. 0 — каждый кадр (тесты).
    pub report_min_interval_ms: u64,
    /// Вид цветных точек полос в разделе калибровки (цвет считает приложение: экран не видит математику цвета).
    pub band_swatch: BandSwatchStyle,
}

impl Default for CameraScreenConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            screen: ScreenConfig::default(),
            motion: MotionPreference::Full,
            input_queue_capacity: 256,
            gravity_every_n_frames: 4,
            fake_capture_delay_ms: 400,
            fake_capture_secured_delay_ms: 60,
            fake_capture_journal_capacity: 64,
            reinstall: ScreenReinstallConfig::default(),
            report_min_interval_ms: 50,
            band_swatch: BandSwatchStyle::default(),
        }
    }
}

/// Почему секция `camera_screen` неверна.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CameraScreenConfigError {
    #[error("camera_screen.input_queue_capacity = {value}: нужно от 1 до {MAX_INPUT_QUEUE_CAPACITY}")]
    InputQueueCapacity { value: usize },
    #[error("camera_screen.gravity_every_n_frames = 0: нужно не меньше 1")]
    GravityEveryNFrames,
    #[error("camera_screen.fake_capture_delay_ms = {value}: не больше {MAX_FAKE_CAPTURE_DELAY_MS}")]
    FakeCaptureDelay { value: u64 },
    #[error("camera_screen.fake_capture_secured_delay_ms = {value}: не больше {MAX_FAKE_CAPTURE_DELAY_MS}")]
    FakeCaptureSecuredDelay { value: u64 },
    #[error("camera_screen.fake_capture_journal_capacity = {value}: нужно от 1 до {MAX_FAKE_CAPTURE_JOURNAL_CAPACITY}")]
    FakeCaptureJournalCapacity { value: usize },
    #[error("camera_screen.reinstall.max_attempts = {value}: нужно от 1 до {MAX_REINSTALL_ATTEMPTS}")]
    ReinstallAttempts { value: u32 },
    #[error("camera_screen.reinstall.pause_ms = {value}: не больше {MAX_REINSTALL_PAUSE_MS} мс")]
    ReinstallPause { value: u64 },
    #[error("camera_screen.reinstall.healthy_after_ms = 0: нужно больше нуля, иначе попытки не ограничены")]
    ReinstallHealthyAfterZero,
    #[error("camera_screen.report_min_interval_ms = {value}: не больше {MAX_REPORT_INTERVAL_MS}")]
    ReportInterval { value: u64 },
    #[error(transparent)]
    BandSwatch(#[from] BandSwatchStyleError),
    #[error("camera_screen.screen: {0}")]
    Screen(#[from] ScreenConfigError),
}

impl CameraScreenConfig {
    /// Проверка значений (вызывается из `RuntimeConfig::from_json`, чтобы неверный конфиг отвергался целиком).
    pub fn validate(&self) -> Result<(), CameraScreenConfigError> {
        if self.input_queue_capacity == 0 || self.input_queue_capacity > MAX_INPUT_QUEUE_CAPACITY {
            return Err(CameraScreenConfigError::InputQueueCapacity {
                value: self.input_queue_capacity,
            });
        }
        if self.gravity_every_n_frames == 0 {
            return Err(CameraScreenConfigError::GravityEveryNFrames);
        }
        if self.fake_capture_delay_ms > MAX_FAKE_CAPTURE_DELAY_MS {
            return Err(CameraScreenConfigError::FakeCaptureDelay {
                value: self.fake_capture_delay_ms,
            });
        }
        if self.fake_capture_secured_delay_ms > MAX_FAKE_CAPTURE_DELAY_MS {
            return Err(CameraScreenConfigError::FakeCaptureSecuredDelay {
                value: self.fake_capture_secured_delay_ms,
            });
        }
        if self.fake_capture_journal_capacity == 0
            || self.fake_capture_journal_capacity > MAX_FAKE_CAPTURE_JOURNAL_CAPACITY
        {
            return Err(CameraScreenConfigError::FakeCaptureJournalCapacity {
                value: self.fake_capture_journal_capacity,
            });
        }
        self.validate_reinstall()?;
        if self.report_min_interval_ms > MAX_REPORT_INTERVAL_MS {
            return Err(CameraScreenConfigError::ReportInterval {
                value: self.report_min_interval_ms,
            });
        }
        self.band_swatch.validate()?;
        self.screen.validate()?;
        Ok(())
    }

    fn validate_reinstall(&self) -> Result<(), CameraScreenConfigError> {
        let reinstall = &self.reinstall;
        if reinstall.max_attempts == 0 || reinstall.max_attempts > MAX_REINSTALL_ATTEMPTS {
            return Err(CameraScreenConfigError::ReinstallAttempts {
                value: reinstall.max_attempts,
            });
        }
        if reinstall.pause_ms > MAX_REINSTALL_PAUSE_MS {
            return Err(CameraScreenConfigError::ReinstallPause {
                value: reinstall.pause_ms,
            });
        }
        // Нулевой срок «здоровья» сбрасывал бы счёт при каждом падении: попытки перестали бы быть ограниченными.
        if reinstall.healthy_after_ms == 0 {
            return Err(CameraScreenConfigError::ReinstallHealthyAfterZero);
        }
        Ok(())
    }

    /// Задержка итога `Saved` фейк-снимка как `Duration` (вторая фаза).
    pub fn fake_capture_delay(&self) -> Duration {
        Duration::from_millis(self.fake_capture_delay_ms)
    }

    /// Сроки двух фаз фейк-снимка как `Duration`.
    pub fn fake_capture_timing(&self) -> FakeCaptureTiming {
        FakeCaptureTiming {
            secured_after: Duration::from_millis(self.fake_capture_secured_delay_ms),
            saved_after: Duration::from_millis(self.fake_capture_delay_ms),
        }
    }
}

#[cfg(test)]
#[path = "camera_screen_config_tests.rs"]
mod tests;
