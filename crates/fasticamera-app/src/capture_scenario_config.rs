//! Секция `camera.scenarios.capture`: параметры smoke-сценария `capture` (сессия 14) — снимок на настоящем экране
//! камеры: один HEIF, HEIF + DNG и серия быстрых нажатий.
//!
//! Сценарий нажимает затвор действиями экрана и судит по итогам службы записи (адреса файлов, время фаз), а не
//! по самому факту вызова. Числа сроков — `[estimate]`: настоящие времена снимка замеряет 14c на телефоне.

use serde::{Deserialize, Serialize};

/// Размер полного кадра сенсора основной камеры, по которому скрипт ПК сверяет размеры файлов 4:3 при 1×.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FullFrameSize {
    /// `[doc]` факты сессии 00: RAW основной камеры 4096 × 3072 (4:3).
    pub width_px: u32,
    pub height_px: u32,
}

impl Default for FullFrameSize {
    fn default() -> Self {
        Self {
            width_px: 4_096,
            height_px: 3_072,
        }
    }
}

/// Серия быстрых нажатий.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BurstPlan {
    /// Сколько нажатий в серии. `[doc]` план сессии 14: 10 (и решение 140: ждать сохранения могут до 10).
    pub presses: u32,
    /// Наименьший интервал между нажатиями, мс. `[estimate]` 100 — быстрый палец. Реальный шаг не меньше этого и
    /// кратен тику маршрутизатора (50 мс): отчёт показывает фактические интервалы.
    pub interval_ms: u64,
    /// Сколько ждать итогов всех принятых снимков серии после последнего нажатия, мс. `[estimate]` 30 000.
    pub drain_timeout_ms: u64,
    /// Сколько нажатий серии может быть отклонено явным отказом (затвор занят `ShutterBusy` или набор сохраняемых
    /// полон `SavingQueueFull`; отказ службы считается так же). Все ПРИНЯТЫЕ нажатия обязаны быть сохранены — это
    /// проверяется отдельно и порогом не смягчается. `[estimate]` 3: при шаге 100 мс затвор занят до «кадр
    /// захвачен» (десятки мс), а набор сохраняемых вмещает 10 (решение 140), поэтому лишний отказ — редкость;
    /// владелец уточнит число после первых цифр 14c.
    pub max_refused_presses: u32,
    /// Наибольшая доля кадров видоискателя, выброшенных переполнением джиттер-буфера за серию:
    /// `выброшено / (выброшено + показано)`. `[measured]` 0,15 (решение 147): на горячем телефоне (решение 145) пока
    /// снимки проявляются и пишутся в HEIF, теряется 4–13 % кадров — нагрузка всей системы (кодер, память, запись),
    /// не блокировки потока рендера (их снял 14c, решение 144); без снимков — 1 кадр из 3603. Порог ловит возврат
    /// блокировок (до мастерской 14c доля была 20–26 %), а цену системы разбирают 16a/17; после мастерской наблюдалось
    /// до 13,5 % — изредка порог может краснеть, это сигнал смотреть цифры, а не повод поднять его молча.
    pub max_jitter_overflow_share: f64,
    /// Сколько раз пул кадров камеры может исчерпаться за серию (`dropped_pool_exhausted`). Любое исчерпание —
    /// ошибка расчёта пула. `[doc]` 0.
    pub max_pool_exhausted: u64,
}

impl Default for BurstPlan {
    fn default() -> Self {
        Self {
            presses: 10,
            interval_ms: 100,
            drain_timeout_ms: 30_000,
            max_refused_presses: 3,
            max_jitter_overflow_share: 0.15,
            max_pool_exhausted: 0,
        }
    }
}

/// Секция `camera.scenarios.capture`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct CaptureScenarioConfig {
    /// Сколько ждать готовности: настоящий сервис снимка, живой кадр активной камеры на экране. `[estimate]` 20 000.
    pub ready_timeout_ms: u64,
    /// Выдержка после первого живого кадра, мс: AE в начале потока даёт почти чёрные кадры (как у остальных
    /// сценариев с экраном). `[estimate]` 2 000.
    pub viewfinder_settle_ms: u64,
    /// Сколько ждать «кадр захвачен» после нажатия, мс. `[derived]` 4 000 = срок захвата экрана
    /// (`camera_screen.capture.capture_timeout_ms` 2 000) с запасом на тик.
    pub secured_timeout_ms: u64,
    /// Сколько ждать «сохранено» после нажатия, мс. `[derived]` 50 000 > срок сохранения экрана
    /// (`saving_timeout_ms` 45 000): сначала должен сработать срок службы, а не сценария.
    pub saved_timeout_ms: u64,
    /// Сколько ждать, пока значок RAW переключит настройку экрана, мс. `[estimate]` 3 000.
    pub raw_toggle_timeout_ms: u64,
    /// Пауза между шагами, мс: прошлый снимок уходит из набора сохраняемых, видоискатель успокаивается. `[estimate]` 500.
    pub pause_between_steps_ms: u64,
    pub burst: BurstPlan,
    /// Полный кадр основной камеры (для сверки размеров файлов на ПК).
    pub full_frame: FullFrameSize,
    /// Наименьшая доля снимков, у которых взят РОВНО показанный кадр (`FrameChoice::ShownAtPress`), от снимков с
    /// известным выбором. `[estimate]` 0,9: на телефоне записи показа драйвера есть всегда, но редкий снимок может
    /// уйти на оценку (`PresentationTimingUnknown`); 1 из 12 допустим, больше — признак неработающего доказательства.
    pub min_exact_frame_share: f64,
    /// Наибольшая задержка «нажатие → начало экспозиции выбранного кадра», мс. `[estimate]` 150: кадр виден на экране
    /// через 67–90 мс после начала выдержки (задержка «сенсор → дисплей», сценарии `raw-stream`/`viewfinder`), нажатие
    /// сценария приходится на момент показа кадра, живой палец добавляет до периода кадра камеры (≈ 17–33 мс);
    /// 90 + 33 + запас на тик и джиттер часов ≈ 150. Это порог «без заметной задержки затвора», а не рекорд.
    pub max_press_to_frame_ms: f64,
}

impl Default for CaptureScenarioConfig {
    fn default() -> Self {
        Self {
            ready_timeout_ms: 20_000,
            viewfinder_settle_ms: 2_000,
            secured_timeout_ms: 4_000,
            saved_timeout_ms: 50_000,
            raw_toggle_timeout_ms: 3_000,
            pause_between_steps_ms: 500,
            burst: BurstPlan::default(),
            full_frame: FullFrameSize::default(),
            min_exact_frame_share: 0.9,
            max_press_to_frame_ms: 150.0,
        }
    }
}

/// Неверная секция `camera.scenarios.capture`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CaptureScenarioConfigError {
    #[error("capture.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("capture.{field} = {value} вне допустимого диапазона: {expected}")]
    OutOfRange {
        field: &'static str,
        value: f64,
        expected: &'static str,
    },
    #[error("capture.burst.max_refused_presses ({refused}) должно быть меньше числа нажатий ({presses})")]
    RefusalsCoverWholeBurst { refused: u32, presses: u32 },
    #[error("capture.secured_timeout_ms ({secured}) не должно быть больше saved_timeout_ms ({saved})")]
    SecuredAfterSaved { secured: u64, saved: u64 },
}

impl CaptureScenarioConfig {
    pub fn validate(&self) -> Result<(), CaptureScenarioConfigError> {
        for (field, value) in [
            ("ready_timeout_ms", self.ready_timeout_ms),
            ("viewfinder_settle_ms", self.viewfinder_settle_ms),
            ("secured_timeout_ms", self.secured_timeout_ms),
            ("saved_timeout_ms", self.saved_timeout_ms),
            ("raw_toggle_timeout_ms", self.raw_toggle_timeout_ms),
            ("burst.presses", u64::from(self.burst.presses)),
            ("burst.interval_ms", self.burst.interval_ms),
            ("burst.drain_timeout_ms", self.burst.drain_timeout_ms),
            ("full_frame.width_px", u64::from(self.full_frame.width_px)),
            ("full_frame.height_px", u64::from(self.full_frame.height_px)),
        ] {
            if value == 0 {
                return Err(CaptureScenarioConfigError::Zero { field });
            }
        }
        if self.secured_timeout_ms > self.saved_timeout_ms {
            return Err(CaptureScenarioConfigError::SecuredAfterSaved {
                secured: self.secured_timeout_ms,
                saved: self.saved_timeout_ms,
            });
        }
        if !(self.min_exact_frame_share.is_finite() && (0.0..=1.0).contains(&self.min_exact_frame_share)) {
            return Err(CaptureScenarioConfigError::OutOfRange {
                field: "min_exact_frame_share",
                value: self.min_exact_frame_share,
                expected: "число от 0 до 1",
            });
        }
        if !(self.burst.max_jitter_overflow_share.is_finite()
            && (0.0..=1.0).contains(&self.burst.max_jitter_overflow_share))
        {
            return Err(CaptureScenarioConfigError::OutOfRange {
                field: "burst.max_jitter_overflow_share",
                value: self.burst.max_jitter_overflow_share,
                expected: "число от 0 до 1",
            });
        }
        if !(self.max_press_to_frame_ms.is_finite() && self.max_press_to_frame_ms > 0.0) {
            return Err(CaptureScenarioConfigError::OutOfRange {
                field: "max_press_to_frame_ms",
                value: self.max_press_to_frame_ms,
                expected: "конечное число больше 0",
            });
        }
        // Если отказать можно все нажатия, серия, не сохранившая ни одного снимка, прошла бы проверку.
        if self.burst.max_refused_presses >= self.burst.presses {
            return Err(CaptureScenarioConfigError::RefusalsCoverWholeBurst {
                refused: self.burst.max_refused_presses,
                presses: self.burst.presses,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "capture_scenario_config_tests.rs"]
mod tests;
