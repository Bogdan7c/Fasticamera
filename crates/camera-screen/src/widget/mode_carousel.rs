//! Капсула режимов (K4 `SnapCarousel`): стеклянная капсула 221,4 × 48,9 pt с подписями режимов из реестра
//! и пилюлей выбранного режима внутри.
//!
//! Режимов два (PHOTO и CALIBRATE, решение 185): горизонтальный жест переключает их, тап по текущему режиму
//! открывает меню (`ModeSelected` того же режима, решает редьюсер).
//!
//! Капсула живёт в двух местах (решение 84, один виджет — два дома): на экране PHOTO и внутри раздела калибровки
//! ([`CarouselHome`]). Дом раздела стоит в повёрнутом корне раздела: жесты он принимает под своими намерениями,
//! смещения и точки тапа переводит в глазные оси, а пилюля берёт ширину выбранной подписи.

use ui_core::geometry::{Point, Rect};
use ui_core::gesture::{GestureData, GestureEvent, GestureState, PanDirection, PanSpec, RecognizerSpec, TapSpec};
use ui_core::id::NodeId;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, PressState, ShapeItem, StrokeStyle};
use ui_draw::geometry::PxSize;
use ui_draw::shape::ShapeGeometry;
use ui_kit::carousel::{CarouselEntry, CarouselEvent, CarouselSpec, SnapCarousel, TapOutcome};
use ui_kit::drive::ScrollTransition;
use ui_kit::pan::PanUpdate;
use ui_kit::tokens::GlassRole;
use ui_kit::view::{CarouselLabels, GlassShape, LabelPainter, carousel_items_in_window, opacity_of_placement};

use super::bottom_row::BottomRowFade;
use super::calibration::space::{EyeAxis, SectionSpace};
use super::error::{WidgetError, element};
use super::intent::ScreenGesture;
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::{Ctx, Outputs, single_member_group};
use crate::action::ScreenAction;
use crate::modes::CaptureMode;

/// Где стоит капсула.
#[derive(Debug, Clone, Copy)]
#[allow(
    clippy::large_enum_variant,
    reason = "значение живёт в виджете один раз на сцену и копируется по значению; коробка только усложнила бы сопоставление"
)]
pub(crate) enum CarouselHome {
    /// Нижний ряд экрана PHOTO.
    Screen,
    /// Нижний ряд раздела калибровки: рамка капсулы и пространство раздела.
    Section { frame: Rect, space: SectionSpace },
}

pub(crate) struct ModeCarouselWidget {
    home: CarouselHome,
    /// Рамка капсулы, pt (измеренный размер).
    frame: Rect,
    node: NodeId,
    carousel: SnapCarousel<CaptureMode>,
    /// Подписи, свёрстанные один раз при сборке сцены, в порядке записей реестра.
    labels: CarouselLabels,
    /// Растворение, пока открыта нижняя карточка (решение 103).
    fade: BottomRowFade,
}

impl ModeCarouselWidget {
    pub(crate) fn build(kit: &mut SceneKit, ctx: &Ctx<'_>, parent: NodeId) -> Result<Self, WidgetError> {
        Self::build_at(kit, ctx, (parent, ctx.geometry.screen()), CarouselHome::Screen)
    }

    /// Капсула в `home`; `(parent, parent_rect)` — узел-родитель и его рамка.
    ///
    /// Капсула сохраняет измеренный размер (221,4 × 48,9 pt) и измеренный промежуток между подписями (35,4 pt,
    /// `[measured: R00/C]`): соседняя подпись видна у края и гаснет в окне капсулы, как в камере iOS 27
    /// (`layout.md`), а не втискивается в расширенную рамку.
    pub(crate) fn build_at(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        home: CarouselHome,
    ) -> Result<Self, WidgetError> {
        let painter = LabelPainter::new(&ctx.tokens.typography, ctx.px);
        let descriptors = ctx.state.mode_carousel();
        let texts: Vec<&str> = descriptors.iter().map(|descriptor| descriptor.label).collect();
        let labels = CarouselLabels::layout(&painter, &texts)?;
        let mut entries = Vec::with_capacity(descriptors.len());
        for (index, descriptor) in descriptors.iter().enumerate() {
            let advance_px = labels
                .advance_px(index)
                .ok_or_else(|| WidgetError::Inconsistent(format!("для режима {index} нет свёрстанной подписи")))?;
            entries.push(CarouselEntry {
                item: descriptor.mode,
                width_points: f64::from(advance_px / ctx.px.get()),
            });
        }
        let frame = match home {
            CarouselHome::Screen => ctx.geometry.capsule(),
            CarouselHome::Section { frame, .. } => frame,
        };
        let node = kit.add_node(parent, "mode_capsule", frame, parent_rect)?;
        match home {
            CarouselHome::Screen => {
                kit.add_recognizer(node, RecognizerSpec::Tap(TapSpec::default()), ScreenGesture::CapsuleTap)?;
                for (direction, intent) in [
                    (PanDirection::Horizontal, ScreenGesture::ModesPan),
                    (PanDirection::Up, ScreenGesture::MenuOpenPan),
                ] {
                    kit.add_recognizer(
                        node,
                        RecognizerSpec::Pan(PanSpec {
                            direction,
                            ..PanSpec::default()
                        }),
                        intent,
                    )?;
                }
            }
            CarouselHome::Section { space, .. } => {
                kit.add_recognizer(
                    node,
                    RecognizerSpec::Tap(TapSpec::default()),
                    ScreenGesture::CalibrationCapsuleTap,
                )?;
                kit.add_recognizer(
                    node,
                    RecognizerSpec::Pan(PanSpec {
                        direction: space.pan_direction(EyeAxis::Horizontal),
                        ..PanSpec::default()
                    }),
                    ScreenGesture::CalibrationModesPan,
                )?;
            }
        }
        let environment = ctx.environment(frame.size.width);
        let carousel = SnapCarousel::new(
            entries,
            &ctx.state.mode(),
            CarouselSpec::default(),
            &environment,
            ctx.now,
        )
        .map_err(element("карусель режимов"))?;
        Ok(Self {
            home,
            frame,
            node,
            carousel,
            labels,
            fade: BottomRowFade::default(),
        })
    }

    /// Жесты капсулы. `kit` нужен дому раздела: точку тапа он переводит в систему рамки капсулы.
    pub(crate) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let (pan_intent, tap_intent) = match self.home {
            CarouselHome::Screen => (ScreenGesture::ModesPan, ScreenGesture::CapsuleTap),
            CarouselHome::Section { .. } => (ScreenGesture::CalibrationModesPan, ScreenGesture::CalibrationCapsuleTap),
        };
        match event.intent {
            intent if intent == pan_intent => {
                if let Some(update) =
                    PanUpdate::from_gesture(event.state, event.data).map_err(element("пан режимов"))?
                {
                    let update = match self.home {
                        CarouselHome::Screen => update,
                        CarouselHome::Section { space, .. } => space.eye_pan(update),
                    };
                    self.carousel
                        .apply_pan(event.time, update)
                        .map_err(element("карусель режимов"))?;
                }
            }
            ScreenGesture::MenuOpenPan if matches!(self.home, CarouselHome::Screen) => {
                // Меню открывается по завершению жеста: отменённый (разрыв потока) ничего не делает.
                if event.state == GestureState::Ended {
                    out.actions.push(ScreenAction::ControlsMenuOpenRequested);
                }
            }
            intent if intent == tap_intent => {
                if let (GestureState::Ended, GestureData::Tap { position, .. }) = (event.state, event.data) {
                    let position = self.capsule_point(kit, position);
                    self.tapped_at(position, event, out)?;
                }
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Точка касания в системе рамки капсулы (дом раздела повёрнут, дом экрана — нет). Вырожденное
    /// преобразование оставляет точку как есть: тап тогда попадёт мимо подписей и выберет текущий режим.
    fn capsule_point(&self, kit: &SceneKit, screen_point: Point) -> Point {
        match self.home {
            CarouselHome::Screen => screen_point,
            CarouselHome::Section { .. } => kit.point_in_node(self.node, screen_point).unwrap_or(screen_point),
        }
    }

    /// Тап по капсуле в точке `position` (pt, система рамки капсулы): режим под пальцем, а если мимо подписей —
    /// текущий.
    fn tapped_at(
        &mut self,
        position: Point,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<(), WidgetError> {
        let index = self
            .carousel
            .item_index_at(position.x - self.frame.center().x, event.time)
            .unwrap_or_else(|| self.carousel.selected_index());
        let outcome = self
            .carousel
            .tapped_item(index, event.time)
            .map_err(element("карусель режимов"))?;
        if outcome == TapOutcome::AlreadySelected {
            // Выбор того же режима — «открой меню»; смена режима придёт событием `SelectionChanged`.
            out.actions.push(ScreenAction::ModeSelected(*self.carousel.selected()));
        }
        Ok(())
    }

    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let wanted_index = self
            .carousel
            .entries()
            .iter()
            .position(|entry| entry.item == ctx.state.mode());
        if let Some(index) = wanted_index
            && index != self.carousel.selected_index()
        {
            self.carousel
                .select(index, ScrollTransition::Animated, ctx.now)
                .map_err(element("карусель режимов"))?;
        }
        // Внутри раздела ряд карточка не растворяет: меню раздела лежит поверх него целиком (шторка и карточка
        // перекрывают ряд, а не растворяют его), поэтому растворять ряд некому.
        let shown = match self.home {
            CarouselHome::Screen => self.fade.sync(kit, ctx, self.node)?,
            CarouselHome::Section { .. } => true,
        };
        kit.set_interactive(self.node, shown)?;
        Ok(())
    }

    pub(crate) fn advance(&mut self, out: &mut Outputs) {
        for event in self.carousel.drain_events() {
            match event {
                CarouselEvent::SelectionChanged {
                    item,
                    cause: ui_kit::carousel::SelectionCause::Swipe | ui_kit::carousel::SelectionCause::Tap,
                    ..
                } => out.actions.push(ScreenAction::ModeSelected(item)),
                // Программный выбор — следствие состояния экрана, обратно действием не отдаётся (без эха).
                CarouselEvent::SelectionChanged { .. } => {}
                CarouselEvent::Haptic { pattern } => out.haptics.push(pattern),
            }
        }
    }

    /// Пилюля: светлая заливка и тонкий светлый ободок. `host` — преобразование центра капсулы.
    fn selected_pill(
        ctx: &Ctx<'_>,
        host: ui_draw::transform::Affine2,
        opacity: ui_draw::color::Opacity,
        pill_width_pt: f64,
    ) -> DrawItem {
        let bottom = &ctx.geometry.config().bottom;
        let pill = ctx.geometry.mode_pill();
        let pill = Rect::from_xywh(pill.min_x(), pill.min_y(), pill_width_pt, pill.size.height);
        let white = ctx.tokens.colors.text_primary;
        DrawItem::Shape(ShapeItem {
            geometry: ShapeGeometry::Capsule {
                size: PxSize::new(ctx.px.px(pill.size.width), ctx.px.px(pill.size.height)),
            },
            transform: host,
            fill: DisplayColor {
                a: bottom.mode_pill_fill_alpha,
                ..white
            },
            stroke: Some(StrokeStyle {
                width_px: ctx.px.px(bottom.mode_pill_border_width_pt),
                color: DisplayColor {
                    a: bottom.mode_pill_border_alpha,
                    ..white
                },
            }),
            shadow: None,
            opacity,
        })
    }

    /// Ширина пилюли: на экране PHOTO — измеренная (89 pt); в разделе — подпись выбранного режима с запасом
    /// (подписи разной длины, «CALIBRATE» шире «PHOTO»).
    fn pill_width_pt(&self, ctx: &Ctx<'_>) -> f64 {
        match self.home {
            CarouselHome::Screen => ctx.geometry.mode_pill().size.width,
            CarouselHome::Section { .. } => {
                let label_pt = self
                    .carousel
                    .entries()
                    .get(self.carousel.selected_index())
                    .map_or(0.0, |entry| entry.width_points);
                (label_pt + 2.0 * ctx.config.calibration.widgets.mode_pill_padding_pt)
                    .max(ctx.geometry.mode_pill().size.width)
            }
        }
    }

    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        if placement.opacity <= 0.0 {
            return Ok(());
        }
        let (group, host) = single_member_group(
            ctx,
            GlassRole::Button,
            GlassShape::Capsule,
            &placement,
            PressState::RELEASED,
            "mode_capsule",
        )?;
        layers.push(Layer::Glass, DrawItem::Glass(group));
        let opacity = opacity_of_placement(&placement, "mode_labels")?;
        // Пилюля выбранного режима стоит по центру капсулы: подписи едут под ней, как в камере iOS.
        layers.push(
            Layer::Content,
            Self::selected_pill(ctx, host, opacity, self.pill_width_pt(ctx)),
        );
        // Подписи ставит вид `ui-kit`: выбранная — `selected_label`, остальные — приглушённые. Окно — сама капсула:
        // соседняя подпись гаснет у её края, а что не поместилось, режется ровно по стеклу (решение 232).
        let labels = carousel_items_in_window(
            host,
            opacity,
            &self.carousel,
            ctx.now,
            &self.labels,
            ctx.px,
            &ctx.tokens.colors,
            self.frame.size.width / 2.0,
        )?;
        let Some(capsule_rect) = placement.screen_rect_px() else {
            return Ok(());
        };
        layers.push_clipped(Layer::Content, capsule_rect, labels);
        Ok(())
    }
}
