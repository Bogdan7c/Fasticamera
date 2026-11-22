//! Конфиг вспышки движка (сессия 14d, решения 158, 159). Числа — с меткой происхождения:
//! `[measured]` — зонд `flash` 14d на OnePlus 11, `[estimate]` — оценка агента, подбирается на
//! телефоне.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Как движок снимает кадр со вспышкой.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct FlashConfig {
    /// Во сколько раз разовая вспышка ярче фонарика при равных выдержке и ISO.
    /// `[measured]` 1,87 (камеры 2 и 3, 16 и 4 мс; силы у обеих нет — один уровень).
    pub torch_to_flash_ratio: f64,
    /// Auto вспыхивает, если сцене без вспышки нужно не меньше этой «выдержки × ISO» (log2 с·ISO)
    /// для среднего серого (решение 159). `[estimate]` 5,5 ≈ 1/60 с × ISO 2700; предел нашего AE
    /// на 60 к/с — 2^6,74 (1/60 × 6400).
    pub auto_fire_min_mid_gray_log2_seconds_iso: f64,
    /// Наибольшая «выдержка × ISO» кадров предвспышки (log2 с·ISO): фонарик на близком объекте
    /// при экспозиции тёмной сцены пересветил бы замер. `[estimate]` 3,7 ≈ 16 мс × ISO 800 —
    /// при ней в тёмной комнате зонда фонарик добавлял ≈ 12 DN из 1023, до пересвета далеко.
    pub preflash_max_log2_seconds_iso: f64,
    /// На сколько EV уменьшить экспозицию предвспышки, если замер с фонариком пересвечен.
    /// `[estimate]` 3 EV.
    pub preflash_remeasure_step_ev: f64,
    /// Сколько раз можно перемерить пересвеченную предвспышку. `[estimate]` 1: каждый повтор —
    /// ещё ≈ 8 кадров до нового освещённого кадра.
    pub max_preflash_remeasures: u32,
    /// Срок на годный замер с фонариком от отправки запроса с фонариком, мс. `[estimate]` 500:
    /// фонарик загорается через 7–8 кадров (результат ≈ 130 мс `[measured]`), замер — ещё
    /// 3–8 кадров. Каждый перемер (`max_preflash_remeasures`) ПРОДЛЕВАЕТ срок ещё на столько же:
    /// после перестройки запроса снова 7–8 кадров до освещённого кадра (запас на 30 к/с).
    pub preflash_timeout_ms: u64,
    /// Пауза между выключением фонарика и разовым запросом, мс. `[measured]`: без паузы вспышка
    /// не горит (1 из 6), с 50/100/200 мс горит 18 из 18; меньше 50 не мерилось. 100 — середина.
    pub torch_off_to_single_gap_ms: u64,
    /// Насколько старым может быть замер без фонарика относительно замера с фонариком, мс.
    /// `[estimate]` 1000: сцена за секунду почти не меняется, а замер без фонарика берётся до
    /// того, как фонарик загорелся (7–8 кадров, ≈ 130 мс); старее — относится к другой сцене.
    pub ambient_sample_max_age_ms: u64,
    /// Срок на результат разового запроса со вспышкой от его отправки, мс. `[estimate]` 400:
    /// `[measured]` результат через 120–142 мс.
    pub result_timeout_ms: u64,
}

impl Default for FlashConfig {
    fn default() -> Self {
        Self {
            torch_to_flash_ratio: 1.87,
            auto_fire_min_mid_gray_log2_seconds_iso: 5.5,
            preflash_max_log2_seconds_iso: 3.7,
            preflash_remeasure_step_ev: 3.0,
            max_preflash_remeasures: 1,
            preflash_timeout_ms: 500,
            torch_off_to_single_gap_ms: 100,
            ambient_sample_max_age_ms: 1_000,
            result_timeout_ms: 400,
        }
    }
}

/// Неверное значение конфига вспышки.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum FlashConfigError {
    #[error("three_a.flash.{field} = {value}: ожидается {expected}")]
    OutOfRange {
        field: &'static str,
        value: f64,
        expected: &'static str,
    },
}

impl FlashConfig {
    /// Проверить границы. Конфиг фиксирован на процесс, поэтому проверка — при старте.
    pub fn validate(&self) -> Result<(), FlashConfigError> {
        check(
            "torch_to_flash_ratio",
            self.torch_to_flash_ratio,
            1.0..=20.0,
            "1…20 (вспышка не слабее фонарика)",
        )?;
        check(
            "auto_fire_min_mid_gray_log2_seconds_iso",
            self.auto_fire_min_mid_gray_log2_seconds_iso,
            -10.0..=20.0,
            "−10…20 (log2 с·ISO)",
        )?;
        check(
            "preflash_max_log2_seconds_iso",
            self.preflash_max_log2_seconds_iso,
            -10.0..=20.0,
            "−10…20 (log2 с·ISO)",
        )?;
        check(
            "preflash_remeasure_step_ev",
            self.preflash_remeasure_step_ev,
            0.5..=10.0,
            "0,5…10 EV",
        )?;
        check(
            "max_preflash_remeasures",
            f64::from(self.max_preflash_remeasures),
            0.0..=3.0,
            "0…3",
        )?;
        check_ms("preflash_timeout_ms", self.preflash_timeout_ms, 100..=5_000)?;
        check_ms(
            "torch_off_to_single_gap_ms",
            self.torch_off_to_single_gap_ms,
            50..=1_000,
        )?;
        check_ms("ambient_sample_max_age_ms", self.ambient_sample_max_age_ms, 100..=5_000)?;
        check_ms("result_timeout_ms", self.result_timeout_ms, 100..=5_000)?;
        Ok(())
    }

    /// Худший срок всей последовательности от принятия команды до итога, мс: предвспышка (срок и по
    /// одному продлению на каждый возможный перемер), пауза и результат. Его проверяет приложение против срока рендера на кадр со вспышкой
    /// (правило сроков «экран > служба > рендер > камера»).
    pub fn worst_case_sequence_ms(&self) -> u64 {
        let preflash_extensions = u64::from(self.max_preflash_remeasures).saturating_add(1);
        self.preflash_timeout_ms
            .saturating_mul(preflash_extensions)
            .saturating_add(self.torch_off_to_single_gap_ms)
            .saturating_add(self.result_timeout_ms)
    }
}

fn check(
    field: &'static str,
    value: f64,
    range: std::ops::RangeInclusive<f64>,
    expected: &'static str,
) -> Result<(), FlashConfigError> {
    if value.is_finite() && range.contains(&value) {
        Ok(())
    } else {
        Err(FlashConfigError::OutOfRange { field, value, expected })
    }
}

fn check_ms(field: &'static str, value: u64, range: std::ops::RangeInclusive<u64>) -> Result<(), FlashConfigError> {
    if range.contains(&value) {
        Ok(())
    } else {
        // Числа мс малы, перевод в f64 для текста ошибки точен.
        #[expect(clippy::cast_precision_loss, reason = "мс конфига ≪ 2^52, только для текста ошибки")]
        let shown = value as f64;
        Err(FlashConfigError::OutOfRange {
            field,
            value: shown,
            expected: "мс в разрешённых пределах (см. документацию поля)",
        })
    }
}

#[cfg(test)]
#[path = "flash_config_tests.rs"]
mod tests;
