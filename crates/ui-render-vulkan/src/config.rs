//! Настройки рендера UI: пределы кадра и пирамида размытия фона. Всё, что можно подогнать, — параметры
//! (serde, `deny_unknown_fields`, `validate()`), а не константы в коде.

use ash::vk;
use gpu_vulkan::color_target::ColorTargetFormat;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ui_draw::color::DisplayPrimaries;

/// Сколько уровней пирамиды умеют шейдеры стекла: по одной привязке сэмплера на уровень
/// (`ui_glass_data.glsl`). Инженерный предел: 6 уровней = размытие до 2⁶ = 64 px на тексель, больше стилю не нужно.
pub const MAX_PYRAMID_LEVELS: u32 = 6;

/// Сколько сэмплеров картинок объявлено в шейдере `ui_image.frag`: по одной привязке на текстуру, число — часть
/// шейдера (GLSL не умеет массив привязок разной длины без индексирования дескрипторов). Инженерный предел:
/// кадру камеры нужно две картинки (кроссфейд миниатюры), четыре — запас.
pub const MAX_IMAGE_BINDINGS: u32 = 4;

/// Ошибка настроек.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum ConfigError {
    /// Нужен хотя бы один слот кадра.
    #[error("frame_slots должен быть больше нуля")]
    NoFrameSlots,
    /// Предел кадра равен нулю: такой рендер ничего не нарисует.
    #[error("предел «{field}» должен быть больше нуля")]
    ZeroLimit {
        /// Имя поля.
        field: &'static str,
    },
    /// Число уровней пирамиды вне допустимого.
    #[error("число уровней пирамиды {levels} вне 1..={MAX_PYRAMID_LEVELS}")]
    PyramidLevelsOutOfRange {
        /// Заданное число.
        levels: u32,
    },
    /// Картинок в кадре больше, чем сэмплеров в шейдере.
    #[error("max_images {max_images} больше числа сэмплеров шейдера {MAX_IMAGE_BINDINGS}")]
    ImagesAboveBindings {
        /// Заданное число.
        max_images: u32,
    },
}

/// Пределы одного кадра. Превышение — честная ошибка кадра со счётчиком (`UiRenderError::LimitExceeded`),
/// а не молчаливая обрезка. Размеры буферов экземпляров на слот кадра выводятся из них при создании.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UiRenderLimits {
    /// Закрашенных фигур в кадре. `[estimate]`: экран камеры — десятки фигур; 512 × 128 байт = 64 КиБ на слот.
    pub max_shape_instances: u32,
    /// Глифов текста и иконок в кадре. `[estimate]`: подписи камеры — сотни глифов; 1024 × 80 байт = 80 КиБ на слот.
    pub max_glyph_instances: u32,
    /// Групп стекла в кадре. `[estimate]`: 6–10 элементов в типичном кадре; 32 групп с запасом.
    /// Членов в группе не больше `MAX_GLASS_GROUP_MEMBERS`, поэтому буфер членов — `групп × 16`.
    pub max_glass_groups: u32,
    /// Элементов-картинок в кадре (1..=`MAX_IMAGE_BINDINGS`); картинок разных не больше этого числа.
    /// `[estimate]`: кроссфейд миниатюры снимка держит на экране две (старую и новую).
    pub max_images: u32,
    /// Наибольшая сторона картинки источника, px. Больше — честный отказ кадра (`PictureTooLarge`): картинка
    /// целиком живёт в видеопамяти и копируется в кадре. `[estimate]`: миниатюра ≈ 256 px, запас ×4.
    pub max_image_side_px: u32,
}

impl Default for UiRenderLimits {
    fn default() -> Self {
        Self {
            max_shape_instances: 512,
            max_glyph_instances: 1024,
            max_glass_groups: 32,
            max_images: 2,
            max_image_side_px: 1024,
        }
    }
}

impl UiRenderLimits {
    /// Проверить числа.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (field, value) in [
            ("max_shape_instances", self.max_shape_instances),
            ("max_glyph_instances", self.max_glyph_instances),
            ("max_glass_groups", self.max_glass_groups),
            ("max_images", self.max_images),
            ("max_image_side_px", self.max_image_side_px),
        ] {
            if value == 0 {
                return Err(ConfigError::ZeroLimit { field });
            }
        }
        if self.max_images > MAX_IMAGE_BINDINGS {
            return Err(ConfigError::ImagesAboveBindings {
                max_images: self.max_images,
            });
        }
        Ok(())
    }
}

/// Формат ступеней пирамиды размытия.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PyramidFormat {
    /// fp16: без квантования и полос на плавных градиентах (по умолчанию).
    Rgba16Float,
    /// 10 бит на канал: вдвое меньше памяти, полосы на очень плавных градиентах возможны.
    A2B10G10R10Unorm,
    /// 8 бит: самый лёгкий, полосы заметны на небе и коже — для отладки.
    Rgba8Unorm,
}

impl PyramidFormat {
    /// Формат цели `gpu-vulkan`.
    pub(crate) fn color_target_format(self) -> ColorTargetFormat {
        match self {
            Self::Rgba16Float => ColorTargetFormat::Rgba16Float,
            Self::A2B10G10R10Unorm => ColorTargetFormat::A2B10G10R10Unorm,
            Self::Rgba8Unorm => ColorTargetFormat::Rgba8Unorm,
        }
    }
}

/// Пирамида размытия фона: dual Kawase, ступени вниз 1/2, 1/4, … экрана.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BackdropPyramidConfig {
    /// Число уровней (1..=6). Уровень k даёт размытие с эффективной сигмой ≈ 0,75·2^k px; стиль
    /// просит радиус 2 pt ≈ 6 px при 3 px/pt — это уровень 2–3. Четыре уровня (до сигмы ≈ 12 px) покрывают
    /// радиусы до ≈ 24 px, а самый мелкий (1/16 экрана) годится для средней яркости под формой.
    /// `[estimate]`: подгонка, не Apple.
    pub levels: u32,
    /// Формат ступеней.
    pub format: PyramidFormat,
}

impl Default for BackdropPyramidConfig {
    fn default() -> Self {
        Self {
            levels: 4,
            format: PyramidFormat::Rgba16Float,
        }
    }
}

impl BackdropPyramidConfig {
    /// Проверить числа.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if (1..=MAX_PYRAMID_LEVELS).contains(&self.levels) {
            Ok(())
        } else {
            Err(ConfigError::PyramidLevelsOutOfRange { levels: self.levels })
        }
    }
}

/// Кэш текстур картинок.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PictureCacheConfig {
    /// Через сколько записанных кадров без использования картинка вытесняется из видеопамяти (0 — сразу
    /// после первого кадра без неё). `[estimate]`: 300 кадров ≈ 5 с при 60 Гц — переключение миниатюры туда-обратно
    /// не перезагружает картинку, а забытая освобождает память.
    pub idle_frames_before_eviction: u32,
}

impl Default for PictureCacheConfig {
    fn default() -> Self {
        Self {
            idle_frames_before_eviction: 300,
        }
    }
}

/// Настройки рендера UI.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UiRendererConfig {
    /// Формат цели показа (кадр swapchain или цель теста); проходы с целью иного формата отклоняются.
    pub target_format: vk::Format,
    /// Сколько кадров может быть в полёте: слотов буферов экземпляров и наборов дескрипторов.
    pub frame_slots: usize,
    /// Пределы кадра.
    pub limits: UiRenderLimits,
    /// Пирамида размытия фона.
    pub pyramid: BackdropPyramidConfig,
    /// Основные цвета экрана: в них шейдер переводит картинки sRGB. Фиксированы на жизнь рендера (константа
    /// специализации конвейера): сменился охват экрана — рендер создаётся заново, как при смене формата.
    pub display_primaries: DisplayPrimaries,
    /// Кэш текстур картинок.
    pub pictures: PictureCacheConfig,
}

impl UiRendererConfig {
    /// Настройки по умолчанию под формат цели и число слотов.
    pub fn new(target_format: vk::Format, frame_slots: usize) -> Self {
        Self {
            target_format,
            frame_slots,
            limits: UiRenderLimits::default(),
            pyramid: BackdropPyramidConfig::default(),
            display_primaries: DisplayPrimaries::Srgb,
            pictures: PictureCacheConfig::default(),
        }
    }

    /// Проверить числа.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.frame_slots == 0 {
            return Err(ConfigError::NoFrameSlots);
        }
        self.limits.validate()?;
        self.pyramid.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_zero_values_are_rejected() {
        assert_eq!(UiRendererConfig::new(vk::Format::R8G8B8A8_UNORM, 2).validate(), Ok(()));
        assert_eq!(
            UiRendererConfig::new(vk::Format::R8G8B8A8_UNORM, 0).validate(),
            Err(ConfigError::NoFrameSlots)
        );
        let mut config = UiRendererConfig::new(vk::Format::R8G8B8A8_UNORM, 2);
        config.limits.max_glass_groups = 0;
        assert_eq!(
            config.validate(),
            Err(ConfigError::ZeroLimit {
                field: "max_glass_groups"
            })
        );
        config.limits = UiRenderLimits {
            max_images: MAX_IMAGE_BINDINGS + 1,
            ..Default::default()
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::ImagesAboveBindings {
                max_images: MAX_IMAGE_BINDINGS + 1
            })
        );
        for levels in [0, MAX_PYRAMID_LEVELS + 1] {
            let pyramid = BackdropPyramidConfig {
                levels,
                ..Default::default()
            };
            assert_eq!(pyramid.validate(), Err(ConfigError::PyramidLevelsOutOfRange { levels }));
        }
    }

    #[test]
    fn limits_and_pyramid_round_trip_through_json_and_reject_unknown_fields() {
        let limits = UiRenderLimits {
            max_shape_instances: 7,
            ..Default::default()
        };
        let json = serde_json::to_string(&limits).unwrap();
        assert_eq!(serde_json::from_str::<UiRenderLimits>(&json).unwrap(), limits);
        assert!(serde_json::from_str::<UiRenderLimits>(r#"{"max_shapes": 1}"#).is_err());
        let pyramid: BackdropPyramidConfig = serde_json::from_str(r#"{"levels": 3, "format": "rgba8_unorm"}"#).unwrap();
        assert_eq!(pyramid.levels, 3);
        assert_eq!(pyramid.format, PyramidFormat::Rgba8Unorm);
    }
}
