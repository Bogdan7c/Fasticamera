//! Настройки адаптера «кадр показа» под стенд насыщения и их проверка (до любых команд GPU).

use ash::vk;
use gpu_vulkan::color_target::ColorTargetFormat;
use gpu_vulkan::swapchain::FramesInFlight;

use crate::config::{BackdropPyramidConfig, ConfigError};

/// Размер цели по умолчанию: логический экран OnePlus 11 в портрете (как в `tests/timing.rs`).
pub const DEFAULT_TARGET_EXTENT: vk::Extent2D = vk::Extent2D {
    width: 1080,
    height: 2412,
};
/// Формат цели по умолчанию: тот же, что выбирает политика показа для swapchain телефона
/// (10 бит на канал, `A2B10G10R10_UNORM`).
pub const DEFAULT_TARGET_FORMAT: ColorTargetFormat = ColorTargetFormat::A2B10G10R10Unorm;
/// Сколько последних кадров помнит журнал времён проходов по умолчанию: хватает на устоявшийся режим,
/// а память фиксирована и не растёт вместе с длиной прогона.
pub const DEFAULT_PASS_TIME_WINDOW_FRAMES: usize = 1024;

/// Из чего состоит один «кадр» работы. Нужен, чтобы ВЫЧИТАНИЕМ получить цену каждой части:
/// `цена(UI) = цена(CompositorThenUi) − цена(CompositorOnly)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameComposition {
    /// Только компоновщик: картинка «видоискателя» кладётся в цель экрана, UI нет.
    CompositorOnly,
    /// Только UI: цель очищается (как у `tests/timing.rs`), компоновщик НЕ записывается; стекло видит фон,
    /// пирамида строится по обычным правилам.
    UiOnly,
    /// Как продукт на показанный кадр: компоновщик, барьер между цветовыми проходами, проход UI.
    CompositorThenUi,
}

impl FrameComposition {
    /// Записывается ли в кадр компоновщик.
    pub fn includes_compositor(self) -> bool {
        matches!(self, Self::CompositorOnly | Self::CompositorThenUi)
    }

    /// Записывается ли в кадр проход UI (с пирамидой фона).
    pub fn includes_ui(self) -> bool {
        matches!(self, Self::UiOnly | Self::CompositorThenUi)
    }

    /// Короткое имя для таблиц и отчётов.
    pub fn name(self) -> &'static str {
        match self {
            Self::CompositorOnly => "compositor",
            Self::UiOnly => "ui",
            Self::CompositorThenUi => "compositor+ui",
        }
    }

    /// Имена проходов в порядке записи кадра — метки для `PassTimeLog`.
    pub(crate) fn pass_labels(self) -> &'static [&'static str] {
        match self {
            Self::CompositorOnly => &["compose"],
            Self::UiOnly => &["clear", "pyramid", "ui"],
            Self::CompositorThenUi => &["compose", "pyramid", "ui"],
        }
    }
}

/// Как меняется кадр фона (картинка «видоискателя») от показанного кадра к показанному.
///
/// Продукт различает эти случаи по НОМЕРУ КАДРА ФОНА в ключе пирамиды (`BackdropSource::frame`):
/// тот же номер → пирамида переиспользуется, новый → перестраивается. Адаптер делает то же самое.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackdropCadence {
    /// Новый кадр камеры на каждом показанном кадре (экран 60 Гц и камера 60 к/с): пирамида каждый раз строится заново.
    NewFrameEveryFrame,
    /// Кадр камеры не менялся (тик 120 Гц без нового кадра): пирамида переиспользуется.
    SameFrame,
}

impl BackdropCadence {
    /// Номер кадра фона для кадра работы `frame_index` (сквозной, с нуля).
    pub(crate) fn backdrop_frame_number(self, frame_index: u64) -> gpu_vulkan::backdrop::BackdropFrameNumber {
        match self {
            // Каждый кадр работы — новый кадр камеры.
            Self::NewFrameEveryFrame => gpu_vulkan::backdrop::BackdropFrameNumber(frame_index),
            // Кадр камеры один на весь прогон.
            Self::SameFrame => gpu_vulkan::backdrop::BackdropFrameNumber(0),
        }
    }

    /// Короткое имя для таблиц и отчётов.
    pub fn name(self) -> &'static str {
        match self {
            Self::NewFrameEveryFrame => "new-frame",
            Self::SameFrame => "same-frame",
        }
    }
}

/// Настройки адаптера. Умолчания повторяют продукт; значения проверяет [`Self::validate`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiFrameSaturationConfig {
    /// Размер цели (картинки «экрана»), пиксели. Умолчание — [`DEFAULT_TARGET_EXTENT`].
    pub target_extent: vk::Extent2D,
    /// Формат цели. Умолчание — [`DEFAULT_TARGET_FORMAT`].
    pub target_format: ColorTargetFormat,
    /// Сколько слотов у адаптера. ДОЛЖНО совпасть с `SaturationConfig::frames_in_flight` стенда: слот стенда
    /// индексирует цели, компоновщик и рендер UI.
    pub frames_in_flight: FramesInFlight,
    /// Что входит в кадр.
    pub composition: FrameComposition,
    /// Как меняется кадр фона.
    pub backdrop_cadence: BackdropCadence,
    /// Пирамида фона. Умолчание — продуктовое: `UiOverlayConfig::pyramid` у продукта берётся из
    /// `BackdropPyramidConfig::default()` (4 уровня, fp16).
    pub pyramid: BackdropPyramidConfig,
    /// Сколько последних кадров помнит журнал времён проходов (> 0).
    pub pass_time_window_frames: usize,
}

impl Default for UiFrameSaturationConfig {
    fn default() -> Self {
        Self {
            target_extent: DEFAULT_TARGET_EXTENT,
            target_format: DEFAULT_TARGET_FORMAT,
            frames_in_flight: FramesInFlight::DEFAULT,
            composition: FrameComposition::CompositorThenUi,
            backdrop_cadence: BackdropCadence::NewFrameEveryFrame,
            pyramid: BackdropPyramidConfig::default(),
            pass_time_window_frames: DEFAULT_PASS_TIME_WINDOW_FRAMES,
        }
    }
}

/// Что не так с настройками адаптера или сценой. Все случаи ловятся ДО создания объектов GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UiSaturationConfigError {
    /// Нулевая сторона цели.
    #[error("размер цели {width}x{height}: обе стороны должны быть > 0")]
    ZeroTargetExtent {
        /// Ширина.
        width: u32,
        /// Высота.
        height: u32,
    },
    /// Нулевая сторона прямоугольника «видоискателя» в сцене.
    #[error("прямоугольник видоискателя {width}x{height}: обе стороны должны быть > 0")]
    ZeroViewfinderRect {
        /// Ширина.
        width: u32,
        /// Высота.
        height: u32,
    },
    /// Журнал времён проходов не может быть пустым.
    #[error("окно журнала времён проходов должно быть > 0 кадров")]
    ZeroPassTimeWindow,
    /// Пирамида фона не прошла проверку рендера UI.
    #[error("настройки пирамиды фона: {0}")]
    Pyramid(#[from] ConfigError),
}

impl UiFrameSaturationConfig {
    /// Проверяет числа (GPU не трогает).
    pub fn validate(&self) -> Result<(), UiSaturationConfigError> {
        let vk::Extent2D { width, height } = self.target_extent;
        if width == 0 || height == 0 {
            return Err(UiSaturationConfigError::ZeroTargetExtent { width, height });
        }
        if self.pass_time_window_frames == 0 {
            return Err(UiSaturationConfigError::ZeroPassTimeWindow);
        }
        self.pyramid.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_match_the_product_screen() {
        let config = UiFrameSaturationConfig::default();
        assert_eq!(config.validate(), Ok(()));
        assert_eq!((config.target_extent.width, config.target_extent.height), (1080, 2412));
        assert_eq!(config.pyramid, BackdropPyramidConfig::default());
    }

    #[test]
    fn broken_numbers_are_rejected_with_distinct_errors() {
        let base = UiFrameSaturationConfig::default();
        let zero_width = UiFrameSaturationConfig {
            target_extent: vk::Extent2D { width: 0, height: 10 },
            ..base
        };
        assert_eq!(
            zero_width.validate(),
            Err(UiSaturationConfigError::ZeroTargetExtent { width: 0, height: 10 })
        );
        let no_window = UiFrameSaturationConfig {
            pass_time_window_frames: 0,
            ..base
        };
        assert_eq!(no_window.validate(), Err(UiSaturationConfigError::ZeroPassTimeWindow));
        let no_levels = UiFrameSaturationConfig {
            pyramid: BackdropPyramidConfig {
                levels: 0,
                ..base.pyramid
            },
            ..base
        };
        assert_eq!(
            no_levels.validate(),
            Err(UiSaturationConfigError::Pyramid(ConfigError::PyramidLevelsOutOfRange {
                levels: 0
            }))
        );
    }

    #[test]
    fn cadence_gives_new_backdrop_numbers_only_when_the_camera_frame_changes() {
        let fresh = BackdropCadence::NewFrameEveryFrame;
        let held = BackdropCadence::SameFrame;
        assert_ne!(fresh.backdrop_frame_number(3), fresh.backdrop_frame_number(4));
        assert_eq!(held.backdrop_frame_number(3), held.backdrop_frame_number(4));
    }

    #[test]
    fn composition_labels_follow_what_is_recorded() {
        assert_eq!(FrameComposition::CompositorOnly.pass_labels(), &["compose"]);
        assert_eq!(FrameComposition::UiOnly.pass_labels(), &["clear", "pyramid", "ui"]);
        assert!(
            FrameComposition::CompositorThenUi.includes_compositor()
                && FrameComposition::CompositorThenUi.includes_ui()
        );
        assert!(!FrameComposition::UiOnly.includes_compositor());
        assert!(!FrameComposition::CompositorOnly.includes_ui());
    }
}
