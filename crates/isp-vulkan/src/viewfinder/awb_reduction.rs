//! Способ сведения статистики AWB внутри рабочей группы (выбор SPIR-V варианта ISP видоискателя).
//!
//! Контракт сумм один — `isp_model::statistics` (целые u32, порядок сложения не влияет на результат),
//! меняется только цена:
//! * [`AwbReduction::SharedMemoryWindow`] — по умолчанию. Если ВСЕ группы кадра лежат каждая в одной плитке
//!   (типичный случай: плитки по 64×64 ячейки против группы 4×32; решает CPU, см. `awb_group_layout`),
//!   шейдер идёт коротким путём: вклады нитей — `atomicAdd` в пять слов shared-памяти, одна запись в
//!   глобальный буфер на поле на группу. Иначе — общий путь: shared-окно 4×4 плитки с тремя `barrier()`.
//!   На Adreno 740 кадр со статистикой стоит на ~5 мс дешевле, чем через subgroup (сессия 16a, пакет I);
//! * [`AwbReduction::SubgroupSingleTile`] — группа целиком в одной плитке складывает поля `subgroupAdd` и делает
//!   по одному глобальному `atomicAdd` на поле на subgroup; группа на стыке плиток — как общий путь. Оставлен
//!   как выбор конфига (RADV и другие устройства, где он может быть выгоднее): на Adreno он дороже.
//!
//! Конфиг задаёт предпочтение ([`AwbReductionPolicy`]), устройство — возможность
//! ([`SubgroupArithmeticSupport`]); итог решается один раз при создании `ViewfinderIsp`.

use gpu_vulkan::context::GpuCapabilities;

/// Предпочтение из конфига.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AwbReductionPolicy {
    /// Быстрый путь через subgroup, если устройство умеет арифметику subgroup в compute; иначе shared-окно.
    /// Не по умолчанию: на Adreno 740 дороже shared-памяти (замер 16a).
    SubgroupWhenSupported,
    /// Shared-память: короткий путь для кадров, где группа не пересекает плитки, иначе общее окно плиток.
    #[default]
    SharedMemoryOnly,
}

/// Умеет ли устройство арифметику subgroup в compute-шейдерах.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubgroupArithmeticSupport {
    /// `subgroupSupportedOperations` ∋ ARITHMETIC и `subgroupSupportedStages` ∋ COMPUTE.
    Supported,
    /// Нет — SPIR-V с `GroupNonUniformArithmetic` на этом устройстве создавать нельзя.
    Unsupported,
}

impl SubgroupArithmeticSupport {
    /// Возможность устройства по отчёту `gpu-vulkan`.
    pub fn of_device(capabilities: &GpuCapabilities) -> Self {
        if capabilities.subgroup_arithmetic_in_compute {
            Self::Supported
        } else {
            Self::Unsupported
        }
    }
}

/// Выбранный способ сведения (определяет SPIR-V вариант).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AwbReduction {
    /// Быстрый путь `subgroupAdd` для групп в одной плитке + shared-окно для групп на стыке.
    SubgroupSingleTile,
    /// Только shared-окно.
    SharedMemoryWindow,
}

impl AwbReductionPolicy {
    /// Итоговый способ: subgroup — только если его хочет конфиг И умеет устройство.
    pub fn resolve(self, device_support: SubgroupArithmeticSupport) -> AwbReduction {
        match (self, device_support) {
            (Self::SubgroupWhenSupported, SubgroupArithmeticSupport::Supported) => AwbReduction::SubgroupSingleTile,
            (Self::SubgroupWhenSupported, SubgroupArithmeticSupport::Unsupported) | (Self::SharedMemoryOnly, _) => {
                AwbReduction::SharedMemoryWindow
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subgroup_path_needs_both_policy_and_device_support() {
        use SubgroupArithmeticSupport::{Supported, Unsupported};
        assert_eq!(
            AwbReductionPolicy::SubgroupWhenSupported.resolve(Supported),
            AwbReduction::SubgroupSingleTile
        );
        assert_eq!(
            AwbReductionPolicy::SubgroupWhenSupported.resolve(Unsupported),
            AwbReduction::SharedMemoryWindow,
            "без арифметики subgroup — запасной SPIR-V, иначе создание модуля нарушило бы спецификацию"
        );
        assert_eq!(
            AwbReductionPolicy::SharedMemoryOnly.resolve(Supported),
            AwbReduction::SharedMemoryWindow
        );
        assert_eq!(
            AwbReductionPolicy::SharedMemoryOnly.resolve(Unsupported),
            AwbReduction::SharedMemoryWindow
        );
    }

    #[test]
    fn policy_reads_from_snake_case_json_and_defaults_to_shared_memory() {
        assert_eq!(AwbReductionPolicy::default(), AwbReductionPolicy::SharedMemoryOnly);
        let parsed: AwbReductionPolicy = serde_json::from_str(r#""shared_memory_only""#).unwrap();
        assert_eq!(parsed, AwbReductionPolicy::SharedMemoryOnly);
        assert!(serde_json::from_str::<AwbReductionPolicy>(r#""subgroup""#).is_err());
    }
}
