//! Работы группы `look_parity`: паритет GPU с CPU-эталоном `isp-model` НА ТЕЛЕФОНЕ (сессия 14o): четыре основы тона на
//! четырёх видах — ручки продукта, полосы оттенка на краях, образец колёс, образец ручек тона.
//!
//! Работа — тот же слитый проход видоискателя с видом (цена строки в таблице не важна и в выводах не участвует), а
//! главное — число после неё: худшая ошибка математики сверх шага хранения на худших синтетических сценах
//! (`isp_vulkan::saturation::parity`) против допуска 1e-5. Порядок строк — основа за основой.

use look_contract::ToneBaseModel;

use super::*;
use crate::saturation_bench::look_variant::ParityCase;

const fn parity(base: ToneBaseModel, case: ParityCase) -> WorkloadKind {
    WorkloadKind::LookParity {
        look: ParityLook { base, case },
    }
}

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "look_parity_agx_product",
        kind: parity(ToneBaseModel::Agx, ParityCase::ProductLook),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа agx, ручки активного пресета продукта",
    },
    CatalogEntry {
        name: "look_parity_agx_bands",
        kind: parity(ToneBaseModel::Agx, ParityCase::BandExtremes),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа agx, все полосы оттенка на краях ручек (стык круга)",
    },
    CatalogEntry {
        name: "look_parity_agx_wheels",
        kind: parity(ToneBaseModel::Agx, ParityCase::WheelsSample),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа agx, образец колёс цвета",
    },
    CatalogEntry {
        name: "look_parity_agx_tone",
        kind: parity(ToneBaseModel::Agx, ParityCase::ToneSample),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа agx, образец ручек тона 14l",
    },
    CatalogEntry {
        name: "look_parity_aces_product",
        kind: parity(ToneBaseModel::Aces, ParityCase::ProductLook),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа aces, ручки активного пресета продукта",
    },
    CatalogEntry {
        name: "look_parity_aces_bands",
        kind: parity(ToneBaseModel::Aces, ParityCase::BandExtremes),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа aces, все полосы оттенка на краях ручек (стык круга)",
    },
    CatalogEntry {
        name: "look_parity_aces_wheels",
        kind: parity(ToneBaseModel::Aces, ParityCase::WheelsSample),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа aces, образец колёс цвета",
    },
    CatalogEntry {
        name: "look_parity_aces_tone",
        kind: parity(ToneBaseModel::Aces, ParityCase::ToneSample),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа aces, образец ручек тона 14l",
    },
    CatalogEntry {
        name: "look_parity_filmic_product",
        kind: parity(ToneBaseModel::Filmic, ParityCase::ProductLook),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа filmic, ручки активного пресета продукта",
    },
    CatalogEntry {
        name: "look_parity_filmic_bands",
        kind: parity(ToneBaseModel::Filmic, ParityCase::BandExtremes),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа filmic, все полосы оттенка на краях ручек (стык круга)",
    },
    CatalogEntry {
        name: "look_parity_filmic_wheels",
        kind: parity(ToneBaseModel::Filmic, ParityCase::WheelsSample),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа filmic, образец колёс цвета",
    },
    CatalogEntry {
        name: "look_parity_filmic_tone",
        kind: parity(ToneBaseModel::Filmic, ParityCase::ToneSample),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа filmic, образец ручек тона 14l",
    },
    CatalogEntry {
        name: "look_parity_legacy_product",
        kind: parity(ToneBaseModel::Legacy, ParityCase::ProductLook),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа legacy, ручки активного пресета продукта",
    },
    CatalogEntry {
        name: "look_parity_legacy_bands",
        kind: parity(ToneBaseModel::Legacy, ParityCase::BandExtremes),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа legacy, все полосы оттенка на краях ручек (стык круга)",
    },
    CatalogEntry {
        name: "look_parity_legacy_wheels",
        kind: parity(ToneBaseModel::Legacy, ParityCase::WheelsSample),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа legacy, образец колёс цвета",
    },
    CatalogEntry {
        name: "look_parity_legacy_tone",
        kind: parity(ToneBaseModel::Legacy, ParityCase::ToneSample),
        group: WorkloadGroup::LookParity,
        summary: "паритет на телефоне: основа legacy, образец ручек тона 14l",
    },
];
