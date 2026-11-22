//! Секция `photo` конфига нативной части: настройки службы записи снимков и её обвязки в приложении.
//!
//! Числа службы (`StillServiceConfig`) принадлежат `photo-output`, здесь лежат как есть вместе с их умолчаниями
//! и объяснениями происхождения. Приложению принадлежит только ёмкость журнала принятых снимков.
//! Kotlin секцию не присылает: действуют умолчания; частичная секция допустима, опечатка — ошибка.

use camera_screen::shutter::CaptureConfig;
use photo_output::service::{StillConfigError, StillServiceConfig};
use render_loop::config::StillDevelopConfig;
use serde::{Deserialize, Serialize};

/// Предел ёмкости журнала: больше — уже не «ограниченный журнал», а скрытая утечка памяти.
const MAX_JOURNAL_CAPACITY: usize = 65_536;

/// Сколько мс, по оценке, снимок впереди в очереди держит кодер, пока проявляется и завершается (`[estimate]`):
/// проявление на телефоне ≈ 0,5 с, у верхней границы 1–2 с, плюс `finish` кодера HEIF. Верхняя граница — 2 с.
/// Совпадает с `render_loop::config` (`still.rs`, `ESTIMATED_MS_PER_QUEUED_STILL`): отсюда выведено умолчание срока
/// цели рендера. Пересмотреть по замеру на телефоне (14c).
const ESTIMATED_MS_PER_QUEUED_STILL: u64 = 2_000;

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
    /// Служба выдаёт цель кодера по очереди (`max_open_encoders`), и последний ждущий снимок ждёт цель, пока
    /// проявятся все впереди. Рендер обязан ждать цель не меньше этого худшего ожидания, иначе серия из полной
    /// очереди теряла бы снимки по `TargetTimedOut`, хотя всё исправно.
    #[error(
        "render.still.target_timeout_ms = {render_ms} меньше худшего ожидания очереди {queue_wait_ms} мс (photo.service.max_pending_stills = {pending})"
    )]
    RenderTargetTimeoutBelowQueueWait {
        render_ms: u64,
        queue_wait_ms: u64,
        pending: usize,
    },
    /// Рендер должен отказаться ждать цель РАНЬШЕ, чем служба сорвёт снимок по сроку проявления (правило
    /// «экран > служба > рендер»): иначе служба успеет сорвать снимок, который рендер ещё честно ждёт.
    #[error("render.still.target_timeout_ms = {render_ms} не меньше photo.service.develop_timeout_ms = {service_ms}")]
    RenderTargetTimeoutNotBelowService { render_ms: u64, service_ms: u64 },
    /// Рендер ждёт кадр со вспышкой (14d) не дольше, чем камере нужно на всю последовательность (предвспышка,
    /// пауза, результат): рендер сдавался бы `FlashFrameNeverArrived` раньше, чем камера честно закончит.
    #[error(
        "render.still.flash_frame_wait_ms = {render_ms} не больше худшего срока вспышки камеры camera.engine.three_a.flash (preflash + пауза + результат) = {camera_ms} мс"
    )]
    FlashFrameWaitNotAboveCameraSequence { render_ms: u64, camera_ms: u64 },
    /// Служба сорвала бы снимок со вспышкой по сроку `capture_timeout_ms` раньше, чем рендер сам откажется ждать
    /// кадр (`flash_frame_wait_ms`): итог был бы «служба молчит», а не причина.
    #[error(
        "photo.service.capture_timeout_ms = {service_ms} не больше render.still.flash_frame_wait_ms = {render_ms}: служба сорвёт снимок со вспышкой раньше рендера"
    )]
    ServiceCaptureTimeoutNotAboveFlashFrameWait { service_ms: u64, render_ms: u64 },
    /// Экран объявил бы снимок со вспышкой потерянным раньше, чем служба довела бы его до итога.
    #[error(
        "camera_screen.screen.capture.capture_timeout_ms = {screen_ms} не больше photo.service.capture_timeout_ms = {service_ms} (цепочка сроков вспышки)"
    )]
    ScreenCaptureTimeoutNotAboveFlashChain { screen_ms: u64, service_ms: u64 },
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

    /// Сверка со срезом рендера (`render.still`): сроки трёх уровней — экран > служба > рендер — и ожидание цели
    /// кодера в очереди. Зовётся при `initialize` рядом с [`PhotoConfig::validate`].
    pub fn validate_against_render(&self, render: &StillDevelopConfig) -> Result<(), PhotoConfigError> {
        let service = &self.service;
        // Последний из ждущих снимков ждёт цель, пока проявятся и завершатся все впереди. Сколько из них
        // держит место сразу, не влияет на худший случай: рендер проявляет строго по одному.
        let ahead = u64::try_from(service.max_pending_stills.saturating_sub(1)).unwrap_or(u64::MAX);
        let queue_wait_ms = ahead.saturating_mul(ESTIMATED_MS_PER_QUEUED_STILL);
        let render_target_ms = u64::from(render.target_timeout_ms);
        if render_target_ms < queue_wait_ms {
            return Err(PhotoConfigError::RenderTargetTimeoutBelowQueueWait {
                render_ms: render_target_ms,
                queue_wait_ms,
                pending: service.max_pending_stills,
            });
        }
        if render_target_ms >= service.develop_timeout_ms {
            return Err(PhotoConfigError::RenderTargetTimeoutNotBelowService {
                render_ms: render_target_ms,
                service_ms: service.develop_timeout_ms,
            });
        }
        Ok(())
    }

    /// Цепочка сроков снимка со вспышкой (14d): камера (вся последовательность, `camera_worst_case_ms`) <
    /// рендер (`flash_frame_wait_ms`) < служба (`capture_timeout_ms`) < экран (`capture_timeout_ms`).
    ///
    /// Каждый внешний слой — страховка внутреннего: он обязан сдаваться ПОЗЖЕ, иначе итог снимка скажет «слой
    /// выше молчит» вместо настоящей причины. Звено «служба < экран» общее с обычным снимком и уже проверяет
    /// [`PhotoConfig::validate`]; здесь оно повторено, чтобы цепочка вспышки читалась и проверялась целиком.
    pub fn validate_flash_deadlines(
        &self,
        camera_worst_case_ms: u64,
        render: &StillDevelopConfig,
        screen: &CaptureConfig,
    ) -> Result<(), PhotoConfigError> {
        let render_ms = u64::from(render.flash_frame_wait_ms);
        if render_ms <= camera_worst_case_ms {
            return Err(PhotoConfigError::FlashFrameWaitNotAboveCameraSequence {
                render_ms,
                camera_ms: camera_worst_case_ms,
            });
        }
        let service_ms = self.service.capture_timeout_ms;
        if service_ms <= render_ms {
            return Err(PhotoConfigError::ServiceCaptureTimeoutNotAboveFlashFrameWait { service_ms, render_ms });
        }
        let screen_ms = u64::from(screen.capture_timeout_ms);
        if screen_ms <= service_ms {
            return Err(PhotoConfigError::ScreenCaptureTimeoutNotAboveFlashChain { screen_ms, service_ms });
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
