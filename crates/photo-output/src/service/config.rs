//! Конфиг службы записи снимков: лимиты очереди, сроки ожидания, качество, каталог в галерее.
//!
//! Все значения по умолчанию — константы с объяснением происхождения (`[estimate]` — оценка до замера
//! на телефоне, пересматривается по данным сессии 14). Ничего не зашито в логику службы.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Сколько снимков одновременно ждут сохранения (решение 140: 10 — как «пачка» быстрой серии,
/// после которой честнее отказать, чем копить 25 МБ DNG в памяти без предела).
pub const DEFAULT_MAX_PENDING_STILLS: usize = 10;
/// Сколько событий рендера может лежать в очереди службы. `[estimate]`: на снимок приходит 2–3
/// события, ждущих снимков не больше 10, значит 32 хватает с запасом; переполнение — явная ошибка.
pub const DEFAULT_EVENT_QUEUE_CAPACITY: usize = 32;
/// Срок от допуска до `Secured`/`NotSecured`. `[estimate]`: снимок без вспышки рендер берёт из кольца за
/// десятки мс, снимок со вспышкой (14d) ждёт предвспышку и разовый кадр — до срока рендера
/// `render.still.flash_frame_wait_ms` (1 800). 2,2 с — запас сверх него, после которого слот допуска
/// освобождается, а не висит вечно, если рендер молчит. Цепочка сроков (проверяет приложение):
/// камера (худшая последовательность вспышки 1 500) < рендер (1 800) < служба (2 200) < экран
/// (`camera_screen.screen.capture.capture_timeout_ms` 2 700): каждый следующий — страховка предыдущего.
pub const DEFAULT_CAPTURE_TIMEOUT_MS: u64 = 2_200;
/// Сколько аппаратных кодеров HEIF службе можно держать открытыми одновременно. `[estimate]`: рендер
/// проявляет снимки строго по одному, поэтому кодеру нужен один; второй — запас на перекрытие `finish`
/// прошлого снимка и открытия следующего. Больше — риск памяти и лимита одновременных кодеров
/// (`concurrent-instances`) на телефоне при серии из 10 ждущих снимков; пересмотреть по замеру 14c.
pub const DEFAULT_MAX_OPEN_ENCODERS: usize = 2;
/// Срок от `Secured` до итогового события рендера. `[estimate]`: проявление одного снимка 12 Мп
/// порциями между кадрами видоискателя — порядка 1–2 с; ждущих до 10, рендер берёт их по очереди,
/// поэтому последний ждёт до ~20 с. 30 с — запас, при котором срыв значит «рендер завис», а не «очередь».
/// Вместе с `heif_finish_timeout_ms` обязан быть КОРОЧЕ срока сохранения экрана (`saving_timeout_ms` 45 000).
pub const DEFAULT_DEVELOP_TIMEOUT_MS: u64 = 30_000;
/// Срок `finish` кодера HEIF. `[estimate]`: аппаратный кодер кодирует один кадр за сотни мс.
pub const DEFAULT_HEIF_FINISH_TIMEOUT_MS: u64 = 5_000;
/// Сколько `shutdown` ждёт рабочий поток, прежде чем отпустить его. `[estimate]`: поток может
/// быть внутри `finish` кодера, но дольше срока кодера ждать нет смысла.
pub const DEFAULT_SHUTDOWN_TIMEOUT_MS: u64 = 8_000;
/// Качество HEIF (0–100). `[estimate]`: 95 — «почти без потерь» для снимка, выбор владельца уточнит.
pub const DEFAULT_HEIF_QUALITY: u8 = 95;
/// Качество JPEG (0–100). `[estimate]`: как у HEIF.
pub const DEFAULT_JPEG_QUALITY: u8 = 95;
/// Каталог снимков в хранилище: стандартный каталог камеры Android.
pub const DEFAULT_MEDIA_DIRECTORY: &str = "DCIM/Camera";

/// Настройки службы записи снимков.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StillServiceConfig {
    /// Не больше стольких снимков одновременно ждут сохранения; следующее нажатие получит отказ.
    pub max_pending_stills: usize,
    /// Предел очереди событий рендера.
    pub event_queue_capacity: usize,
    /// Не больше стольких кодеров HEIF открыто одновременно: цель рендеру остальных выдаётся по мере
    /// освобождения места, по порядку `Secured`. JPEG кодера не требует и не ограничен.
    pub max_open_encoders: usize,
    /// Срок ожидания кадра после допуска, мс. Считается от прихода билета в службу.
    pub capture_timeout_ms: u64,
    /// Срок ожидания итога проявления после `Secured`, мс. Считается от прихода `Secured` в службу и
    /// включает ожидание места под кодер.
    pub develop_timeout_ms: u64,
    /// Срок `finish` кодера HEIF, мс.
    pub heif_finish_timeout_ms: u64,
    /// Срок ожидания остановки рабочего потока, мс.
    pub shutdown_timeout_ms: u64,
    pub heif_quality: u8,
    pub jpeg_quality: u8,
    /// Каталог в хранилище (относительно корня), например `DCIM/Camera`.
    pub media_directory: String,
}

impl Default for StillServiceConfig {
    fn default() -> Self {
        Self {
            max_pending_stills: DEFAULT_MAX_PENDING_STILLS,
            event_queue_capacity: DEFAULT_EVENT_QUEUE_CAPACITY,
            max_open_encoders: DEFAULT_MAX_OPEN_ENCODERS,
            capture_timeout_ms: DEFAULT_CAPTURE_TIMEOUT_MS,
            develop_timeout_ms: DEFAULT_DEVELOP_TIMEOUT_MS,
            heif_finish_timeout_ms: DEFAULT_HEIF_FINISH_TIMEOUT_MS,
            shutdown_timeout_ms: DEFAULT_SHUTDOWN_TIMEOUT_MS,
            heif_quality: DEFAULT_HEIF_QUALITY,
            jpeg_quality: DEFAULT_JPEG_QUALITY,
            media_directory: DEFAULT_MEDIA_DIRECTORY.to_owned(),
        }
    }
}

/// Что не так с конфигом.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("конфиг службы снимков: поле `{field}` {reason}")]
pub struct StillConfigError {
    pub field: &'static str,
    pub reason: &'static str,
}

impl StillServiceConfig {
    /// Проверяет, что службу можно запускать с этими значениями.
    pub fn validate(&self) -> Result<(), StillConfigError> {
        let fail = |field, reason| Err(StillConfigError { field, reason });
        if self.max_pending_stills < 1 {
            return fail("max_pending_stills", "должно быть не меньше 1");
        }
        if self.event_queue_capacity < 1 {
            return fail("event_queue_capacity", "должно быть не меньше 1");
        }
        if self.max_open_encoders < 1 {
            return fail("max_open_encoders", "должно быть не меньше 1");
        }
        for (field, value) in [
            ("capture_timeout_ms", self.capture_timeout_ms),
            ("develop_timeout_ms", self.develop_timeout_ms),
            ("heif_finish_timeout_ms", self.heif_finish_timeout_ms),
            ("shutdown_timeout_ms", self.shutdown_timeout_ms),
        ] {
            if value == 0 {
                return fail(field, "должно быть больше 0");
            }
        }
        for (field, quality) in [("heif_quality", self.heif_quality), ("jpeg_quality", self.jpeg_quality)] {
            if quality > 100 {
                return fail(field, "должно быть в диапазоне 0–100");
            }
        }
        if self.media_directory.trim().is_empty() {
            return fail("media_directory", "не должно быть пустым");
        }
        Ok(())
    }

    pub(super) fn capture_timeout(&self) -> Duration {
        Duration::from_millis(self.capture_timeout_ms)
    }

    pub(super) fn develop_timeout(&self) -> Duration {
        Duration::from_millis(self.develop_timeout_ms)
    }

    pub(super) fn heif_finish_timeout(&self) -> Duration {
        Duration::from_millis(self.heif_finish_timeout_ms)
    }

    pub(super) fn shutdown_timeout(&self) -> Duration {
        Duration::from_millis(self.shutdown_timeout_ms)
    }
}
