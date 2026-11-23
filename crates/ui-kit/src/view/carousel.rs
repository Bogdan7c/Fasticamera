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
    carousel_items_in_window(host, opacity, carousel, now, labels, px_per_pt, colors, f64::INFINITY)
}

/// То же, но видна только часть карусели шириной `2 × half_window_points` вокруг центра хоста: подпись, вышедшая
/// за окно, гаснет по доле видимой части (в квадрате) и не рисуется совсем, когда целиком снаружи. Нужна там, где
/// карусель стоит внутри панели и соседние подписи не должны вылезать за её край (переключатель основы тона).
/// `f64::INFINITY` — окна нет: все подписи рисуются как есть.
#[allow(
    clippy::too_many_arguments,
    reason = "те же входы, что у `carousel_items`, плюс окно: связка «хост + прозрачность + элемент» не делится"
)]
pub fn carousel_items_in_window<T: Clone + PartialEq + std::fmt::Debug>(
    host: Affine2,
    opacity: Opacity,
    carousel: &SnapCarousel<T>,
    now: MonotonicNanos,
    labels: &CarouselLabels,
    px_per_pt: PxPerPt,
    colors: &ColorTokens,
    half_window_points: f64,
) -> Result<Vec<DrawItem>, ViewError> {
    let placements = carousel.placements(now);
    if placements.len() != labels.runs.len() {
        return Err(ViewError::CarouselLabelsMismatch {
            items: placements.len(),
            labels: labels.runs.len(),
        });
    }
    let selected = carousel.selected_index();
    let mut items = Vec::with_capacity(placements.len());
    for (placement, run) in placements.iter().zip(&labels.runs) {
        let color = if placement.index == selected {
            colors.selected_label
        } else {
            colors.text_muted
        };
        let visible = visible_fraction(
            placement.center_offset_points,
            placement.width_points,
            half_window_points,
        );
        if visible <= 0.0 {
            continue;
        }
        let shown = if visible >= 1.0 {
            opacity
        } else {
            let faded = opacity.get() * (visible * visible) as f32;
            Opacity::new(faded).map_err(|_| ViewError::InvalidOpacity {
                what: "carousel_label",
                value: faded,
            })?
        };
        let shift = Affine2::translate(px_per_pt.px(placement.center_offset_points), 0.0);
        items.push(centered_text(run, shift.then(host), color, shown));
    }
    Ok(items)
}

/// Какая доля подписи (по ширине) лежит в окне `±half_window_points` вокруг центра хоста: 1 — целиком внутри.
fn visible_fraction(center_offset_points: f64, width_points: f64, half_window_points: f64) -> f64 {
    if !half_window_points.is_finite() || width_points <= 0.0 {
        return 1.0;
    }
    let left = center_offset_points - width_points / 2.0;
    let right = center_offset_points + width_points / 2.0;
    let visible = right.min(half_window_points) - left.max(-half_window_points);
    (visible / width_points).clamp(0.0, 1.0)
}
