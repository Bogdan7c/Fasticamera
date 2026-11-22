//! Секция `photo` конфига нативной части: настройки службы записи снимков и её обвязки в приложении.
//!
//! Числа службы (`StillServiceConfig`) принадлежат `photo-output`, здесь лежат как есть вместе с их умолчаниями
//! и объяснениями происхождения. Приложению принадлежит только ёмкость журнала принятых снимков.
//! Kotlin секцию не присылает: действуют умолчания; частичная секция допустима, опечатка — ошибка.

use camera_screen::shutter::CaptureConfig;
use photo_output::service::{StillConfigError, StillServiceConfig};
use serde::{Deserialize, Serialize};

/// Предел ёмкости журнала: больше — уже не «ограниченный журнал», а скрытая утечка памяти.
const MAX_JOURNAL_CAPACITY: usize = 65_536;

/// Секция `photo`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PhotoConfig {
    /// Служба записи: допуск, очередь событий, сроки, качество, каталог.
    pub service: StillServiceConfig,
    /// Сколько последних принятых запросов снимка помнит журнал обвязки (для сценариев). [estimate] 64: как у
    /// журнала фейка; сценарию нужны единицы, а процесс живёт неделями.
    pub journal_capacity: usize,
}

impl Default for PhotoConfig {
    fn default() -> Self {
        Self {
            service: StillServiceConfig::default(),
            journal_capacity: 64,
        }
    }
}

/// Почему секция `photo` неверна.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PhotoConfigError {
    #[error(transparent)]
    Service(#[from] StillConfigError),
    #[error("photo.journal_capacity = {value}: нужно от 1 до {MAX_JOURNAL_CAPACITY}")]
    JournalCapacity { value: usize },
    /// Экран пускает в сохранение до `screen_limit` снимков, а служба допускает меньше: «лишние» нажатия
    /// получали бы отказ службы там, где экран обещал место. Конфиги двух crate-ов — согласуются на стыке.
    #[error(
        "photo.service.max_pending_stills = {service_limit} меньше camera_screen.screen.capture.max_saving_shots = {screen_limit}"
    )]
    ServiceLimitBelowScreenLimit { service_limit: usize, screen_limit: usize },
    /// Срок экрана — страховка на случай, что служба молчит: он обязан быть ДЛИННЕЕ срока службы. Иначе
    /// экран объявил бы потерянным (ошибка и вибрация) снимок, который служба ещё спокойно ведёт и
    /// сохранит в галерею, а миниатюра так и не обновится.
    #[error(
        "camera_screen.screen.capture.capture_timeout_ms = {screen_ms} не больше photo.service.capture_timeout_ms = {service_ms}"
    )]
    ScreenCaptureTimeoutNotAboveService { screen_ms: u64, service_ms: u64 },
    /// То же для ожидания итога после захвата кадра: экран ждёт полный срок проявления плюс завершения
    /// кодера HEIF (`develop_timeout_ms + heif_finish_timeout_ms`).
    #[error(
        "camera_screen.screen.capture.saving_timeout_ms = {screen_ms} не больше photo.service.develop_timeout_ms + heif_finish_timeout_ms = {service_ms}"
    )]
    ScreenSavingTimeoutNotAboveService { screen_ms: u64, service_ms: u64 },
}

impl PhotoConfig {
    /// Проверка при `initialize`. `screen` — секция `camera_screen.screen.capture`: её предел очереди и
    /// сроки обязаны согласоваться со службой (экран — страховка, его сроки длиннее сроков службы).
    pub fn validate(&self, screen: &CaptureConfig) -> Result<(), PhotoConfigError> {
        let screen_saving_limit = screen.max_saving_shots;
        self.service.validate()?;
        if self.journal_capacity == 0 || self.journal_capacity > MAX_JOURNAL_CAPACITY {
            return Err(PhotoConfigError::JournalCapacity {
                value: self.journal_capacity,
            });
        }
        if self.service.max_pending_stills < screen_saving_limit {
            return Err(PhotoConfigError::ServiceLimitBelowScreenLimit {
                service_limit: self.service.max_pending_stills,
                screen_limit: screen_saving_limit,
            });
        }
        let service = &self.service;
        let screen_capture_ms = u64::from(screen.capture_timeout_ms);
        if screen_capture_ms <= service.capture_timeout_ms {
            return Err(PhotoConfigError::ScreenCaptureTimeoutNotAboveService {
                screen_ms: screen_capture_ms,
                service_ms: service.capture_timeout_ms,
            });
        }
        // Экран отсчитывает срок сохранения от «кадр захвачен»; служба с этого момента тратит
        // `develop_timeout_ms`, а после итога рендера ещё до `heif_finish_timeout_ms` на кодер.
        let screen_saving_ms = u64::from(screen.saving_timeout_ms);
        let service_saving_ms = service
            .develop_timeout_ms
            .saturating_add(service.heif_finish_timeout_ms);
        if screen_saving_ms <= service_saving_ms {
            return Err(PhotoConfigError::ScreenSavingTimeoutNotAboveService {
                screen_ms: screen_saving_ms,
                service_ms: service_saving_ms,
            });
        }
        Ok(())
    }

    /// Ёмкость таблицы «снимок → экран»: ждущие снимки плюс запас на окно между освобождением слота службой
    /// и снятием записи (слот освобождается раньше, чем фаза доходит до таблицы).
    pub fn ledger_capacity(&self) -> usize {
        const SLOT_RELEASE_WINDOW: usize = 8;
        self.service.max_pending_stills.saturating_add(SLOT_RELEASE_WINDOW)
    }
}

#[cfg(test)]
#[path = "photo_config_tests.rs"]
mod tests;
