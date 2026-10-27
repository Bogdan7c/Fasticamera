//! Настройки подсказок производительности системе (ADPF, `APerformanceHint`, P13 сессии 07).
//!
//! Зачем: на OnePlus 11 GPU под DVFS «растягивает» кадр до ~13 мс из 16,7 — частоты держатся
//! впритык, и любая неровность кадра стоит пропущенного vsync. Поток рендера может создать сессию
//! ADPF для себя и после каждого показанного кадра сообщать системе фактические CPU- и GPU-времена;
//! система держит частоты так, чтобы кадр укладывался в целевую длительность.
//!
//! Цель = `target_fraction` × период экрана. Компромисс: меньшая доля — система считает, что мы
//! не успеваем, и держит частоты выше (кадры устойчивее, но больше мА); доля около 1 — частоты
//! «впритык» к периоду (экономно, но хрупко, как без ADPF). 0,75 — четверть периода запаса.
//!
//! По умолчанию ADPF выключен (поведение до P13): включает главный агент после A/B на телефоне.

use serde::{Deserialize, Serialize};

/// Наименьшая доля периода для цели ADPF: меньше — система держала бы максимальные частоты
/// почти всегда (ток!), и смысл подсказки «нужно ровно столько» терялся бы.
pub const MIN_ADPF_TARGET_FRACTION: f64 = 0.25;
/// Наибольшее ожидание времени GPU для отчёта, кадров.
pub const MAX_ADPF_GPU_WAIT_FRAMES: usize = 64;

/// Включён ли ADPF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdpfMode {
    /// Сессии нет, отчётов нет (как до P13). По умолчанию.
    #[default]
    Disabled,
    /// Поток рендера создаёт сессию на себя и сообщает о каждом показанном кадре.
    Enabled,
}

/// Секция `adpf` конфига потока рендера; отсутствующие поля — из [`Default`], неизвестные — ошибка.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct AdpfConfig {
    /// Режим на старте; дальше переключается командой `RenderInbox::adpf_command`.
    pub mode: AdpfMode,
    /// Цель длительности кадра как доля периода экрана, (0,25; 1]. См. описание модуля.
    pub target_fraction: f64,
    /// Сколько кадров ждать время кадра на GPU, прежде чем отчитаться только по CPU. Метки GPU
    /// приходят через 1–3 кадра (слот кольца собирается после своего fence); 6 — с запасом на
    /// неровный кадр. Бэкенд, который GPU не меряет, получает отчёты «только CPU» через столько кадров.
    pub gpu_report_wait_frames: usize,
}

impl Default for AdpfConfig {
    fn default() -> Self {
        Self {
            mode: AdpfMode::Disabled,
            target_fraction: 0.75,
            gpu_report_wait_frames: 6,
        }
    }
}

/// Что не так с [`AdpfConfig`].
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum AdpfConfigError {
    #[error("adpf.target_fraction должен лежать в [{MIN_ADPF_TARGET_FRACTION}, 1], а не {0}")]
    TargetFractionOutOfRange(f64),
    #[error("adpf.gpu_report_wait_frames должен лежать в [1, {MAX_ADPF_GPU_WAIT_FRAMES}], а не {0}")]
    GpuWaitOutOfRange(usize),
}

impl AdpfConfig {
    /// Проверяет значения; вызывается из `RenderLoopConfig::validate`.
    pub fn validate(&self) -> Result<(), AdpfConfigError> {
        // `contains` ложно и для NaN: NaN-доля отвергается.
        if !(MIN_ADPF_TARGET_FRACTION..=1.0).contains(&self.target_fraction) {
            return Err(AdpfConfigError::TargetFractionOutOfRange(self.target_fraction));
        }
        if !(1..=MAX_ADPF_GPU_WAIT_FRAMES).contains(&self.gpu_report_wait_frames) {
            return Err(AdpfConfigError::GpuWaitOutOfRange(self.gpu_report_wait_frames));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_keep_adpf_disabled() {
        let config = AdpfConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!(config.mode, AdpfMode::Disabled);
    }

    #[test]
    fn section_is_read_by_name_and_rejects_unknown_keys() {
        let config: AdpfConfig = serde_json::from_str(r#"{"mode": "enabled", "target_fraction": 0.6}"#).expect("json");
        assert_eq!((config.mode, config.target_fraction), (AdpfMode::Enabled, 0.6));
        assert_eq!(config.gpu_report_wait_frames, 6, "остальное — по умолчанию");
        assert!(serde_json::from_str::<AdpfConfig>(r#"{"mode": "on"}"#).is_err());
        assert!(serde_json::from_str::<AdpfConfig>(r#"{"target": 0.5}"#).is_err());
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        let with = |fraction: f64, wait: usize| AdpfConfig {
            target_fraction: fraction,
            gpu_report_wait_frames: wait,
            ..AdpfConfig::default()
        };
        assert_eq!(
            with(0.2, 6).validate(),
            Err(AdpfConfigError::TargetFractionOutOfRange(0.2))
        );
        assert!(with(1.01, 6).validate().is_err());
        assert!(with(f64::NAN, 6).validate().is_err());
        assert_eq!(with(1.0, 6).validate(), Ok(()), "граница 1 включена");
        assert_eq!(with(0.75, 0).validate(), Err(AdpfConfigError::GpuWaitOutOfRange(0)));
        assert_eq!(with(0.75, 65).validate(), Err(AdpfConfigError::GpuWaitOutOfRange(65)));
    }
}
