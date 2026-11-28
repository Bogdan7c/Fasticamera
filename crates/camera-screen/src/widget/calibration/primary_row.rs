//! Строки-полоски каналов R, G, B (вид BARS, решение 343): номера слотов, подписи и цвет канала.
//!
//! Вынесено из `knob_row` отдельным модулем: сама строка-линейка общая со всеми ручками (та же шкала −200…+200, ход
//! 400 pt, «↻», метка пресета), а здесь только то, что знает про каналы.

use look_contract::PrimaryChannel;
use ui_draw::color::DisplayColor;

use super::knob_row::{RowTarget, WHEEL_RING_SLOT};
use super::paint::srgb8;
use crate::widget::Ctx;

/// Номер строки первой полоски (R) среди строк раздела: сразу после кольца яркости колеса ([`WHEEL_RING_SLOT`] = 12).
/// Дальше G и B подряд: 13, 14, 15. Зона полоски в номер НЕ входит — её решает вкладка, общая с колёсами.
pub(super) const FIRST_PRIMARY_SLOT: u8 = WHEEL_RING_SLOT + 1;

/// Номер строки полоски канала.
pub(super) fn slot_of(channel: PrimaryChannel) -> u8 {
    let index = PrimaryChannel::ALL
        .iter()
        .position(|known| *known == channel)
        .map_or(0, |index| index as u8);
    FIRST_PRIMARY_SLOT + index
}

/// Подпись строки полоски: одна буква канала.
pub(super) fn title_of(channel: PrimaryChannel) -> &'static str {
    match channel {
        PrimaryChannel::Red => "R",
        PrimaryChannel::Green => "G",
        PrimaryChannel::Blue => "B",
    }
}

/// Цвет канала для подписи строки; `None` — строка не полоска. Цвета — из конфига экрана (`bars.channel_colors`).
pub(super) fn accent_of(target: RowTarget, ctx: &Ctx<'_>) -> Option<DisplayColor> {
    let RowTarget::Primary(channel) = target else {
        return None;
    };
    let colors = &ctx.config.calibration.widgets.bars.channel_colors;
    Some(srgb8(colors.of(channel)))
}
