//! Диск «↻» раздела калибровки: один рисунок на строки-линейки и на кнопки колеса (решение 207).
//!
//! Яркая — орган управления отличается от значения пресета; приглушённая (`card.reset_dim_opacity`, 44 %) — как в
//! пресете (решение 196). Вид кнопки заимствован у карточки Exposure (`widgets.card`), числа здесь не дублируются.

use ui_draw::color::DisplayColor;
use ui_draw::icons::IconId;
use ui_kit::view::centered_icon;

use super::paint::NodePaint;
use crate::widget::Ctx;
use crate::widget::layers::{DrawLayers, Layer};

/// Рисует «↻» с центром в `center_pt` (смещение от центра узла кисти). `differs` — яркая или приглушённая.
pub(super) fn draw_reset_disc(
    ctx: &Ctx<'_>,
    paint: &NodePaint,
    layers: &mut DrawLayers,
    center_pt: (f64, f64),
    differs: bool,
) {
    let card = &ctx.geometry.config().card;
    let diameter_pt = ctx.config.calibration.widgets.reset_button_diameter_pt;
    let dim = if differs { 1.0 } else { card.reset_dim_opacity };
    let paint = paint.dimmed(dim);
    let disc_color = DisplayColor {
        a: card.button_disc_alpha,
        ..DisplayColor::BLACK
    };
    layers.push(
        Layer::Content,
        paint.disc(center_pt.0, center_pt.1, diameter_pt, disc_color),
    );
    let icon_size_pt = card.button_icon_size_pt * diameter_pt / card.button_diameter_pt;
    layers.push(
        Layer::Content,
        centered_icon(
            IconId::Reset,
            icon_size_pt,
            ctx.px,
            paint.at(center_pt.0, center_pt.1),
            ctx.tokens.colors.text_primary,
            paint.opacity(),
        ),
    );
}
