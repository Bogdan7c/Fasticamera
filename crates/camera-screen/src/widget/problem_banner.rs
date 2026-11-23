//! Плашка проблемы пресетов (решения 196, 197): K8 `Banner` с коротким английским текстом, сама гаснет по таймеру.
//!
//! Проблема приходит двумя путями (`LookView`): от библиотеки пресетов (имя занято, запись не удалась…) и от самого
//! экрана (неверное имя, нет клавиатуры). Состояние хранит ТЕКУЩУЮ плашку с номером (`ProblemBanner::number`), а
//! этот виджет показывает её и сам решает, когда погасить.
//!
//! # Инварианты
//! * Одна плашка на весь экран (владелец — `Stage`, а не раздел калибровки): раздел пересобирается при повороте, и
//!   таймер, живущий в нём, сбрасывался бы. Где плашку нарисовать, решает вызывающий (PHOTO и раздел калибровки
//!   ставят её по-разному).
//! * Новая проблема (новый номер) показывается заново, даже если прежняя ещё висит: срок отсчитывается с нуля.
//! * Истечение срока сообщается `ProblemDismissed` ровно один раз за плашку; закрытая вручную (состоянием) просто
//!   скрывается без действия.
//! * Тексты придуманы (в эталоне Apple такой плашки нет) и английские, как весь UI.

use look_contract::{LookProblemKind, PresetError};
use ui_core::geometry::Size;
use ui_core::time::UiDuration;
use ui_draw::geometry::PxSize;
use ui_draw::scene_bridge::NodePlacementPx;
use ui_kit::banner::{Banner, BannerEvent, BannerLifetime, HideCause};
use ui_kit::picker::LabelId;
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{banner_items, measure_banner_pt};

use super::error::{WidgetError, element};
use super::layers::{DrawLayers, Layer};
use super::text_runs::TextRuns;
use super::{Ctx, Outputs};
use crate::action::ScreenAction;
use crate::look_view::ProblemKind;

/// Ключ подписи плашки (единственный текст этого виджета; сама строка хранится рядом).
const PROBLEM_LABEL: LabelId = LabelId(0);

/// Текст плашки по виду проблемы. Перечислены ВСЕ варианты: новая проблема библиотеки не пройдёт молча.
pub(crate) fn problem_text(kind: &ProblemKind) -> &'static str {
    match kind {
        ProblemKind::Look(LookProblemKind::Rejected(error)) => match error {
            PresetError::NameTaken => "NAME TAKEN",
            PresetError::BuiltinIsReadOnly(_) => "READ ONLY",
            PresetError::NotFound(_) => "PRESET MISSING",
            PresetError::UnknownControl(_) => "UNKNOWN CONTROL",
            PresetError::Storage(_) => "SAVE FAILED",
        },
        ProblemKind::Look(LookProblemKind::SaveFailed(_)) => "SAVE FAILED",
        ProblemKind::Look(LookProblemKind::SavesRefused) => "SAVES OFF",
        ProblemKind::InvalidPresetName(_) => "INVALID NAME",
        ProblemKind::KeyboardUnavailable => "KEYBOARD UNAVAILABLE",
    }
}

pub(crate) struct ProblemBannerWidget {
    banner: Banner,
    /// Номер плашки состояния, которую виджет показал последней (`None` — плашки не было или её убрали).
    shown_number: Option<u64>,
    /// Текст показанной плашки.
    text: &'static str,
}

impl ProblemBannerWidget {
    pub(crate) fn build(ctx: &Ctx<'_>) -> Result<Self, WidgetError> {
        Ok(Self {
            banner: Banner::new(&ctx.environment(0.0), ctx.now).map_err(element("плашка проблемы"))?,
            shown_number: None,
            text: "",
        })
    }

    /// Выравнивает плашку по состоянию: новый номер — показать (с отсчётом срока с нуля), плашки в состоянии нет —
    /// скрыть. Тот же номер повторно ничего не меняет (срок не продлевается кадрами).
    pub(crate) fn sync(&mut self, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        match ctx.state.look().banner() {
            Some(problem) if self.shown_number != Some(problem.number) => {
                self.shown_number = Some(problem.number);
                self.text = problem_text(&problem.kind);
                let lifetime = BannerLifetime::For(UiDuration::from_secs_f64(
                    ctx.config.calibration.widgets.problem_banner_seconds,
                ));
                self.banner
                    .show(PROBLEM_LABEL, lifetime, ctx.now)
                    .map_err(element("плашка проблемы"))?;
            }
            Some(_) => {}
            None => {
                if self.shown_number.take().is_some() {
                    self.banner.hide(ctx.now).map_err(element("плашка проблемы"))?;
                }
            }
        }
        Ok(())
    }

    /// Кадр: по истечению срока плашка сообщает `ProblemDismissed`, и состояние убирает её.
    pub(crate) fn advance(&mut self, ctx: &Ctx<'_>, out: &mut Outputs) -> Result<(), WidgetError> {
        self.banner.advance(ctx.now).map_err(element("плашка проблемы"))?;
        for event in self.banner.drain_events() {
            if let BannerEvent::Hidden {
                cause: HideCause::Expired,
                ..
            } = event
                && let Some(number) = self.shown_number
            {
                out.actions.push(ScreenAction::ProblemDismissed { number });
            }
        }
        Ok(())
    }

    /// Рисует плашку; `place` по размеру плашки (pt) отдаёт её размещение (PHOTO и раздел калибровки ставят её
    /// по-разному). `None` от `place` — поставить негде, плашка пропускается.
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        place: impl FnOnce(Size) -> Option<NodePlacementPx>,
    ) -> Result<(), WidgetError> {
        let snapshot = self.banner.snapshot(ctx.now);
        if snapshot.label.is_none() {
            return Ok(());
        }
        let run = texts.run(ctx, self.text, TextRoleKind::BannerText)?;
        let size = measure_banner_pt(run.advance_px, ctx.px, &ctx.tokens.banner);
        let Some(placement) = place(size) else {
            return Ok(());
        };
        let placement = NodePlacementPx {
            size: PxSize::new(ctx.px.px(size.width), ctx.px.px(size.height)),
            ..placement
        };
        layers.extend(
            Layer::Plain,
            banner_items(&placement, &snapshot, run, &ctx.tokens.colors)?,
        );
        Ok(())
    }

    /// Размещение плашки на экране PHOTO: там же, где плашка AE/AF LOCK (по центру кадра у его верха); пока LOCK
    /// висит, проблема встаёт под неё, а не поверх.
    pub(crate) fn photo_placement(ctx: &Ctx<'_>, size: Size) -> NodePlacementPx {
        let mut center = ctx.geometry.lock_plate_center(ctx.state.settings().aspect, size.height);
        if ctx.state.focus().is_lock_plaque_visible() {
            center.y += size.height + ctx.config.calibration.widgets.problem_banner_gap_pt;
        }
        NodePlacementPx {
            size: PxSize::new(ctx.px.px(size.width), ctx.px.px(size.height)),
            transform: ui_draw::transform::Affine2::translate(ctx.px.px(center.x), ctx.px.px(center.y)),
            opacity: 1.0,
        }
    }
}
