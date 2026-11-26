//! Работы группы `ui_ablation`: тот же настоящий список рисования экрана, но один вид элементов.
//!
//! Срезов по стеклу CALIBRATE нет: после 14m в разделе стекла нет, «только стекло» было бы пустым списком, а
//! «всё, кроме стекла» совпало бы с полным `display_calibrate`. Срезы по стеклу — только у PHOTO.
//!
//! Таблица — данные: что за работа, решает `WorkloadKind`, а не имя. Порядок строк — порядок прогона.

use super::*;

/// Строки группы в порядке прогона.
pub(super) const ENTRIES: &[CatalogEntry] = &[
    CatalogEntry {
        name: "display_calibrate_shapes_only",
        kind: ablated_display(DisplayScreen::Calibrate, DrawFilter::ShapesOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ CALIBRATE: только фигуры",
    },
    CatalogEntry {
        name: "display_calibrate_text_only",
        kind: ablated_display(DisplayScreen::Calibrate, DrawFilter::TextOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ CALIBRATE: только текст и значки (глифы)",
    },
    CatalogEntry {
        name: "display_calibrate_images_only",
        kind: ablated_display(DisplayScreen::Calibrate, DrawFilter::ImagesOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ CALIBRATE: только картинки (живая картинка прибора)",
    },
    CatalogEntry {
        name: "display_photo_glass_only",
        kind: ablated_display(DisplayScreen::Photo, DrawFilter::GlassOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ PHOTO: только стеклянные группы",
    },
    CatalogEntry {
        name: "display_photo_no_glass",
        kind: ablated_display(DisplayScreen::Photo, DrawFilter::NoGlass),
        group: WorkloadGroup::UiAblation,
        summary: "показ PHOTO: всё, кроме стекла",
    },
];
