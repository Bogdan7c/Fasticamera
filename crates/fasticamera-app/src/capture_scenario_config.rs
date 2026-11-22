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
    /// Наименьшее число СОХРАНЁННЫХ снимков серии. Остальное — явный отказ (экран занят, очередь полна), он
    /// считается и показывается в отчёте. `[estimate]` 1: решение о том, сколько отказов терпимо при 100 мс,
    /// принимает владелец после первых цифр 14c.
    pub min_saved_shots: u32,
    /// Сколько кадров видоискателя может быть выброшено переполнением джиттер-буфера за серию (`pacing.jitter_overflow`).
    /// `[estimate]` 1: дрейф часов камеры против экрана даёт ≈ 1 кадр за оборот фазы (решение 53), снимок его не растит.
    pub max_jitter_overflow: u64,
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
            min_saved_shots: 1,
            max_jitter_overflow: 1,
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
        }
    }
}

/// Неверная секция `camera.scenarios.capture`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaptureScenarioConfigError {
    #[error("capture.{field} должно быть больше нуля")]
    Zero { field: &'static str },
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
            ("burst.min_saved_shots", u64::from(self.burst.min_saved_shots)),
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
        Ok(())
    }
}

#[cfg(test)]
#[path = "capture_scenario_config_tests.rs"]
mod tests;
