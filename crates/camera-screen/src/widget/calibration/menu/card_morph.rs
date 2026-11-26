//! Проявление нижней карточки раздела на месте (решения 283, 292): общая для карточки пресетов и карточки белого.
//!
//! Обе карточки — одно и то же стекло на материале раздела с одной пружиной (`MENU_MORPH_MOTION`), поэтому жизненный
//! цикл «открыть / закрыть / дорисовать растворение / забыть» принадлежит одному типу, а не копируется в каждую
//! карточку.
//!
//! # Инварианты
//! * Хочется ли открытой карточку, решает `ScreenState`; сюда это приходит как `wanted` в [`CardMorph::follow`].
//! * Пока карточка закрывается, состояние уже забыло о ней: `shown` держит её на экране до конца растворения.
//! * Идущий морф переиспользуется (открытие посреди закрытия — разворот с сохранением скорости).

use ui_core::geometry::Rect;
use ui_draw::draw_list::DrawItem;
use ui_kit::panel_morph::{PanelMorph, PanelMorphSpec, PanelPhase};
use ui_kit::view::panel_material_hosted;

use crate::widget::Ctx;
use crate::widget::SECTION_PANEL_MATERIAL;
use crate::widget::calibration::host::RootHost;
use crate::widget::error::{WidgetError, element};
use crate::widget::layers::{DrawLayers, Layer};
use crate::widget::tool_card::MENU_MORPH_MOTION;

/// Что `follow` сделал с морфом: нужно ли владельцу карточки что-то помнить.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FollowEffect {
    /// Карточка должна быть закрыта (или закрывается): касания надо снять.
    Closed,
    /// Карточка должна быть открыта: касания и содержимое живы.
    Open,
}

/// Содержимое карточки на этом кадре: прозрачность и въезд.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct ContentLook {
    /// Прозрачность содержимого 0..1.
    pub(super) alpha: f32,
    /// Въезд содержимого вдоль вертикали корня, px.
    pub(super) slide_px: f32,
}

/// Морф нижней карточки раздела.
pub(super) struct CardMorph {
    rect: Rect,
    morph: Option<PanelMorph>,
    open_wanted: bool,
    /// Карточка показана: нужно и во время закрытия, когда состояние уже забыло о ней.
    shown: bool,
}

impl CardMorph {
    pub(super) fn new(rect: Rect) -> Self {
        Self {
            rect,
            morph: None,
            open_wanted: false,
            shown: false,
        }
    }

    /// Рамка карточки в системе корня.
    pub(super) fn rect(&self) -> Rect {
        self.rect
    }

    /// Карточка проявляется на месте: рамка та же, пружина — меню Controls.
    fn new_morph(&self, ctx: &Ctx<'_>) -> Result<PanelMorph, WidgetError> {
        let card_config = &ctx.geometry.config().card;
        let sheet_config = &ctx.geometry.config().sheet;
        let spec = PanelMorphSpec::in_place(self.rect, card_config.corner_radius_pt, MENU_MORPH_MOTION)
            .with_content_slide_pt(sheet_config.open_content_slide_pt);
        PanelMorph::new(spec, &ctx.environment(self.rect.size.height), ctx.now).map_err(element("карточка раздела"))
    }

    /// Выравнивает морф по желанию состояния: `wanted` — должна ли карточка быть открыта.
    pub(super) fn follow(&mut self, ctx: &Ctx<'_>, wanted: bool) -> Result<FollowEffect, WidgetError> {
        if !wanted {
            if self.open_wanted {
                self.open_wanted = false;
                if let Some(morph) = &mut self.morph {
                    morph.close(ctx.now).map_err(element("карточка раздела"))?;
                }
            }
            return Ok(FollowEffect::Closed);
        }
        if !self.open_wanted {
            self.open_wanted = true;
            let reusable = self
                .morph
                .as_ref()
                .is_some_and(|morph| morph.phase(ctx.now) != PanelPhase::Closed);
            if !reusable {
                self.morph = Some(self.new_morph(ctx)?);
            }
            if let Some(morph) = &mut self.morph {
                morph.open(ctx.now).map_err(element("карточка раздела"))?;
            }
        }
        self.shown = true;
        Ok(FollowEffect::Open)
    }

    /// Кадр морфа. `true` — карточка только что закрылась целиком: владельцу пора забыть запомненное содержимое.
    pub(super) fn advance(&mut self, ctx: &Ctx<'_>) -> bool {
        let Some(morph) = &mut self.morph else {
            return false;
        };
        morph.advance(ctx.now);
        let _events = morph.drain_events();
        if !self.open_wanted && morph.phase(ctx.now) == PanelPhase::Closed {
            self.morph = None;
            self.shown = false;
            return true;
        }
        false
    }

    /// Видна ли карточка хоть сколько-нибудь (открыта, открывается или растворяется): пока видна, она лежит над
    /// нижним рядом, и ряд не рисуется.
    pub(super) fn is_visible(&self, ctx: &Ctx<'_>) -> bool {
        self.shown
            && self
                .morph
                .as_ref()
                .is_some_and(|morph| morph.phase(ctx.now) != PanelPhase::Closed)
    }

    /// Рисует стекло и возвращает вид содержимого; `None` — рисовать нечего (закрыта или содержимое ещё невидимо).
    pub(super) fn draw_glass(
        &self,
        ctx: &Ctx<'_>,
        host: &RootHost,
        layers: &mut DrawLayers,
    ) -> Result<Option<ContentLook>, WidgetError> {
        let (Some(morph), true) = (&self.morph, self.shown) else {
            return Ok(None);
        };
        let snapshot = morph.snapshot(ctx.now);
        if snapshot.phase == PanelPhase::Closed {
            return Ok(None);
        }
        let glass: DrawItem =
            panel_material_hosted(&snapshot, ctx.tokens, SECTION_PANEL_MATERIAL, ctx.px, host.transform())?;
        layers.push(Layer::Glass, glass);
        let alpha = snapshot.content_opacity.clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return Ok(None);
        }
        Ok(Some(ContentLook {
            alpha,
            slide_px: ctx.px.px(snapshot.content_offset_pt),
        }))
    }
}
