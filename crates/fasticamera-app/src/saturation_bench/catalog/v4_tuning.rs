//! Работы группы `v4_tuning`: опыты над цепочкой V4 «запас под видео» (сессия 16a, пакет V) — форма группы, пробы
//! вместо проходов Menon, отключение стадий вывода. Ни одна из них не даёт осмысленной картинки.
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;
use crate::saturation_bench::headroom_tuning::{
    DemosaicChoice, HeadroomExperiment, OutputSkipChoice, StabilizerChoice, ToneModelChoice,
};

const PRODUCT: HeadroomExperiment = HeadroomExperiment::PRODUCT;

const fn v4(experiment: HeadroomExperiment) -> WorkloadKind {
    WorkloadKind::HeadroomV4Tuned(experiment)
}

/// Продукт без одной стадии вывода.
const fn skip(skips: OutputSkipChoice) -> WorkloadKind {
    v4(PRODUCT.skip_output(skips))
}

const NONE: OutputSkipChoice = OutputSkipChoice::NONE;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    // Пробы вместо Menon: чем заняты 8 проходов — математикой или памятью.
    CatalogEntry {
        name: "v4_probe_empty",
        kind: v4(PRODUCT.demosaic(DemosaicChoice::ProbeEmpty)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: проходы Menon пустые (цена запуска групп и барьеров)",
    },
    CatalogEntry {
        name: "v4_probe_copy_f32",
        kind: v4(PRODUCT.demosaic(DemosaicChoice::ProbeCopyF32)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: проходы Menon только читают и пишут плоскости f32 (цена трафика)",
    },
    CatalogEntry {
        name: "v4_probe_copy_half",
        kind: v4(PRODUCT.demosaic(DemosaicChoice::ProbeCopyHalf)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: проходы Menon читают и пишут плоскости fp16 (цена трафика вдвое меньшего)",
    },
    // Стадии вывода: что из 20 мс чем занято.
    CatalogEntry {
        name: "v4_skip_tone_base",
        kind: skip(OutputSkipChoice {
            tone_base: true,
            ..NONE
        }),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод без основы тона AgX",
    },
    CatalogEntry {
        name: "v4_skip_look",
        kind: skip(OutputSkipChoice { look: true, ..NONE }),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод без вида (цвет и таблица)",
    },
    CatalogEntry {
        name: "v4_skip_output_gamut",
        kind: skip(OutputSkipChoice {
            output_gamut: true,
            ..NONE
        }),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод без выходного сжатия охвата",
    },
    CatalogEntry {
        name: "v4_skip_oetf",
        kind: skip(OutputSkipChoice { oetf: true, ..NONE }),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод без OETF",
    },
    CatalogEntry {
        name: "v4_skip_all_output",
        kind: skip(OutputSkipChoice {
            camera_matrix: true,
            tone_base: true,
            look: true,
            output_gamut: true,
            oetf: true,
        }),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод без всех стадий (остаётся чтение трёх плоскостей и запись картинки)",
    },
    // Модель основы тона зафиксирована в пайплайне вывода: остальные ветки выброшены (картинка прежняя, если модель AgX).
    CatalogEntry {
        name: "v4_agx_only",
        kind: v4(PRODUCT.tone_model(ToneModelChoice::AgxOnly)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод собран только под AgX (остальные модели выброшены из шейдера)",
    },
    // Точные приёмы вместе (картинка бит в бит прежняя): вывод только под AgX + форма группы всех проходов.
    CatalogEntry {
        name: "v4_agx_wg_128x2",
        kind: v4(PRODUCT.tone_model(ToneModelChoice::AgxOnly).workgroup(128, 2)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод только под AgX + группа 128x2",
    },
    CatalogEntry {
        name: "v4_agx_wg_32x8",
        kind: v4(PRODUCT.tone_model(ToneModelChoice::AgxOnly).workgroup(32, 8)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод только под AgX + группа 32x8",
    },
    CatalogEntry {
        name: "v4_agx_wg_64x4",
        kind: v4(PRODUCT.tone_model(ToneModelChoice::AgxOnly).workgroup(64, 4)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод только под AgX + группа 64x4",
    },
    // Макет перепроекции стабилизатора, слитой с выводом (оценка цены; поворот 0,3° и сдвиг строки до 3 px).
    CatalogEntry {
        name: "v4_stabilizer_mock",
        kind: v4(PRODUCT.stabilizer(StabilizerChoice::MergedWithOutput)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: вывод с макетом стабилизатора (билинейная перепроекция кадра, слитая с выводом)",
    },
    // Форма рабочей группы (все проходы цепочки; продукт — 16x16).
    CatalogEntry {
        name: "v4_wg_8x8",
        kind: v4(PRODUCT.workgroup(8, 8)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: группа 8x8",
    },
    CatalogEntry {
        name: "v4_wg_32x8",
        kind: v4(PRODUCT.workgroup(32, 8)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: группа 32x8",
    },
    CatalogEntry {
        name: "v4_wg_64x4",
        kind: v4(PRODUCT.workgroup(64, 4)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: группа 64x4",
    },
    CatalogEntry {
        name: "v4_wg_128x2",
        kind: v4(PRODUCT.workgroup(128, 2)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: группа 128x2",
    },
    CatalogEntry {
        name: "v4_wg_32x32",
        kind: v4(PRODUCT.workgroup(32, 32)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: группа 32x32",
    },
    CatalogEntry {
        name: "v4_wg_4x32",
        kind: v4(PRODUCT.workgroup(4, 32)),
        group: WorkloadGroup::V4Tuning,
        summary: "V4: группа 4x32",
    },
];
