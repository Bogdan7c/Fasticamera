//! K2: содержимое кнопки — иконка и подпись по центру хоста; свёрстанные подписи из токенов.

use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, IconItem, TextItem};
use ui_draw::geometry::PxSize;
use ui_draw::icons::IconId;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::text::{TextRun, layout_line};
use ui_draw::transform::Affine2;

use super::error::ViewError;
use super::scale::PxPerPt;
use crate::tokens::{TextRoleKind, TypographyTokens};

/// Верстальщик подписей: роль текста из токенов + масштаб экрана.
#[derive(Debug, Clone, Copy)]
pub struct LabelPainter<'a> {
    typography: &'a TypographyTokens,
    scale: PxPerPt,
}

impl<'a> LabelPainter<'a> {
    pub fn new(typography: &'a TypographyTokens, scale: PxPerPt) -> Self {
        Self { typography, scale }
    }

    /// Свёрстать строку в роли `role`. Символ вне запечённого набора — [`ViewError::Text`].
    pub fn layout(&self, text: &str, role: TextRoleKind) -> Result<TextRun, ViewError> {
        let style = self.typography.role(role).style(self.scale.get());
        Ok(layout_line(text, &style)?)
    }
}

/// Прозрачность размещения узла (уже перемножена с предками) как проверенный `Opacity`.
pub fn opacity_of_placement(placement: &NodePlacementPx, what: &'static str) -> Result<Opacity, ViewError> {
    Opacity::new(placement.opacity).map_err(|_| ViewError::InvalidOpacity {
        what,
        value: placement.opacity,
    })
}

/// Подпись по центру хоста, выровненная по высоте заглавных (`assets.md` раздел 2): начало строки —
/// левый конец базовой линии, поэтому сдвиг на половину ширины влево и половину высоты заглавных
/// вниз. `host` — преобразование хоста (члена группы или узла), в нём уже есть поворот узла.
pub fn centered_text(run: &TextRun, host: Affine2, color: DisplayColor, opacity: Opacity) -> DrawItem {
    let to_baseline_origin = Affine2::translate(-run.advance_px * 0.5, run.cap_height_px * 0.5);
    DrawItem::Text(TextItem {
        run: run.clone(),
        transform: to_baseline_origin.then(host),
        color,
        opacity,
    })
}

/// Иконка по центру хоста; сторона квадрата — `size_pt`.
pub fn centered_icon(
    icon: IconId,
    size_pt: f64,
    scale: PxPerPt,
    host: Affine2,
    color: DisplayColor,
    opacity: Opacity,
) -> DrawItem {
    let side_px = scale.px(size_pt);
    DrawItem::Icon(IconItem {
        icon,
        transform: host,
        size_px: PxSize::new(side_px, side_px),
        color,
        opacity,
    })
}

/// Содержимое кнопки.
#[derive(Debug, Clone)]
pub enum ButtonContent {
    Icon { icon: IconId, size_pt: f64 },
    Label { run: TextRun },
}

/// Содержимое кнопки по центру хоста.
pub fn button_content_item(
    content: &ButtonContent,
    scale: PxPerPt,
    host: Affine2,
    color: DisplayColor,
    opacity: Opacity,
) -> DrawItem {
    match content {
        ButtonContent::Icon { icon, size_pt } => centered_icon(*icon, *size_pt, scale, host, color, opacity),
        ButtonContent::Label { run } => centered_text(run, host, color, opacity),
    }
}
