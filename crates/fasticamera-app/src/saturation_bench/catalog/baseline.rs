//! Работы группы `baseline`: первые 14 работ сессии 16a. Состав и порядок НЕ меняются — это умолчание списка.
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "viewfinder",
        kind: viewfinder(ViewfinderVariant::Product),
        group: WorkloadGroup::Baseline,
        summary: "ISP видоискателя: RAW10 4096x3072, все стадии, AWB каждый 4-й кадр",
    },
    CatalogEntry {
        name: "scopes_waveform",
        kind: scopes(ScopeShape::Waveform, ScopeInputKind::Natural),
        group: WorkloadGroup::Baseline,
        summary: "прибор waveform на сцене",
    },
    CatalogEntry {
        name: "scopes_parade",
        kind: scopes(ScopeShape::Parade, ScopeInputKind::Natural),
        group: WorkloadGroup::Baseline,
        summary: "прибор RGB parade на сцене",
    },
    CatalogEntry {
        name: "scopes_vectorscope_ycbcr",
        kind: scopes(ScopeShape::VectorscopeYcbcr, ScopeInputKind::Natural),
        group: WorkloadGroup::Baseline,
        summary: "вектороскоп Cb/Cr на сцене",
    },
    CatalogEntry {
        name: "scopes_vectorscope_oklab",
        kind: scopes(ScopeShape::VectorscopeOklab, ScopeInputKind::Natural),
        group: WorkloadGroup::Baseline,
        summary: "вектороскоп a/b OKLab на сцене",
    },
    CatalogEntry {
        name: "scopes_parade_gray",
        kind: scopes(ScopeShape::Parade, ScopeInputKind::UniformGray),
        group: WorkloadGroup::Baseline,
        summary: "parade на однородном сером (худший случай atomicAdd)",
    },
    CatalogEntry {
        name: "scopes_vectorscope_gray",
        kind: scopes(ScopeShape::VectorscopeYcbcr, ScopeInputKind::UniformGray),
        group: WorkloadGroup::Baseline,
        summary: "вектороскоп на однородном сером (худший случай atomicAdd)",
    },
    CatalogEntry {
        name: "compositor",
        kind: WorkloadKind::Compositor,
        group: WorkloadGroup::Baseline,
        summary: "компоновщик: картинка видоискателя в цель экрана",
    },
    CatalogEntry {
        name: "display_photo",
        kind: display(DisplayScreen::Photo, BackdropRhythm::NewFrameEveryFrame),
        group: WorkloadGroup::Baseline,
        summary: "кадр показа экрана PHOTO, фон новый каждый кадр",
    },
    CatalogEntry {
        name: "display_photo_recompose",
        kind: display(DisplayScreen::Photo, BackdropRhythm::SameFrame),
        group: WorkloadGroup::Baseline,
        summary: "кадр показа экрана PHOTO, фон тот же кадр",
    },
    CatalogEntry {
        name: "display_calibrate",
        kind: display(DisplayScreen::Calibrate, BackdropRhythm::NewFrameEveryFrame),
        group: WorkloadGroup::Baseline,
        summary: "кадр показа раздела CALIBRATE с живой картинкой прибора, фон новый каждый кадр",
    },
    CatalogEntry {
        name: "display_calibrate_recompose",
        kind: display(DisplayScreen::Calibrate, BackdropRhythm::SameFrame),
        group: WorkloadGroup::Baseline,
        summary: "кадр показа раздела CALIBRATE, фон тот же кадр",
    },
    CatalogEntry {
        name: "headroom_v4",
        kind: WorkloadKind::HeadroomV4,
        group: WorkloadGroup::Baseline,
        summary: "стенд V4 4096x2304 без шумодава",
    },
    CatalogEntry {
        name: "still",
        kind: WorkloadKind::Still,
        group: WorkloadGroup::Baseline,
        summary: "проявление снимка целиком",
    },
];
