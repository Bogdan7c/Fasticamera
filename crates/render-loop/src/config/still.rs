//! Секция `still`: как рендер выбирает кадр для снимка и проявляет его между кадрами видоискателя
//! (сессия 14, решения 139, 140, 142).
//!
//! Числа с пометкой `[estimate]` — оценки до замера на телефоне (14c), «подгонка, не Apple».

use isp_vulkan::photo::{PhotoConfig, PhotoError};
use serde::{Deserialize, Serialize};

/// Наибольший срок любого ожидания, мс (минута): больше — почти наверняка ошибка единиц.
pub const MAX_STILL_WAIT_MS: u32 = 60_000;
/// Наибольшая ёмкость очереди снимков: предел «разумной памяти» (один ждущий снимок ≈ 45 МБ, решение 140).
pub const MAX_STILL_QUEUE_CAPACITY: u32 = 64;
/// Наибольшее число полос на один показанный кадр: больше — это уже «весь снимок в один кадр».
pub const MAX_SLICES_PER_FRAME: u32 = 64;
/// Наибольшая ёмкость журналов кадров (показы, параметры): больше — расход памяти без пользы.
pub const MAX_STILL_LOG_CAPACITY: u32 = 4_096;

/// Секция `still` конфига потока рендера.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct StillDevelopConfig {
    /// Сколько снимков рендер ведёт одновременно (от нажатия до итога). Решение 140: 10. Сверх очереди рендер
    /// отвечает отказом (`NotSecured`), уже принятое не теряется.
    pub queue_capacity: u32,
    /// Сколько ждать записей показа драйвера, чтобы доказать, какой кадр был на экране в момент нажатия.
    /// Записи приходят на 1–3 кадра позже показа (60 Гц: до ~50 мс); не дождались — кадр по оценке с пометкой
    /// `PresentationTimingUnknown`. `[estimate]`: 80.
    pub presentation_wait_ms: u32,
    /// Сколько ждать от записи цель кодера после `Secured` (решение 140: снимок ждёт сохранения). Истекло —
    /// `Failed(TargetTimedOut)`, снимок отпущен. Запись отдаёт цель не сразу, а когда освободится место под кодер
    /// (`photo.service.max_open_encoders`), поэтому последний из `queue_capacity` ждущих снимков ждёт цель, пока
    /// проявятся и завершатся все впереди: ≈ (10 − 1) × 2 с = 18 с (2 с на снимок впереди: проявление ≈ 0,5 с на телефоне, до 1–2 с, плюс `finish`). `[estimate]`:
    /// 20 000 (18 с и запас); короче срока записи на проявление (`photo.service.develop_timeout_ms` 30 000):
    /// иначе запись сорвала бы снимок раньше, чем рендер перестал бы ждать цель.
    pub target_timeout_ms: u32,
    /// Сколько ждать копию RAW (один короткий compute-проход по 24 МиБ, на ПК и телефоне — единицы мс). Истекло —
    /// GPU снимков объявлен потерянным (fence завис), снимок получает `NotSecured(CaptureFailed)`, лизинг кадра
    /// камере НЕ возвращается (GPU мог ещё читать; учтённая утечка, решение 43). `[estimate]`: 2 000 (запас на
    /// порядки против нормы и против DVFS, но зависший fence не держит камеру дольше пары секунд).
    pub capture_timeout_ms: u32,
    /// Сколько ждать проявление целиком (от первой полосы до готовой картинки): полосы идут по `slices_per_frame`
    /// между кадрами видоискателя, т. е. длительность ≈ число полос × период кадра + GPU. Истекло — GPU потерян,
    /// снимок получает `Failed(Gpu)`. `[estimate]`: 30 000 (до замера на телефоне, 14c; заведомо больше любой
    /// здоровой проявки, но конечно).
    pub develop_timeout_ms: u32,
    /// Сколько полос проявления может быть в полёте у GPU одновременно. Слоты одноразовой очереди резервируются
    /// под это число + копию RAW + показ в кодер: проявление не может занять слот, нужный захвату второго затвора.
    /// `[estimate]`: 4 (при одной полосе на кадр и fence-е за 1–2 кадра в полёте обычно 1–2).
    pub max_slices_in_flight: u32,
    /// Сколько целей кодера, чей показ нельзя отследить fence-ом (устройство без `VK_EXT_swapchain_maintenance1`),
    /// держать до остановки GPU снимков. Сверх числа самая старая остаётся в учтённой утечке. `[estimate]`: 2.
    pub retired_present_capacity: u32,
    /// Сколько ждать завершения показа в поверхность кодера (GPU-копия и present). Истекло — `Failed(Gpu)`.
    /// `[estimate]`: 5 000.
    pub encoder_present_timeout_ms: u32,
    /// Сколько полос проявления отправлять на один показанный кадр видоискателя (решение 137: нарезка GPU).
    /// 1 — самое бережное к видоискателю; больше — снимок готов быстрее, но кадр видоискателя может опоздать.
    pub slices_per_frame: u32,
    /// Сколько последних записей «параметры кадра» помнить (белая точка и подтяжка `k` показанных кадров).
    /// Покрывает кольцо снимков (12 кадров) с запасом. `[estimate]`: 48.
    pub frame_params_capacity: u32,
    /// Сколько последних показов помнить (что было на экране под каждым `present_id`). Как минимум на время
    /// ожидания записей показа при 120 Гц с запасом. `[estimate]`: 256.
    pub present_log_capacity: u32,
    /// Проявление: высота полосы, рабочая группа (`isp_vulkan::photo`).
    pub photo: PhotoConfig,
}

impl Default for StillDevelopConfig {
    fn default() -> Self {
        Self {
            queue_capacity: 10,
            presentation_wait_ms: 80,
            target_timeout_ms: 20_000,
            capture_timeout_ms: 2_000,
            develop_timeout_ms: 30_000,
            max_slices_in_flight: 4,
            retired_present_capacity: 2,
            encoder_present_timeout_ms: 5_000,
            slices_per_frame: 1,
            frame_params_capacity: 48,
            present_log_capacity: 256,
            photo: PhotoConfig::default(),
        }
    }
}

/// Что не так с секцией `still`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum StillConfigError {
    /// Целое поле вне допустимого диапазона.
    #[error("still.{field} должен лежать в {min}..={max}, а не {value}")]
    OutOfRange {
        /// Имя поля.
        field: &'static str,
        /// Значение.
        value: u32,
        /// Наименьшее допустимое.
        min: u32,
        /// Наибольшее допустимое.
        max: u32,
    },
    /// Настройки проявления отвергнуты.
    #[error("still.photo: {0}")]
    Photo(String),
}

impl StillDevelopConfig {
    /// Проверяет секцию до создания объектов GPU.
    pub fn validate(&self) -> Result<(), StillConfigError> {
        let ranges: [(&'static str, u32, u32, u32); 10] = [
            ("queue_capacity", self.queue_capacity, 1, MAX_STILL_QUEUE_CAPACITY),
            ("presentation_wait_ms", self.presentation_wait_ms, 1, MAX_STILL_WAIT_MS),
            ("target_timeout_ms", self.target_timeout_ms, 1, MAX_STILL_WAIT_MS),
            (
                "encoder_present_timeout_ms",
                self.encoder_present_timeout_ms,
                1,
                MAX_STILL_WAIT_MS,
            ),
            ("capture_timeout_ms", self.capture_timeout_ms, 1, MAX_STILL_WAIT_MS),
            ("develop_timeout_ms", self.develop_timeout_ms, 1, MAX_STILL_WAIT_MS),
            (
                "max_slices_in_flight",
                self.max_slices_in_flight,
                1,
                MAX_SLICES_PER_FRAME,
            ),
            (
                "retired_present_capacity",
                self.retired_present_capacity,
                1,
                MAX_STILL_QUEUE_CAPACITY,
            ),
            ("slices_per_frame", self.slices_per_frame, 1, MAX_SLICES_PER_FRAME),
            (
                "frame_params_capacity",
                self.frame_params_capacity,
                1,
                MAX_STILL_LOG_CAPACITY,
            ),
        ];
        for (field, value, min, max) in ranges {
            if value < min || value > max {
                return Err(StillConfigError::OutOfRange { field, value, min, max });
            }
        }
        if self.present_log_capacity == 0 || self.present_log_capacity > MAX_STILL_LOG_CAPACITY {
            return Err(StillConfigError::OutOfRange {
                field: "present_log_capacity",
                value: self.present_log_capacity,
                min: 1,
                max: MAX_STILL_LOG_CAPACITY,
            });
        }
        self.photo
            .validate()
            .map_err(|error: PhotoError| StillConfigError::Photo(error.to_string()))
    }
}

#[cfg(test)]
#[path = "still_tests.rs"]
mod tests;
