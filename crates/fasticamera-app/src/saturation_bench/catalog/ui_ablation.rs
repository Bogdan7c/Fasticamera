//! Работы группы `ui_ablation`: тот же настоящий список рисования экрана, но один вид элементов.
//!
//! Срезов по стеклу нет: после 14m в разделе, а после 14p и в PHOTO стекла нет (решения 261, 297), «только стекло»
//! было бы пустым списком, а «всё, кроме стекла» совпало бы с полным экраном. У PHOTO — те же срезы по фигурам
//! (поверхности с ободком, кольцо затвора) и глифам, что у раздела.
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
        name: "display_photo_shapes_only",
        kind: ablated_display(DisplayScreen::Photo, DrawFilter::ShapesOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ PHOTO: только фигуры (поверхности с ободком, кольцо и диск затвора)",
    },
    CatalogEntry {
        name: "display_photo_text_only",
        kind: ablated_display(DisplayScreen::Photo, DrawFilter::TextOnly),
        group: WorkloadGroup::UiAblation,
        summary: "показ PHOTO: только текст и значки (глифы)",
    },
];
