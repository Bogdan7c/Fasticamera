//! Работы группы `look_cost`: сколько мс при насыщении стоит вид картинки в слитом проходе видоискателя (сессия 14o).
//!
//! Каждая работа — тот же проход, что `viewfinder`, но с видом из рецепта продукта (`MeasurementLook`): меняется одна
//! вещь, разница двух работ и есть цена этой вещи. Порядок строк — порядок прогона и порядок пар «вкл против выкл»
//! (`compare_pairs`): ВЫКЛ-эталон стоит РАНЬШЕ своего варианта, и эталон не бывает вариантом.

use look_contract::ToneBaseModel;

use super::*;
use crate::measurement_look::MeasurementLook;

const fn look(look: MeasurementLook) -> WorkloadKind {
    WorkloadKind::ViewfinderLook {
        look: MeasuredLook(look),
    }
}

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "look_neutral",
        kind: look(MeasurementLook::NeutralOnProductBase),
        group: WorkloadGroup::LookCost,
        summary: "основа продукта, слой вида целиком в нуле (эталон пары с look_product)",
    },
    CatalogEntry {
        name: "look_product",
        kind: look(MeasurementLook::Product),
        group: WorkloadGroup::LookCost,
        summary: "рецепт продукта как есть (активный пресет владельца)",
    },
    CatalogEntry {
        name: "look_tone_base_legacy",
        kind: look(MeasurementLook::ToneBase(ToneBaseModel::Legacy)),
        group: WorkloadGroup::LookCost,
        summary: "ручки продукта на прежней основе тона (эталон пар с AgX, ACES, filmic)",
    },
    CatalogEntry {
        name: "look_tone_base_agx",
        kind: look(MeasurementLook::ToneBase(ToneBaseModel::Agx)),
        group: WorkloadGroup::LookCost,
        summary: "ручки продукта на основе AgX",
    },
    CatalogEntry {
        name: "look_tone_base_aces",
        kind: look(MeasurementLook::ToneBase(ToneBaseModel::Aces)),
        group: WorkloadGroup::LookCost,
        summary: "ручки продукта на основе ACES",
    },
    CatalogEntry {
        name: "look_tone_base_filmic",
        kind: look(MeasurementLook::ToneBase(ToneBaseModel::Filmic)),
        group: WorkloadGroup::LookCost,
        summary: "ручки продукта на основе filmic",
    },
    CatalogEntry {
        name: "look_without_wheels",
        kind: look(MeasurementLook::WithoutWheels),
        group: WorkloadGroup::LookCost,
        summary: "продукт без колёс цвета (эталон пары с look_wheels_sample)",
    },
    CatalogEntry {
        name: "look_wheels_sample",
        kind: look(MeasurementLook::WheelsSample),
        group: WorkloadGroup::LookCost,
        summary: "продукт с колёсами образца из конфига замеров",
    },
    CatalogEntry {
        name: "look_without_tone",
        kind: look(MeasurementLook::WithoutTone),
        group: WorkloadGroup::LookCost,
        summary: "продукт без ручек тона 14l (эталон пары с look_tone_sample)",
    },
    CatalogEntry {
        name: "look_tone_sample",
        kind: look(MeasurementLook::ToneSample),
        group: WorkloadGroup::LookCost,
        summary: "продукт с ручками тона образца из конфига замеров",
    },
];
