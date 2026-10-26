//! `SurfaceTransform` — поворот/зеркало поверхности относительно «прямого» вида экрана.
//!
//! Значения соответствуют `VkSurfaceTransformFlagBitsKHR`. Определение из спецификации
//! (раздел «Surface Transform»): `ROTATE_90` — содержимое показанной картинки поворачивается
//! на 90° ПО ЧАСОВОЙ стрелке перед выводом на дисплей. Чтобы на экране картинка стояла прямо,
//! рендерер обязан нарисовать её уже повёрнутой в обратную сторону — это и делает паттерн.
//! Пакет swapchain использует этот же тип для `preTransform = currentTransform`.

use ash::vk;

use crate::error::GpuError;

/// Преобразование поверхности относительно прямого вида.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SurfaceTransform {
    /// Без поворота.
    Identity,
    /// Поворот на 90° по часовой стрелке при показе.
    Rotate90,
    /// Поворот на 180°.
    Rotate180,
    /// Поворот на 270° по часовой стрелке.
    Rotate270,
    /// Зеркало по горизонтали (рендерер паттерна не поддерживает).
    HorizontalMirror,
    /// Зеркало + поворот на 90° (не поддерживается паттерном).
    HorizontalMirrorRotate90,
    /// Зеркало + поворот на 180° (не поддерживается паттерном).
    HorizontalMirrorRotate180,
    /// Зеркало + поворот на 270° (не поддерживается паттерном).
    HorizontalMirrorRotate270,
}

impl SurfaceTransform {
    /// Четыре поддерживаемых паттерном поворота (для перебора в тестах и диагностиках).
    pub const ROTATIONS: [SurfaceTransform; 4] = [
        SurfaceTransform::Identity,
        SurfaceTransform::Rotate90,
        SurfaceTransform::Rotate180,
        SurfaceTransform::Rotate270,
    ];

    /// Из флага Vulkan. `None` — если флаг не одиночный известный (например, `INHERIT` или маска).
    pub fn from_vk(flags: vk::SurfaceTransformFlagsKHR) -> Option<Self> {
        match flags {
            vk::SurfaceTransformFlagsKHR::IDENTITY => Some(Self::Identity),
            vk::SurfaceTransformFlagsKHR::ROTATE_90 => Some(Self::Rotate90),
            vk::SurfaceTransformFlagsKHR::ROTATE_180 => Some(Self::Rotate180),
            vk::SurfaceTransformFlagsKHR::ROTATE_270 => Some(Self::Rotate270),
            vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR => Some(Self::HorizontalMirror),
            vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR_ROTATE_90 => Some(Self::HorizontalMirrorRotate90),
            vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR_ROTATE_180 => Some(Self::HorizontalMirrorRotate180),
            vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR_ROTATE_270 => Some(Self::HorizontalMirrorRotate270),
            _ => None,
        }
    }

    /// Во флаг Vulkan (для `preTransform`).
    pub fn to_vk(self) -> vk::SurfaceTransformFlagsKHR {
        match self {
            Self::Identity => vk::SurfaceTransformFlagsKHR::IDENTITY,
            Self::Rotate90 => vk::SurfaceTransformFlagsKHR::ROTATE_90,
            Self::Rotate180 => vk::SurfaceTransformFlagsKHR::ROTATE_180,
            Self::Rotate270 => vk::SurfaceTransformFlagsKHR::ROTATE_270,
            Self::HorizontalMirror => vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR,
            Self::HorizontalMirrorRotate90 => vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR_ROTATE_90,
            Self::HorizontalMirrorRotate180 => vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR_ROTATE_180,
            Self::HorizontalMirrorRotate270 => vk::SurfaceTransformFlagsKHR::HORIZONTAL_MIRROR_ROTATE_270,
        }
    }

    /// Меняются ли местами ширина и высота (повороты на 90° и 270°, в том числе с зеркалом).
    pub fn swaps_extent(self) -> bool {
        matches!(
            self,
            Self::Rotate90 | Self::Rotate270 | Self::HorizontalMirrorRotate90 | Self::HorizontalMirrorRotate270
        )
    }

    /// Число четвертей оборота по часовой (0..=3). Зеркальные варианты — явная ошибка `Unsupported`.
    pub fn quarter_turns(self) -> Result<u32, GpuError> {
        match self {
            Self::Identity => Ok(0),
            Self::Rotate90 => Ok(1),
            Self::Rotate180 => Ok(2),
            Self::Rotate270 => Ok(3),
            mirrored => Err(GpuError::Unsupported {
                what: format!("зеркальный preTransform {mirrored:?} в тестовом паттерне"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vk_conversion_round_trips_for_every_variant() {
        for transform in [
            SurfaceTransform::Identity,
            SurfaceTransform::Rotate90,
            SurfaceTransform::Rotate180,
            SurfaceTransform::Rotate270,
            SurfaceTransform::HorizontalMirror,
            SurfaceTransform::HorizontalMirrorRotate90,
            SurfaceTransform::HorizontalMirrorRotate180,
            SurfaceTransform::HorizontalMirrorRotate270,
        ] {
            assert_eq!(SurfaceTransform::from_vk(transform.to_vk()), Some(transform));
        }
        assert_eq!(SurfaceTransform::from_vk(vk::SurfaceTransformFlagsKHR::INHERIT), None);
        assert_eq!(
            SurfaceTransform::from_vk(vk::SurfaceTransformFlagsKHR::IDENTITY | vk::SurfaceTransformFlagsKHR::ROTATE_90),
            None
        );
    }

    #[test]
    fn rotations_swap_extent_only_for_quarter_turns() {
        assert!(!SurfaceTransform::Identity.swaps_extent());
        assert!(SurfaceTransform::Rotate90.swaps_extent());
        assert!(!SurfaceTransform::Rotate180.swaps_extent());
        assert!(SurfaceTransform::Rotate270.swaps_extent());
        assert!(SurfaceTransform::HorizontalMirrorRotate90.swaps_extent());
    }

    #[test]
    fn mirrored_variants_are_explicitly_unsupported() {
        assert_eq!(SurfaceTransform::Rotate270.quarter_turns(), Ok(3));
        let error = SurfaceTransform::HorizontalMirror.quarter_turns().unwrap_err();
        assert!(matches!(error, GpuError::Unsupported { .. }), "{error:?}");
    }
}
