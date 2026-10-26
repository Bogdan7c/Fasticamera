//! Раскладки импорта `AHardwareBuffer` в Vulkan и наши собственные проверки размеров.
//!
//! Модуль нейтральный (без Android и без Vulkan-вызовов): проверки размеров и выбор типа памяти —
//! чистые функции, которые тестируются на ПК. Зачем проверять самим: по факту 00 (дефект 5) драйвер
//! Adreno НЕ отвергает bind картинки/буфера, который больше `allocationSize` импортированной памяти —
//! молча получится выход за границы. Поэтому «влезает ли ресурс в аллокацию» решаем мы, до bind.

use std::fmt;

/// Как интерпретировать содержимое импортируемого буфера в Vulkan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImportLayout {
    /// `VkImage` формата `R16_UINT` (sampled image, читается `texelFetch`) — RAW16 (AHB-формат 0x20).
    SampledImageR16Uint {
        /// Ширина, текселей.
        width: u32,
        /// Высота, текселей.
        height: u32,
    },
    /// `VkBuffer` поверх памяти AHB — упакованный RAW10 (AHB-формат 0x25), только буфер:
    /// распаковка делается шейдером (факт 00, дефект 5b: картинкой RAW10 не импортируется).
    PackedBuffer {
        /// Шаг строки в БАЙТАХ (у AHB для RAW10 `stride` — байты строки, дефект 5a).
        row_stride_bytes: u32,
        /// Число строк.
        rows: u32,
    },
}

/// Ошибка проверки раскладки до обращения к драйверу.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    /// В раскладке нулевой размер (ширина/высота/шаг/число строк = 0).
    EmptyDimension,
    /// Ресурсу нужно больше байт, чем в аллокации импортированной памяти.
    ExceedsAllocation {
        /// Минимум байт, которые нужны раскладке.
        required_bytes: u64,
        /// `allocationSize`, который вернул драйвер для этого AHB.
        allocation_size: u64,
    },
}

impl fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LayoutError::EmptyDimension => formatter.write_str("в раскладке нулевой размер"),
            LayoutError::ExceedsAllocation {
                required_bytes,
                allocation_size,
            } => write!(
                formatter,
                "раскладке нужно {required_bytes} байт, а в аллокации AHB {allocation_size}"
            ),
        }
    }
}

impl std::error::Error for LayoutError {}

/// Размер текселя `R16_UINT`, байт.
const R16_TEXEL_BYTES: u64 = 2;

impl ImportLayout {
    /// Нижняя граница размера в байтах, который займёт ресурс этой раскладки.
    ///
    /// * картинка: `width * height * 2` — настоящий размер (с выравниванием строк) знает только
    ///   драйвер, он проверяется отдельно по `vkGetImageMemoryRequirements`;
    /// * буфер: `rows * row_stride_bytes` — все строки целиком, с паддингом.
    ///
    /// Умножение с насыщением: переполнение `u64` дало бы заведомо «не влезает», а не тихий ноль.
    pub fn minimum_bytes(&self) -> u64 {
        match *self {
            ImportLayout::SampledImageR16Uint { width, height } => u64::from(width)
                .saturating_mul(u64::from(height))
                .saturating_mul(R16_TEXEL_BYTES),
            ImportLayout::PackedBuffer { row_stride_bytes, rows } => {
                u64::from(row_stride_bytes).saturating_mul(u64::from(rows))
            }
        }
    }

    /// Проверяет раскладку против `allocationSize` импортированной памяти.
    pub fn check_fits(&self, allocation_size: u64) -> Result<(), LayoutError> {
        if self.minimum_bytes() == 0 {
            return Err(LayoutError::EmptyDimension);
        }
        Self::check_bytes_fit(self.minimum_bytes(), allocation_size)
    }

    /// Общая проверка «нужно ≤ есть» (используется и для размера из `vkGet*MemoryRequirements`).
    pub fn check_bytes_fit(required_bytes: u64, allocation_size: u64) -> Result<(), LayoutError> {
        if required_bytes > allocation_size {
            return Err(LayoutError::ExceedsAllocation {
                required_bytes,
                allocation_size,
            });
        }
        Ok(())
    }
}

/// Тип памяти для импорта: любой, разрешённый и AHB (`memoryTypeBits` из
/// `vkGetAndroidHardwareBufferPropertiesANDROID`), и самим ресурсом (`vkGet*MemoryRequirements`).
///
/// Берём младший из пересечения: для импорта внешней памяти спецификация требует лишь, чтобы бит
/// типа был в маске AHB (VUID-VkMemoryAllocateInfo-memoryTypeIndex-02385); флаги свойств не важны.
/// `None` — пересечение пусто.
pub fn pick_import_memory_type(ahb_memory_type_bits: u32, resource_memory_type_bits: u32) -> Option<u32> {
    let allowed = ahb_memory_type_bits & resource_memory_type_bits;
    (allowed != 0).then(|| allowed.trailing_zeros())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PHONE_RAW10_ALLOCATION: u64 = 15_728_640;

    #[test]
    fn minimum_bytes_for_each_layout() {
        assert_eq!(
            ImportLayout::SampledImageR16Uint {
                width: 4096,
                height: 3072
            }
            .minimum_bytes(),
            25_165_824
        );
        assert_eq!(
            ImportLayout::PackedBuffer {
                row_stride_bytes: 5120,
                rows: 3072
            }
            .minimum_bytes(),
            PHONE_RAW10_ALLOCATION
        );
    }

    #[test]
    fn exactly_fitting_layout_is_accepted() {
        let layout = ImportLayout::PackedBuffer {
            row_stride_bytes: 5120,
            rows: 3072,
        };
        assert_eq!(layout.check_fits(PHONE_RAW10_ALLOCATION), Ok(()));
    }

    #[test]
    fn layout_bigger_than_allocation_is_rejected_with_both_sizes() {
        // Дефект 5: драйвер такой bind не отвергает, поэтому отвергаем мы.
        let layout = ImportLayout::PackedBuffer {
            row_stride_bytes: 5121,
            rows: 3072,
        };
        assert_eq!(
            layout.check_fits(PHONE_RAW10_ALLOCATION),
            Err(LayoutError::ExceedsAllocation {
                required_bytes: 5121 * 3072,
                allocation_size: PHONE_RAW10_ALLOCATION
            })
        );
        let image = ImportLayout::SampledImageR16Uint {
            width: 4096,
            height: 3072,
        };
        assert!(image.check_fits(25_165_823).is_err());
        assert!(image.check_fits(25_165_824).is_ok());
    }

    #[test]
    fn zero_dimension_is_rejected_even_for_huge_allocation() {
        assert_eq!(
            ImportLayout::SampledImageR16Uint { width: 0, height: 10 }.check_fits(u64::MAX),
            Err(LayoutError::EmptyDimension)
        );
        assert_eq!(
            ImportLayout::PackedBuffer {
                row_stride_bytes: 8,
                rows: 0
            }
            .check_fits(u64::MAX),
            Err(LayoutError::EmptyDimension)
        );
    }

    #[test]
    fn huge_dimensions_do_not_overflow_into_a_small_number() {
        let layout = ImportLayout::PackedBuffer {
            row_stride_bytes: u32::MAX,
            rows: u32::MAX,
        };
        assert!(layout.minimum_bytes() > u64::from(u32::MAX));
        assert!(layout.check_fits(1 << 40).is_err());
    }

    #[test]
    fn memory_type_is_lowest_bit_of_the_intersection() {
        assert_eq!(pick_import_memory_type(0b1100, 0b0110), Some(2));
        assert_eq!(pick_import_memory_type(0b1000, 0b0111), None);
        assert_eq!(pick_import_memory_type(0, u32::MAX), None);
    }
}
