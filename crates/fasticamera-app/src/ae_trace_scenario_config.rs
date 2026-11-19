//! Секция `camera.scenarios.ae_trace`: параметры smoke-сценария `ae-trace` (сессия 08c, шаги 5–6).
//!
//! Режим камеры и сроки запуска берутся из `camera.scenarios.viewfinder` (сценарий стартует продуктовый
//! видоискатель тем же запуском). Здесь — расписание фаз и подсказок владельцу, частота замеров,
//! пороги разбора ступенек и «дыхания», допуски детектора мерцания. Предел подтяжки ISP в EV здесь НЕ
//! хранится: его задаёт сам контроллер (`viewfinder.auto_exposure.controller.max_residual_ev`), сценарий
//! судит по нему, чтобы два числа не разошлись; тут только допуск округления поверх предела.

use serde::{Deserialize, Serialize};

/// Одна секунда в миллисекундах — для читаемости значений по умолчанию.
const SECOND_MS: u64 = 1_000;

/// Сколько подсказок («переключи лампу») в одной фазе помещается в строку результата (4000 байт):
/// каждая ступенька даёт в сводке около 90 байт. Больше — только подробностями, а их сводка не несёт.
pub const MAX_CUES_PER_PHASE: u64 = 6;

/// Параметры сценария `ae-trace`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AeTraceScenarioConfig {
    /// Длина фазы `photo` (профиль 3A «Фото»), мс.
    pub photo_phase_ms: u64,
    /// Длина фазы `video` (профиль 3A «Видео»), мс.
    pub video_phase_ms: u64,
    /// Длина фазы `static` (профиль «Фото», владелец ничего не трогает), мс.
    pub static_phase_ms: u64,
    /// Период подсказок в фазах `photo` и `video`, мс. Первая подсказка — через один период после начала фазы:
    /// до неё остаётся спокойный участок для базового уровня сцены.
    pub cue_interval_ms: u64,
    /// Сколько начала фазы `static` не разбирается (догорают последние ступеньки видео-фазы), мс.
    pub static_warmup_ms: u64,
    /// Не чаще чем раз в столько мс сценарий пишет отсчёт ряда (тик маршрутизатора — 50 мс).
    pub sample_interval_ms: u64,
    /// Окно оценки уровня сцены до подсказки и в конце отрезка, мс.
    pub step_window_ms: u64,
    /// Сколько отсчётов минимум должно быть в окне, чтобы по нему судить; меньше — «мало данных».
    pub min_window_samples: u64,
    /// Ступенька меньше этого (EV по сырому замеру сцены) — владелец подсказку пропустил (`no_step`).
    pub min_step_ev: f64,
    /// Полоса «установилось» вокруг конечного значения, EV. 0,1 EV — порог, который глаз только начинает видеть.
    pub settle_tolerance_ev: f64,
    /// Предельное перерегулирование на настоящей ступеньке, EV. Выше — нарушение.
    pub max_overshoot_ev: f64,
    /// Допуск поверх предела подтяжки ISP (EV): подтяжка квантуется шагом `residual_gain_step_ev`.
    pub gain_limit_epsilon_ev: f64,
    /// Период мерцания при сети 50 Гц (свет мерцает на 100 Гц), мс: выдержка, кратная ему, мерцание гасит.
    pub flicker_period_ms: f64,
    /// С какой выдержки кадр считается «длинным» для оценки кратности периоду, мс. Короче периода кратной не бывает.
    pub flicker_min_exposure_ms: f64,
    /// Допуск кратности периоду, мс: выдержка квантуется по строке сенсора, ровных 10 мс HAL не отдаёт.
    pub flicker_tolerance_ms: f64,
    /// Сколько ждать первой диагностики нашего AE после старта видоискателя, мс; нет — вендорский AE у руля.
    pub ae_diagnostics_timeout_ms: u64,
    /// Шаг прореживания ряда в подробностях (`detail_ae_trace`), мс.
    pub detail_decimation_ms: u64,
    /// Ёмкость ряда отсчётов. Расписание обязано в неё помещаться (проверяет `validate`): ряд не растёт без предела.
    pub series_capacity: usize,
}

impl Default for AeTraceScenarioConfig {
    /// Значения шага 5 плана 08c: фазы по 36 с с подсказкой каждые 6 с (на ступеньку остаётся 6 с, а видео-профилю
    /// с τ = 0,8 с хватает на восемь постоянных времени), статика 20 с. Окно 1 с — 20 отсчётов при 50 мс.
    fn default() -> Self {
        Self {
            photo_phase_ms: 36 * SECOND_MS,
            video_phase_ms: 36 * SECOND_MS,
            static_phase_ms: 20 * SECOND_MS,
            cue_interval_ms: 6 * SECOND_MS,
            static_warmup_ms: 4 * SECOND_MS,
            sample_interval_ms: 50,
            step_window_ms: SECOND_MS,
            min_window_samples: 5,
            min_step_ev: 0.5,
            settle_tolerance_ev: 0.1,
            max_overshoot_ev: 0.15,
            gain_limit_epsilon_ev: 0.02,
            flicker_period_ms: 10.0,
            flicker_min_exposure_ms: 10.0,
            flicker_tolerance_ms: 0.05,
            ae_diagnostics_timeout_ms: 5 * SECOND_MS,
            detail_decimation_ms: 250,
            series_capacity: 4_096,
        }
    }
}

/// Неверная секция `camera.scenarios.ae_trace`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AeTraceScenarioConfigError {
    #[error("ae_trace.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("ae_trace.{field} = {value}: нужно конечное число больше нуля")]
    NotPositive { field: &'static str, value: f64 },
    #[error("ae_trace.cue_interval_ms ({interval_ms}) должен быть больше двух окон step_window_ms ({window_ms})")]
    CueIntervalTooShort { interval_ms: u64, window_ms: u64 },
    #[error(
        "ae_trace.{field} ({phase_ms} мс) короче периода подсказок {interval_ms} мс: в фазе нет ни одной ступеньки"
    )]
    PhaseShorterThanCue {
        field: &'static str,
        phase_ms: u64,
        interval_ms: u64,
    },
    #[error("ae_trace.{field} ({phase_ms} мс): подсказок больше {limit} — сводка не поместится в строку результата")]
    TooManyCues {
        field: &'static str,
        phase_ms: u64,
        limit: u64,
    },
    #[error("ae_trace.step_window_ms ({window_ms}) вмещает меньше {needed} отсчётов при шаге {sample_ms} мс")]
    WindowTooThin {
        window_ms: u64,
        needed: u64,
        sample_ms: u64,
    },
    #[error("ae_trace.static_warmup_ms ({warmup_ms}) не меньше static_phase_ms ({phase_ms}): разбирать нечего")]
    WarmupSwallowsStatic { warmup_ms: u64, phase_ms: u64 },
    #[error("ae_trace.series_capacity = {capacity}: расписанию нужно {needed} отсчётов")]
    CapacityTooSmall { capacity: usize, needed: u64 },
}

impl AeTraceScenarioConfig {
    /// Полная длина расписания, мс.
    pub fn total_ms(&self) -> u64 {
        self.photo_phase_ms
            .saturating_add(self.video_phase_ms)
            .saturating_add(self.static_phase_ms)
    }

    /// Проверяет секцию целиком. Каждое правило — отдельная ошибка с именем поля.
    pub fn validate(&self) -> Result<(), AeTraceScenarioConfigError> {
        self.validate_positive()?;
        self.validate_schedule()?;
        self.validate_series()
    }

    /// Нули и нечисла: всё, что делится на значение или сравнивается с ним, обязано быть конечным и > 0.
    fn validate_positive(&self) -> Result<(), AeTraceScenarioConfigError> {
        for (field, value) in [
            ("photo_phase_ms", self.photo_phase_ms),
            ("video_phase_ms", self.video_phase_ms),
            ("static_phase_ms", self.static_phase_ms),
            ("cue_interval_ms", self.cue_interval_ms),
            ("sample_interval_ms", self.sample_interval_ms),
            ("step_window_ms", self.step_window_ms),
            ("min_window_samples", self.min_window_samples),
            ("ae_diagnostics_timeout_ms", self.ae_diagnostics_timeout_ms),
            ("detail_decimation_ms", self.detail_decimation_ms),
        ] {
            if value == 0 {
                return Err(AeTraceScenarioConfigError::Zero { field });
            }
        }
        for (field, value) in [
            ("min_step_ev", self.min_step_ev),
            ("settle_tolerance_ev", self.settle_tolerance_ev),
            ("max_overshoot_ev", self.max_overshoot_ev),
            ("gain_limit_epsilon_ev", self.gain_limit_epsilon_ev),
            ("flicker_period_ms", self.flicker_period_ms),
            ("flicker_min_exposure_ms", self.flicker_min_exposure_ms),
            ("flicker_tolerance_ms", self.flicker_tolerance_ms),
        ] {
            // Через сравнение: NaN не проходит.
            if !(value.is_finite() && value > 0.0) {
                return Err(AeTraceScenarioConfigError::NotPositive { field, value });
            }
        }
        Ok(())
    }

    /// Согласованность расписания: подсказки помещаются в фазы, окна — в отрезки между подсказками.
    fn validate_schedule(&self) -> Result<(), AeTraceScenarioConfigError> {
        if self.cue_interval_ms <= self.step_window_ms.saturating_mul(2) {
            return Err(AeTraceScenarioConfigError::CueIntervalTooShort {
                interval_ms: self.cue_interval_ms,
                window_ms: self.step_window_ms,
            });
        }
        for (field, phase_ms) in [
            ("photo_phase_ms", self.photo_phase_ms),
            ("video_phase_ms", self.video_phase_ms),
        ] {
            if phase_ms < self.cue_interval_ms {
                return Err(AeTraceScenarioConfigError::PhaseShorterThanCue {
                    field,
                    phase_ms,
                    interval_ms: self.cue_interval_ms,
                });
            }
            if phase_ms / self.cue_interval_ms > MAX_CUES_PER_PHASE {
                return Err(AeTraceScenarioConfigError::TooManyCues {
                    field,
                    phase_ms,
                    limit: MAX_CUES_PER_PHASE,
                });
            }
        }
        if self.static_warmup_ms >= self.static_phase_ms {
            return Err(AeTraceScenarioConfigError::WarmupSwallowsStatic {
                warmup_ms: self.static_warmup_ms,
                phase_ms: self.static_phase_ms,
            });
        }
        Ok(())
    }

    /// Ряд: окно вмещает нужное число отсчётов, а вся запись — в ёмкость.
    fn validate_series(&self) -> Result<(), AeTraceScenarioConfigError> {
        let in_window = self.step_window_ms / self.sample_interval_ms;
        if in_window < self.min_window_samples {
            return Err(AeTraceScenarioConfigError::WindowTooThin {
                window_ms: self.step_window_ms,
                needed: self.min_window_samples,
                sample_ms: self.sample_interval_ms,
            });
        }
        let needed = self.total_ms() / self.sample_interval_ms + 1;
        if (self.series_capacity as u64) < needed {
            return Err(AeTraceScenarioConfigError::CapacityTooSmall {
                capacity: self.series_capacity,
                needed,
            });
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "ae_trace_scenario_config_tests.rs"]
mod tests;
