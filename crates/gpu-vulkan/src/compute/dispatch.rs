//! Размер рабочей группы и число групп dispatch — чистая арифметика.

use std::num::NonZeroU32;

use crate::context::GpuContext;
use crate::error::GpuError;

/// Лимиты размера ОДНОЙ рабочей группы устройства (`VkPhysicalDeviceLimits`).
///
/// Размер группы шейдер получает specialization-константами (`local_size_x_id`), поэтому драйвер узнаёт
/// его только при создании пайплайна; превышение лимита — нарушение `VUID-RuntimeSpirv-x-06429..06432`
/// (неопределённое поведение, а не ошибка Vulkan). Поэтому размер проверяется заранее
/// ([`LocalSize::validate_for_device`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkgroupSizeLimits {
    /// `maxComputeWorkGroupInvocations`: наибольшее произведение x·y·z (спецификация гарантирует ≥ 128).
    pub max_invocations: u32,
    /// `maxComputeWorkGroupSize`: наибольший размер по каждой оси.
    pub max_size: [u32; 3],
}

/// Размер локальной рабочей группы по X и Y (Z = 1). Ноль невозможен по построению.
///
/// Это тот же размер, что шейдер получает через `local_size_x_id`/`local_size_y_id`: вызывающий
/// передаёт его specialization-константами и им же считает число групп — одно число в двух местах
/// не разъедется.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalSize {
    /// Нитей по X.
    pub x: NonZeroU32,
    /// Нитей по Y.
    pub y: NonZeroU32,
}

impl LocalSize {
    /// Из чисел; ноль по любой оси — `InvalidArgument`.
    pub fn new(x: u32, y: u32) -> Result<Self, GpuError> {
        match (NonZeroU32::new(x), NonZeroU32::new(y)) {
            (Some(x), Some(y)) => Ok(Self { x, y }),
            _ => Err(GpuError::InvalidArgument {
                what: format!("размер рабочей группы {x}x{y}: ноль по оси"),
            }),
        }
    }

    /// Проверяет размер против лимитов устройства контекста (см. [`WorkgroupSizeLimits`]).
    /// Звать до создания пайплайна с этим размером: иначе шейдер с недопустимой группой — UB.
    pub fn validate_for_device(self, context: &GpuContext) -> Result<(), GpuError> {
        self.validate_against(context.compute_work_group_limits())
    }

    /// Чистая проверка против заданных лимитов (Z = 1).
    pub fn validate_against(self, limits: WorkgroupSizeLimits) -> Result<(), GpuError> {
        let (x, y) = (self.x.get(), self.y.get());
        let invocations = u64::from(x) * u64::from(y);
        let axis_fits = x <= limits.max_size[0] && y <= limits.max_size[1] && limits.max_size[2] >= 1;
        if !axis_fits || invocations > u64::from(limits.max_invocations) {
            return Err(GpuError::InvalidArgument {
                what: format!(
                    "рабочая группа {x}x{y} ({invocations} нитей) вне лимитов устройства: размер ≤ {:?}, нитей ≤ {}",
                    limits.max_size, limits.max_invocations
                ),
            });
        }
        Ok(())
    }
}

/// Число рабочих групп dispatch по осям.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkgroupCount {
    /// Групп по X.
    pub x: u32,
    /// Групп по Y.
    pub y: u32,
    /// Групп по Z.
    pub z: u32,
}

impl WorkgroupCount {
    /// Сколько групп нужно, чтобы покрыть картинку `width × height` группами `local`: деление с
    /// округлением вверх. Крайние группы частично выходят за картинку — шейдер обязан проверять
    /// границы (`if (x >= width) return;`).
    pub fn covering(width: u32, height: u32, local: LocalSize) -> Self {
        Self {
            x: width.div_ceil(local.x.get()),
            y: height.div_ceil(local.y.get()),
            z: 1,
        }
    }

    /// Пустой ли dispatch (ноль групп хотя бы по одной оси — шейдер не запустится ни разу).
    pub fn is_empty(&self) -> bool {
        self.x == 0 || self.y == 0 || self.z == 0
    }

    /// Проверяет число групп против лимита устройства `maxComputeWorkGroupCount`.
    pub(crate) fn validate_against(&self, limits: [u32; 3]) -> Result<(), GpuError> {
        let counts = [self.x, self.y, self.z];
        if counts.iter().zip(limits).any(|(count, limit)| *count > limit) {
            return Err(GpuError::InvalidArgument {
                what: format!("dispatch {counts:?} групп превышает лимит устройства {limits:?}"),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn covering_rounds_up_partial_groups() {
        let local = LocalSize::new(16, 8).unwrap();
        assert_eq!(
            WorkgroupCount::covering(32, 16, local),
            WorkgroupCount { x: 2, y: 2, z: 1 }
        );
        assert_eq!(
            WorkgroupCount::covering(33, 17, local),
            WorkgroupCount { x: 3, y: 3, z: 1 }
        );
        assert_eq!(
            WorkgroupCount::covering(1, 1, local),
            WorkgroupCount { x: 1, y: 1, z: 1 }
        );
        // 4K-вход видоискателя: 4096×3072 при 16×16 → 256×192 групп.
        let square = LocalSize::new(16, 16).unwrap();
        assert_eq!(
            WorkgroupCount::covering(4096, 3072, square),
            WorkgroupCount { x: 256, y: 192, z: 1 }
        );
    }

    #[test]
    fn zero_sized_image_gives_empty_dispatch_and_zero_local_size_is_rejected() {
        let local = LocalSize::new(8, 8).unwrap();
        assert!(WorkgroupCount::covering(0, 10, local).is_empty());
        assert!(!WorkgroupCount::covering(1, 1, local).is_empty());
        assert!(LocalSize::new(0, 8).is_err());
        assert!(LocalSize::new(8, 0).is_err());
    }

    #[test]
    fn local_size_is_checked_against_axis_and_invocation_limits() {
        // Минимумы спецификации: 128 нитей, 128×128×64.
        let minimal = WorkgroupSizeLimits {
            max_invocations: 128,
            max_size: [128, 128, 64],
        };
        assert!(LocalSize::new(16, 8).unwrap().validate_against(minimal).is_ok());
        assert!(
            LocalSize::new(16, 16).unwrap().validate_against(minimal).is_err(),
            "256 нитей > 128"
        );
        assert!(
            LocalSize::new(256, 1).unwrap().validate_against(minimal).is_err(),
            "ось X больше лимита при допустимом числе нитей"
        );
        let generous = WorkgroupSizeLimits {
            max_invocations: 1024,
            max_size: [1024, 1024, 1024],
        };
        assert!(LocalSize::new(32, 32).unwrap().validate_against(generous).is_ok());
        assert!(LocalSize::new(64, 32).unwrap().validate_against(generous).is_err());
    }

    #[test]
    fn device_limit_is_enforced_per_axis() {
        let count = WorkgroupCount { x: 10, y: 70_000, z: 1 };
        assert!(count.validate_against([65_535, 65_535, 65_535]).is_err());
        assert!(count.validate_against([65_535, 70_000, 1]).is_ok());
    }
}
