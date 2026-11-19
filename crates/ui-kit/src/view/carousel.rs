//! K4: вид карусели — подписи элементов по `placements(now)`; выбранный выделен цветом.

use ui_core::time::MonotonicNanos;
use ui_draw::color::Opacity;
use ui_draw::draw_list::DrawItem;
use ui_draw::text::TextRun;
use ui_draw::transform::Affine2;

use super::content::{LabelPainter, centered_text};
use super::error::ViewError;
use super::scale::PxPerPt;
use crate::carousel::SnapCarousel;
use crate::tokens::{ColorTokens, TextRoleKind};

/// Свёрстанные подписи элементов карусели: по одной на элемент, в порядке списка карусели.
/// Верстаются один раз (при создании или `set_entries`), а не каждый кадр; роль текста — `ModeLabel`.
#[derive(Debug, Clone)]
pub struct CarouselLabels {
    runs: Vec<TextRun>,
}

impl CarouselLabels {
    /// Свёрстать подписи `texts` ролью `ModeLabel`. Символ вне запечённого набора — [`ViewError::Text`].
    pub fn layout(painter: &LabelPainter<'_>, texts: &[&str]) -> Result<Self, ViewError> {
        let runs = texts
            .iter()
            .map(|text| painter.layout(text, TextRoleKind::ModeLabel))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { runs })
    }

    /// Ширина подписи элемента `index` (с трекингом), px — по ней экран считает ширины
    /// [`crate::carousel::CarouselEntry`]. `None`, если такого элемента нет.
    pub fn advance_px(&self, index: usize) -> Option<f32> {
        self.runs.get(index).map(|run| run.advance_px)
    }

    pub fn len(&self) -> usize {
        self.runs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }
}

/// Подписи карусели в момент `now`. `host` — преобразование узла-хоста (центр капсулы режимов,
/// в нём уже есть поворот и масштаб нажатия), `opacity` — его прозрачность. Каждая подпись стоит
/// на `center_offset_points` от центра хоста вдоль горизонтальной оси; выбранный
/// элемент ([`SnapCarousel::selected_index`]) — цветом `selected_label`, остальные — приглушённым
/// `text_muted`. Во время свайпа выбранным остаётся прежний элемент, пока карусель не сменит выбор.
///
/// Число подписей обязано совпадать с числом элементов карусели: несовпадение — ошибка, а не
/// молчаливая обрезка.
pub fn carousel_items<T: Clone + PartialEq + std::fmt::Debug>(
    host: Affine2,
    opacity: Opacity,
    carousel: &SnapCarousel<T>,
    now: MonotonicNanos,
    labels: &CarouselLabels,
    px_per_pt: PxPerPt,
    colors: &ColorTokens,
) -> Result<Vec<DrawItem>, ViewError> {
    let placements = carousel.placements(now);
    if placements.len() != labels.runs.len() {
        return Err(ViewError::CarouselLabelsMismatch {
            items: placements.len(),
            labels: labels.runs.len(),
        });
    }
    let selected = carousel.selected_index();
    Ok(placements
        .iter()
        .zip(&labels.runs)
        .map(|(placement, run)| {
            let color = if placement.index == selected {
                colors.selected_label
            } else {
                colors.text_muted
            };
            let shift = Affine2::translate(px_per_pt.px(placement.center_offset_points), 0.0);
            centered_text(run, shift.then(host), color, opacity)
        })
        .collect())
}
