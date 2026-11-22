//! Настройки Vulkan-бэкенда показа. Всё, что можно подкрутить, — поле, а не константа в логике.

use std::num::NonZeroUsize;
use std::time::Duration;

use gpu_vulkan::error::GpuError;
use gpu_vulkan::swapchain::{ColorDepthPreference, DisplayColorPreference, DisplayTargetConfig, FramesInFlight};
use gpu_vulkan::validation::ValidationMode;

use super::present_time::PresentTimePolicy;
use crate::config::{DisplayColorConfig, RenderLoopConfig, StillDevelopConfig, UiOverlayConfig, ViewfinderConfig};

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
    /// Сколько записей «показ — метка сенсора» хранить для расчёта задержки «сенсор -> дисплей»
    /// (только показ кадров камеры). Должно покрывать интервал опроса диагностики.
    pub camera_present_history_capacity: usize,
    /// Сколько импортов буферов камеры держать живыми (только Android). Не меньше пула буферов
    /// `AImageReader`, иначе каждый кадр будет импортироваться заново.
    pub camera_import_cache_capacity: NonZeroUsize,
    /// Какое цветовое пространство просить у цепочки показа (из `RenderLoopConfig::display_color`).
    pub display_color: DisplayColorPreference,
    /// Видоискатель камеры (из `RenderLoopConfig::viewfinder`).
    pub viewfinder: ViewfinderConfig,
    /// UI и стекло поверх видоискателя (из `RenderLoopConfig::ui_overlay`).
    pub ui_overlay: UiOverlayConfig,
    /// Снимок без задержки затвора (из `RenderLoopConfig::still`).
    pub still: StillDevelopConfig,
}

impl VulkanBackendConfig {
    /// Запас таблицы показов по умолчанию: при 120 Гц это ~2 секунды, с избытком больше
    /// интервала опроса диагностики (250 мс).
    pub const DEFAULT_PRESENTATION_HISTORY_CAPACITY: usize = 256;

    /// История показов кадров камеры по умолчанию: ~2 секунды при 60 к/с, с запасом над опросом диагностики.
    pub const DEFAULT_CAMERA_PRESENT_HISTORY_CAPACITY: usize = 128;

    /// Кэш импортов буферов камеры по умолчанию: с запасом над пулом `AImageReader` (рендер держит 2 кадра,
    /// остальное — очередь и ожидание пары).
    pub const DEFAULT_CAMERA_IMPORT_CACHE_CAPACITY: NonZeroUsize = match NonZeroUsize::new(12) {
        Some(capacity) => capacity,
        None => NonZeroUsize::MIN,
    };

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
            camera_present_history_capacity: Self::DEFAULT_CAMERA_PRESENT_HISTORY_CAPACITY,
            camera_import_cache_capacity: Self::DEFAULT_CAMERA_IMPORT_CACHE_CAPACITY,
            display_color: match config.display_color {
                DisplayColorConfig::PreferDisplayP3 => DisplayColorPreference::PreferDisplayP3,
                DisplayColorConfig::SrgbOnly => DisplayColorPreference::SrgbOnly,
            },
            viewfinder: config.viewfinder.clone(),
            ui_overlay: config.ui_overlay,
            still: config.still,
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
            // ISP видоискателя кодирует выход в пространстве, которое выбрала цепочка (сессия 07).
            display_color: self.display_color,
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
            ui_overlay: UiOverlayConfig {
                pyramid: ui_render_vulkan::BackdropPyramidConfig {
                    levels: 2,
                    ..ui_render_vulkan::BackdropPyramidConfig::default()
                },
                ..UiOverlayConfig::default()
            },
            ..RenderLoopConfig::default()
        };
        let config =
            VulkanBackendConfig::from_render_config(&render, ValidationMode::Disabled).expect("корректный конфиг");
        assert_eq!(
            config.ui_overlay.pyramid.levels, 2,
            "секция ui_overlay — из конфига потока"
        );
        assert_eq!(config.frames_in_flight.get(), 3);
        assert_eq!(config.acquire_timeout, Duration::from_millis(70));
        assert_eq!(
            config.teardown_timeout,
            Duration::from_millis(120),
            "бюджет освобождения — из конфига потока"
        );
        assert_eq!(config.validation, ValidationMode::Disabled);
        assert_eq!(
            config.display_color,
            DisplayColorPreference::PreferDisplayP3,
            "по умолчанию просим Display P3"
        );
        let srgb = RenderLoopConfig {
            display_color: DisplayColorConfig::SrgbOnly,
            ..RenderLoopConfig::default()
        };
        let config =
            VulkanBackendConfig::from_render_config(&srgb, ValidationMode::Disabled).expect("корректный конфиг");
        assert_eq!(
            config
                .display_target_config(shell_contract::surface::SurfaceSize {
                    width_px: 4,
                    height_px: 4
                })
                .display_color,
            DisplayColorPreference::SrgbOnly,
            "выбор пространства доходит до цели показа"
        );
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
