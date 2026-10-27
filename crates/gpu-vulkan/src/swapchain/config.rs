//! Настройки цели показа. Без констант в логике: всё, что можно подкрутить, — поле конфигурации.

use std::num::NonZeroUsize;
use std::time::Duration;

use shell_contract::surface::SurfaceSize;

use crate::error::GpuError;

/// Сколько кадров одновременно «в полёте» (CPU записывает следующий, пока GPU рисует предыдущий).
///
/// Newtype над `NonZeroUsize`: ноль слотов невозможен по построению, а на месте вызова
/// `FramesInFlight::new(2)` читается лучше голого числа.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FramesInFlight(NonZeroUsize);

impl FramesInFlight {
    /// Верхняя граница: больше слотов только копят задержку, а ресурсов (cmd buffer + 2 объекта) жалко.
    pub const MAX: usize = 8;

    /// Значение по умолчанию: двойная буферизация CPU/GPU (решение конфигурации `frames_in_flight = 2`).
    pub const DEFAULT: FramesInFlight = FramesInFlight(NonZeroUsize::MIN.saturating_add(1));

    /// Создаёт значение; `0` и числа больше [`Self::MAX`] — `InvalidArgument`.
    pub fn new(count: usize) -> Result<Self, GpuError> {
        match NonZeroUsize::new(count) {
            Some(non_zero) if count <= Self::MAX => Ok(Self(non_zero)),
            _ => Err(GpuError::InvalidArgument {
                what: format!("frames_in_flight = {count}, допустимо 1..={}", Self::MAX),
            }),
        }
    }

    /// Число слотов.
    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for FramesInFlight {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Предпочтение глубины цвета swapchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepthPreference {
    /// 10 бит на канал, если surface предлагает; иначе 8 бит.
    PreferTenBit,
    /// Только 8 бит на канал (10-битный формат даже не рассматривается).
    EightBitOnly,
}

/// Предпочтение цветового пространства показа (сессия 07).
///
/// Экран телефона широкоцветный: видоискатель в Display P3 показывает насыщенные цвета сцены, которые
/// в sRGB пришлось бы обрезать. P3 выбирается только вместе с 10-битным UNORM-форматом (8 бит на
/// более широкий охват дают заметные ступеньки) и только если включено расширение instance
/// `VK_EXT_swapchain_colorspace`; иначе — прежний выбор sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DisplayColorPreference {
    /// Display P3 + 10-бит UNORM, если surface предлагает; иначе sRGB.
    #[default]
    PreferDisplayP3,
    /// Только sRGB (`SRGB_NONLINEAR`), P3 даже не рассматривается.
    SrgbOnly,
}

/// Цветовое пространство, в котором дисплей интерпретирует значения картинки swapchain.
///
/// Нейтральная замена `vk::ColorSpaceKHR` для потребителей: ISP по нему выбирает охват выхода
/// (матрица XYZ → экран и OETF). Оба варианта — нелинейные с кривой sRGB (у Display P3 та же
/// передаточная функция, другие первичные цвета). Значения пишутся «как есть» в UNORM-формат.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DisplayColorSpace {
    /// sRGB / BT.709 первичные, кривая sRGB (`VK_COLOR_SPACE_SRGB_NONLINEAR_KHR`).
    Srgb,
    /// Display P3 первичные (D65), кривая sRGB (`VK_COLOR_SPACE_DISPLAY_P3_NONLINEAR_EXT`).
    DisplayP3,
}

impl DisplayColorSpace {
    /// Из цветового пространства Vulkan; `None` — пространство, которое политика не выбирает.
    pub fn from_vk(color_space: ash::vk::ColorSpaceKHR) -> Option<Self> {
        match color_space {
            ash::vk::ColorSpaceKHR::SRGB_NONLINEAR => Some(Self::Srgb),
            ash::vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT => Some(Self::DisplayP3),
            _ => None,
        }
    }

    /// В цветовое пространство Vulkan.
    pub fn to_vk(self) -> ash::vk::ColorSpaceKHR {
        match self {
            Self::Srgb => ash::vk::ColorSpaceKHR::SRGB_NONLINEAR,
            Self::DisplayP3 => ash::vk::ColorSpaceKHR::DISPLAY_P3_NONLINEAR_EXT,
        }
    }
}

/// Параметры создания [`crate::swapchain::DisplayTarget`].
#[derive(Debug, Clone)]
pub struct DisplayTargetConfig {
    /// Число кадров в полёте.
    pub frames_in_flight: FramesInFlight,
    /// Бюджет ожидания кадра в `acquire_frame`: ожидание слота (fence) + `vkAcquireNextImageKHR`.
    /// Истёк — `AcquireOutcome::TimedOut`, а не вечное зависание.
    pub acquire_timeout: Duration,
    /// Бюджет ожидания GPU (fence'ы кадров + fence'ы показа) при пересоздании и отсоединении. Истёк — объекты НЕ уничтожаются
    /// (уничтожать занятое нельзя), цель сообщает ошибку.
    pub teardown_timeout: Duration,
    /// Сколько fence'ов показа пул держит максимум (при `VK_EXT_swapchain_maintenance1`).
    /// Нужно не меньше числа картинок swapchain: столько показов может быть в полёте. Исчерпание
    /// не ошибка, но освобождение цели тогда вернётся к ожиданию простоя устройства (без таймаута).
    pub present_fence_capacity: usize,
    /// Какую глубину цвета предпочесть.
    pub color_depth: ColorDepthPreference,
    /// Какое цветовое пространство показа предпочесть (Display P3 требует 10 бит и расширения instance).
    pub display_color: DisplayColorPreference,
    /// Желаемый размер (в обычной ориентации, как сообщил `surfaceChanged`). Используется, только если
    /// surface не диктует размер сама (`currentExtent` не определён — так у headless-стенда ПК).
    pub requested_size: Option<SurfaceSize>,
}

impl DisplayTargetConfig {
    /// 100 мс — значение `acquire_timeout_ms` из конфигурации render-loop: ровно столько
    /// допустимо ждать кадр, прежде чем цикл рендера проверит команды и остановку.
    pub const DEFAULT_ACQUIRE_TIMEOUT: Duration = Duration::from_millis(100);
    /// 32 — с большим запасом над числом картинок swapchain (обычно 3–4, предел драйвера 8–16).
    pub const DEFAULT_PRESENT_FENCE_CAPACITY: usize = 32;
    /// 5 секунд — запас для медленного драйвера/CI при простое GPU, но не вечное ожидание.
    pub const DEFAULT_TEARDOWN_TIMEOUT: Duration = Duration::from_secs(5);
}

impl Default for DisplayTargetConfig {
    fn default() -> Self {
        Self {
            frames_in_flight: FramesInFlight::DEFAULT,
            acquire_timeout: Self::DEFAULT_ACQUIRE_TIMEOUT,
            teardown_timeout: Self::DEFAULT_TEARDOWN_TIMEOUT,
            present_fence_capacity: Self::DEFAULT_PRESENT_FENCE_CAPACITY,
            color_depth: ColorDepthPreference::PreferTenBit,
            display_color: DisplayColorPreference::PreferDisplayP3,
            requested_size: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_in_flight_rejects_zero_and_oversized_values() {
        assert_eq!(FramesInFlight::new(1).map(FramesInFlight::get), Ok(1));
        assert_eq!(
            FramesInFlight::new(FramesInFlight::MAX).map(FramesInFlight::get),
            Ok(FramesInFlight::MAX)
        );
        assert!(matches!(FramesInFlight::new(0), Err(GpuError::InvalidArgument { .. })));
        assert!(matches!(
            FramesInFlight::new(FramesInFlight::MAX + 1),
            Err(GpuError::InvalidArgument { .. })
        ));
    }

    #[test]
    fn defaults_match_render_loop_config() {
        let config = DisplayTargetConfig::default();
        assert_eq!(config.frames_in_flight.get(), 2);
        assert_eq!(config.acquire_timeout, Duration::from_millis(100));
        assert_eq!(config.color_depth, ColorDepthPreference::PreferTenBit);
        assert_eq!(config.display_color, DisplayColorPreference::PreferDisplayP3);
    }

    #[test]
    fn display_color_space_round_trips_and_rejects_unknown_spaces() {
        for space in [DisplayColorSpace::Srgb, DisplayColorSpace::DisplayP3] {
            assert_eq!(DisplayColorSpace::from_vk(space.to_vk()), Some(space));
        }
        assert_eq!(
            DisplayColorSpace::from_vk(ash::vk::ColorSpaceKHR::EXTENDED_SRGB_LINEAR_EXT),
            None
        );
    }
}
