//! Карточка белого в разделе калибровки (решения 292, 294): нижняя карточка на материале раздела, как карточка
//! пресетов.
//!
//! # Что здесь своё, а что общее
//! Содержимое — те же кирпичи, что у карточки белого PHOTO: числа осей, шкала Кельвинов и оттенка, ряд «Phone» и
//! света, «×» (`WhiteCardWidget` и его части). Здесь только раздельный дом ([`WhiteCardSite::section`]): глазная
//! геометрия, свои номера кнопок, поворот шкалы в глазные оси и ряд БЕЗ «Auto» (решение 294). Стекло и проявление
//! на месте — общий [`CardMorph`] с карточкой пресетов.
//!
//! # Владение и инварианты
//! * Открыта ли карточка, решает `CalibrationState::card() == White`; виджет выравнивается по нему без эха.
//! * Белое экран не хранит: шкалы встают на факты камеры; ручная правка и «Phone» уходят намерениями, а выключает
//!   «Auto» движок по прежним правилам white-balance.
//! * Касания живут только пока карточка открыта; закрывающаяся карточка дорисовывается без касаний.

use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::time::MonotonicNanos;

use super::card_morph::{CardMorph, ContentLook, FollowEffect};
use crate::calibration::SectionCard;
use crate::widget::calibration::host::RootHost;
use crate::widget::calibration::space::SectionSpace;
use crate::widget::error::WidgetError;
use crate::widget::geometry::ScreenGeometry;
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::white_card::WhiteCardWidget;
use crate::widget::white_card_site::WhiteCardSite;
use crate::widget::{Ctx, Outputs};
use ui_core::id::NodeId;

/// Карточка белого раздела.
pub(super) struct SectionWhiteCard {
    content: WhiteCardWidget,
    morph: CardMorph,
}

impl SectionWhiteCard {
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (eye, space): (&ScreenGeometry, &SectionSpace),
        root: NodeId,
    ) -> Result<Self, WidgetError> {
        let content = WhiteCardWidget::build(kit, ctx, root, WhiteCardSite::section(eye, space))?;
        Ok(Self {
            content,
            morph: CardMorph::new(eye.white_card()),
        })
    }

    /// Рамка карточки в системе корня.
    pub(super) fn rect(&self) -> Rect {
        self.morph.rect()
    }

    /// Видна ли карточка (открыта, открывается или растворяется).
    pub(super) fn is_visible(&self, ctx: &Ctx<'_>) -> bool {
        self.morph.is_visible(ctx)
    }

    /// Событие жеста карточки. `false` — событие не её.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        ctx: &Ctx<'_>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        self.content.handle_gesture(kit, event, ctx.state.white().axis(), out)
    }

    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let wanted = ctx.state.calibration().card() == SectionCard::White;
        let effect = self.morph.follow(ctx, wanted)?;
        // Шкалы и «Phone» следят за фактами всегда; касания включаются, только пока карточка открыта.
        self.content.sync(kit, ctx, effect == FollowEffect::Open)
    }

    pub(super) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) {
        self.morph.advance(ctx);
        self.content.advance(ctx, out);
    }

    /// Нужны ли кадры: идёт мерцание ожидания или тряска «Phone».
    pub(super) fn is_animating(&self, now: MonotonicNanos) -> bool {
        self.content.is_animating(now)
    }

    /// Сколько значений шкал не стали белым (диагностика).
    pub(super) fn invalid_values(&self) -> u64 {
        self.content.invalid_values()
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        (visuals, host): (&VisualIndex, &RootHost),
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
    ) -> Result<(), WidgetError> {
        let Some(ContentLook { alpha, slide_px }) = self.morph.draw_glass(ctx, host, layers)? else {
            return Ok(());
        };
        self.content.draw(ctx, visuals, layers, texts, (alpha, slide_px))
    }
}
