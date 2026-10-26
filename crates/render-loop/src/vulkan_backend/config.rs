//! Настройки Vulkan-бэкенда показа. Всё, что можно подкрутить, — поле, а не константа в логике.

use std::time::Duration;

use gpu_vulkan::error::GpuError;
use gpu_vulkan::swapchain::{ColorDepthPreference, DisplayTargetConfig, FramesInFlight};
use gpu_vulkan::validation::ValidationMode;

use super::present_time::PresentTimePolicy;
use crate::config::RenderLoopConfig;

/// Параметры [`super::VulkanDisplayBackend`].
#[derive(Debug, Clone)]
pub struct VulkanBackendConfig {
    /// Режим validation-слоя (в релизе `Disabled`, в debug и на ПК-стенде — `FULL`).
    pub validation: ValidationMode,
    /// Сколько кадров одновременно в полёте (из `RenderLoopConfig::frames_in_flight`).
    pub frames_in_flight: FramesInFlight,
    /// Сколько ждать картинку swapchain (из `RenderLoopConfig::acquire_timeout_ms`).
    pub acquire_timeout: Duration,
    /// Бюджет ожидания GPU при пересоздании и отсоединении цепочки (из `RenderLoopConfig::teardown_timeout_ms`): истёк — объекты не
    /// уничтожаются, `detach` сообщает об ошибке.
    pub teardown_timeout: Duration,
    /// Предпочтение глубины цвета swapchain.
    pub color_depth: ColorDepthPreference,
    /// Сколько ещё не вычерпанных соответствий «id показа -> номер кадра потока» хранить.
    /// Нужно, чтобы связать записи display timing с кадрами; на ПК-стенде (записей нет)
    /// ограничение не даёт таблице расти бесконечно.
    pub presentation_history_capacity: usize,
    /// Упреждение желаемого времени показа, доля периода (из `RenderLoopConfig::present_time_lead_ratio`).
    pub present_time_lead_ratio: f64,
    /// Период обновления, пока драйвер не сообщил реальный (из `RenderLoopConfig::expected_refresh_period`).
    pub fallback_refresh_period: Duration,
}

impl VulkanBackendConfig {
    /// Запас таблицы показов по умолчанию: при 120 Гц это ~2 секунды, с избытком больше
    /// интервала опроса диагностики (250 мс).
    pub const DEFAULT_PRESENTATION_HISTORY_CAPACITY: usize = 256;

    /// Собирает настройки бэкенда из конфигурации потока рендера.
    /// Неверное `frames_in_flight` (0 или больше предела gpu-vulkan) — `InvalidArgument`.
    pub fn from_render_config(config: &RenderLoopConfig, validation: ValidationMode) -> Result<Self, GpuError> {
        let frames_in_flight = FramesInFlight::new(usize::try_from(config.frames_in_flight).unwrap_or(usize::MAX))?;
        Ok(Self {
            validation,
            frames_in_flight,
            acquire_timeout: config.acquire_timeout(),
            teardown_timeout: config.teardown_timeout(),
            color_depth: ColorDepthPreference::PreferTenBit,
            presentation_history_capacity: Self::DEFAULT_PRESENTATION_HISTORY_CAPACITY,
            present_time_lead_ratio: config.present_time_lead_ratio,
            fallback_refresh_period: config.expected_refresh_period(),
        })
    }

    /// Правило выбора желаемого времени показа для сессии показа.
    pub(super) fn present_time_policy(&self) -> PresentTimePolicy {
        PresentTimePolicy {
            lead_ratio: self.present_time_lead_ratio,
            fallback_refresh_period: self.fallback_refresh_period,
        }
    }

    /// Настройки цели показа для surface заданного размера.
    pub(super) fn display_target_config(&self, size: shell_contract::surface::SurfaceSize) -> DisplayTargetConfig {
        DisplayTargetConfig {
            frames_in_flight: self.frames_in_flight,
            acquire_timeout: self.acquire_timeout,
            teardown_timeout: self.teardown_timeout,
            present_fence_capacity: DisplayTargetConfig::DEFAULT_PRESENT_FENCE_CAPACITY,
            color_depth: self.color_depth,
            requested_size: Some(size),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_taken_from_render_loop_config() {
        let render = RenderLoopConfig {
            frames_in_flight: 3,
            acquire_timeout_ms: 70,
            // Не значение по умолчанию: тест доказывает, что поле берётся из конфига потока.
            teardown_timeout_ms: 120,
            ..RenderLoopConfig::default()
        };
        let config =
            VulkanBackendConfig::from_render_config(&render, ValidationMode::Disabled).expect("корректный конфиг");
        assert_eq!(config.frames_in_flight.get(), 3);
        assert_eq!(config.acquire_timeout, Duration::from_millis(70));
        assert_eq!(
            config.teardown_timeout,
            Duration::from_millis(120),
            "бюджет освобождения — из конфига потока"
        );
        assert_eq!(config.validation, ValidationMode::Disabled);
    }

    #[test]
    fn invalid_frames_in_flight_is_rejected() {
        for invalid in [0, 1000] {
            let render = RenderLoopConfig {
                frames_in_flight: invalid,
                ..RenderLoopConfig::default()
            };
            assert!(
                matches!(
                    VulkanBackendConfig::from_render_config(&render, ValidationMode::FULL),
                    Err(GpuError::InvalidArgument { .. })
                ),
                "frames_in_flight = {invalid}"
            );
        }
    }
}
