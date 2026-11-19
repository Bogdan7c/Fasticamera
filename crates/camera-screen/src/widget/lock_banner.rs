//! Плашка «AE/AF LOCK» (K8 `Banner`): вверху кадра по центру, пока камера подтверждает блокировку
//! (`focus().is_lock_plaque_visible()`).

use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_draw::transform::Affine2;
use ui_kit::banner::{Banner, BannerLifetime};
use ui_kit::picker::LabelId;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{banner_items, measure_banner_pt};

use super::Ctx;
use super::error::{WidgetError, element};
use super::layers::{DrawLayers, Layer};
use super::text_runs::TextRuns;

/// Ключ подписи плашки (единственный текст этого виджета).
const LOCK_LABEL: LabelId = LabelId(0);
/// Текст плашки (английский, как в iOS).
const LOCK_TEXT: &str = "AE/AF LOCK";

pub(crate) struct LockBannerWidget {
    banner: Banner,
    shown_wanted: bool,
}

impl LockBannerWidget {
    pub(crate) fn build(ctx: &Ctx<'_>) -> Result<Self, WidgetError> {
        Ok(Self {
            banner: Banner::new(&ctx.environment(0.0), ctx.now).map_err(element("плашка LOCK"))?,
            shown_wanted: false,
        })
    }

    pub(crate) fn sync(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let visible = ctx.state.focus().is_lock_plaque_visible();
        if visible == self.shown_wanted {
            return Ok(());
        }
        self.shown_wanted = visible;
        if visible {
            self.banner
                .show(LOCK_LABEL, BannerLifetime::UntilHidden, ctx.now)
                .map_err(element("плашка LOCK"))?;
        } else {
            self.banner.hide(ctx.now).map_err(element("плашка LOCK"))?;
        }
        Ok(())
    }

    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        self.banner.advance(ctx.now).map_err(element("плашка LOCK"))?;
        // События «показана/скрыта» состоянию экрана не нужны: его источник — камера (`LockState`).
        let _events = self.banner.drain_events();
        Ok(())
    }

    pub(crate) fn draw(&self, ctx: &Ctx<'_>, layers: &mut DrawLayers, texts: &mut TextRuns) -> Result<(), WidgetError> {
        let snapshot = self.banner.snapshot(ctx.now);
        if snapshot.label.is_none() {
            return Ok(());
        }
        let run = texts.run(ctx, LOCK_TEXT, TextRoleKind::BannerText)?;
        let size = measure_banner_pt(run.advance_px, ctx.px, &ctx.tokens.banner);
        let center = ctx.geometry.lock_plate_center(ctx.state.settings().aspect, size.height);
        let placement = NodePlacementPx {
            size: PxSize::new(ctx.px.px(size.width), ctx.px.px(size.height)),
            transform: Affine2::translate(ctx.px.px(center.x), ctx.px.px(center.y)),
            opacity: 1.0,
        };
        layers.extend(
            Layer::Plain,
            banner_items(&placement, &snapshot, run, &ctx.tokens.colors)?,
        );
        Ok(())
    }
}
