//! Секция `camera.scenarios.saturation`: параметры сценария `gpu-saturation-bench` (сессия 16a, P4b).
//!
//! Все умолчания живут здесь, в одном месте; проверка значений — [`SaturationScenarioConfig::validate`]
//! (неизвестное имя работы — ошибка конфига ДО запуска, а не провал посреди прогона).

use std::time::Duration;

use gpu_vulkan::saturation::{SaturationConfig, SaturationConfigError};
use gpu_vulkan::swapchain::FramesInFlight;
use serde::{Deserialize, Serialize};

use crate::saturation_bench::catalog::{CatalogEntry, UnknownWorkload, all_names, find_entry};

/// Секция `camera.scenarios.saturation`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct SaturationScenarioConfig {
    /// Имена работ каталога в порядке прогона. Умолчание — все.
    pub workloads: Vec<String>,
    /// Прогрев каждой работы, мс (кадры не считаются): GPU успевает разогнаться до рабочей частоты.
    pub warmup_ms: u64,
    /// Окно замера каждой работы, мс.
    pub measure_ms: u64,
    /// Шаг шкалы (к/с по шагам), мс: по ней видно разгон DVFS и троттлинг.
    pub timeline_step_ms: u64,
    /// Пауза между работами, мс (GPU простаивает). 0 — без паузы.
    pub cooldown_between_ms: u64,
    /// Кадров в полёте у стенда; каждая работа обязана иметь столько же слотов.
    pub frames_in_flight: usize,
    /// Сколько ждать закрытия камеры перед стартом (видоискатель на время замера молчит), мс.
    pub camera_close_timeout_ms: u64,
    /// Пауза после закрытия камеры перед «снимком до» активности рендера, мс: потоки доигрывают хвост.
    pub quiet_settle_ms: u64,
    /// Сколько кадров продуктовой работы GPU поток рендера вправе выполнить за время замера (ISP, перекомпоновка,
    /// UI, приборы вместе). Камера закрыта, экран снят — в норме 0; запас на кадр, что был в полёте в момент
    /// «снимка до», иначе цифра стенда не про работу, а про работу плюс продукт.
    pub render_gpu_work_tolerance_frames: u64,
    /// Запас времени на создание и освобождение каждой работы (V4 ≈ 360 МиБ, снимок ≈ 400 МиБ), мс: входит в
    /// предельный срок всего прогона (см. [`Self::job_deadline`]).
    pub setup_allowance_ms: u64,
}

impl Default for SaturationScenarioConfig {
    fn default() -> Self {
        Self {
            workloads: all_names(),
            warmup_ms: 3_000,
            measure_ms: 10_000,
            timeline_step_ms: 1_000,
            cooldown_between_ms: 3_000,
            frames_in_flight: 2,
            camera_close_timeout_ms: 10_000,
            quiet_settle_ms: 500,
            render_gpu_work_tolerance_frames: 5,
            setup_allowance_ms: 30_000,
        }
    }
}

/// Неверная секция `camera.scenarios.saturation`.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum SaturationScenarioConfigError {
    #[error("saturation.workloads пуст: нечего мерить")]
    NoWorkloads,
    #[error("saturation.workloads: работа {name:?} указана дважды")]
    DuplicateWorkload { name: String },
    #[error("saturation.workloads: {0}")]
    UnknownWorkload(#[from] UnknownWorkload),
    #[error("saturation.{field} должно быть больше нуля")]
    Zero { field: &'static str },
    #[error("saturation.frames_in_flight: {0}")]
    FramesInFlight(String),
    #[error("saturation: настройки стенда: {0}")]
    Bench(#[from] SaturationConfigError),
}

impl SaturationScenarioConfig {
    /// Проверяет значения; ничего не запускает.
    pub fn validate(&self) -> Result<(), SaturationScenarioConfigError> {
        self.resolve_workloads()?;
        for (field, value) in [
            ("warmup_ms", self.warmup_ms),
            ("measure_ms", self.measure_ms),
            ("timeline_step_ms", self.timeline_step_ms),
            ("camera_close_timeout_ms", self.camera_close_timeout_ms),
            ("setup_allowance_ms", self.setup_allowance_ms),
        ] {
            if value == 0 {
                return Err(SaturationScenarioConfigError::Zero { field });
            }
        }
        // Остальное (шаг не больше окна, число шагов шкалы) проверяет сам стенд.
        self.bench_config()?.validate()?;
        Ok(())
    }

    /// Строки каталога в порядке списка. Неизвестное имя и повтор — ошибка.
    pub fn resolve_workloads(&self) -> Result<Vec<&'static CatalogEntry>, SaturationScenarioConfigError> {
        if self.workloads.is_empty() {
            return Err(SaturationScenarioConfigError::NoWorkloads);
        }
        let mut entries: Vec<&'static CatalogEntry> = Vec::with_capacity(self.workloads.len());
        for name in &self.workloads {
            let entry = find_entry(name.trim())?;
            if entries.iter().any(|known| known.name == entry.name) {
                return Err(SaturationScenarioConfigError::DuplicateWorkload {
                    name: entry.name.to_owned(),
                });
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    /// Настройки стенда насыщения из секции.
    pub fn bench_config(&self) -> Result<SaturationConfig, SaturationScenarioConfigError> {
        let frames_in_flight = FramesInFlight::new(self.frames_in_flight)
            .map_err(|error| SaturationScenarioConfigError::FramesInFlight(error.to_string()))?;
        Ok(SaturationConfig {
            frames_in_flight,
            warmup: Duration::from_millis(self.warmup_ms),
            measure: Duration::from_millis(self.measure_ms),
            timeline_step: Duration::from_millis(self.timeline_step_ms),
            ..SaturationConfig::default()
        })
    }

    /// Предельный срок всего прогона: на каждую работу прогрев + окно + пауза + запас на создание, плюс запас
    /// на закрытие камеры. Прогон, не уложившийся в срок, отменяется (токен), а не висит.
    pub fn job_deadline(&self, workload_count: usize) -> Duration {
        let per_workload_ms = self
            .warmup_ms
            .saturating_add(self.measure_ms)
            .saturating_add(self.cooldown_between_ms)
            .saturating_add(self.setup_allowance_ms);
        let total_ms = per_workload_ms
            .saturating_mul(u64::try_from(workload_count).unwrap_or(u64::MAX))
            .saturating_add(self.camera_close_timeout_ms);
        Duration::from_millis(total_ms)
    }
}

#[cfg(test)]
#[path = "saturation_scenario_config_tests.rs"]
mod tests;
