//! Круг толчка выбранного колеса: подложка, цветное кольцо оттенков, перекрестие и точка (решения 205, 206).
//!
//! # Оси
//! Точка круга живёт в «глазных» осях: `x` вправо, `y` ВВЕРХ, и это прямо `(a, b)` OKLab (0° = +a, 90° = +b —
//! как вектороскоп 14h и кольцо оттенков). Пан приходит в осях поверхности, поэтому ПЕРЕД площадкой он
//! переводится в глазные оси (`SectionSpace::eye_pan`, поворот раздела), а площадка сама переворачивает `y`
//! экрана (вниз) в `y` значения (вверх). Своих переворотов здесь нет: два места, где они бывают, — `eye_pan` и
//! `DiscPad`.
//!
//! # Эхо и залипание
//! * Действие рождает только палец (`DiscPad::apply_pan`); синхронизация с фактами — `set_point`, без событий.
//! * Зона колеса фиксируется на `Began`: вкладка, сменённая другим пальцем посреди жеста, толчок не перенаправляет.
//! * Из фактов точка подтягивается только при ИЗМЕНЕНИИ факта (зоны или значения), а пока палец ведёт — не
//!   трогается и ждёт отпускания: факт, пришедший во время ведения, не теряется.
//! * Раздел, пересобранный посреди жеста (поворот), роняет этот виджет вместе с площадкой: «залипшего» ведения
//!   новая площадка не наследует, а запоздалый `Cancelled` ей безвреден (нет ведения — нет действий).

use look_contract::{WheelPush, WheelZone};
use ui_core::geometry::Rect;
use ui_core::gesture::{GestureEvent, PanDirection, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, ImageClip, ImageFit, ImageItem, ImageSource};
use ui_draw::geometry::PxSize;
use ui_draw::picture::UiPicture;
use ui_kit::disc_pad::{DiscPad, DiscPadSpec, DiscPoint, ProgrammaticSet};
use ui_kit::pan::PanUpdate;
use ui_kit::view::{RingGeometry, disc_pad_items, ring_picture};

use crate::action::ScreenAction;
use crate::calibration::WheelsFrames;
use crate::widget::calibration::paint::NodePaint;
use crate::widget::calibration::space::SectionSpace;
use crate::widget::error::{WidgetError, element};
use crate::widget::intent::ScreenGesture;
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::{Ctx, Outputs};

/// Круг толчка.
pub(super) struct WheelPad {
    node: NodeId,
    pad: DiscPad,
    /// Внешний диаметр цветного кольца, pt (из раскладки) и px (сторона картинки при текущем масштабе).
    circle_diameter_pt: f64,
    outer_diameter_px: u32,
    /// Колесо, которое палец взял на `Began`: действие уходит ему, даже если вкладка сменилась посреди жеста.
    dragged_zone: Option<WheelZone>,
    /// Последний толчок, отправленный пальцем: `DiscPad` сравнивает f64, а действие несёт f32 — две соседние
    /// прижатые к краю точки могут совпасть в f32, и повторять такое действие незачем.
    last_sent: Option<(WheelZone, WheelPush)>,
    /// Последний факт, к которому точка уже подтянута: подтягиваем при его ИЗМЕНЕНИИ, а не каждый кадр.
    seen_fact: Option<(WheelZone, WheelPush)>,
    /// Цвета кольца, по которым построена картинка (сравниваются, чтобы не строить растр каждый кадр).
    ring_stops: Vec<[u8; 3]>,
    ring_picture: Option<UiPicture>,
    /// Доступен ли круг касаниям (факты есть).
    enabled: bool,
    /// Сколько точек пальца не стали толчком (не конечные): ошибка видна в диагностике.
    invalid_values: u64,
}

impl WheelPad {
    /// Строит узел круга внутри `parent`; рамка узла — квадрат круга из `frames`.
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        frames: &WheelsFrames,
    ) -> Result<Self, WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let node = kit.add_node(parent, "calib_wheel_pad", frames.circle, parent_rect)?;
        kit.add_recognizer(
            node,
            RecognizerSpec::Pan(PanSpec {
                // Палец тянет точку в любую сторону; повёрнутый раздел направление не меняет.
                direction: PanDirection::Any,
                ..PanSpec::default()
            }),
            ScreenGesture::CalibrationWheelPadPan,
        )?;
        let spec = DiscPadSpec::with_token_drive(frames.pad_radius_pt(config), &ctx.tokens.disc_pad);
        let pad = DiscPad::new(spec, DiscPoint::CENTER).map_err(element("круг колеса"))?;
        Ok(Self {
            node,
            pad,
            circle_diameter_pt: frames.circle.size.width,
            outer_diameter_px: ctx.px.px(frames.circle.size.width).round() as u32,
            dragged_zone: None,
            last_sent: None,
            seen_fact: None,
            ring_stops: Vec::new(),
            ring_picture: None,
            enabled: false,
            invalid_values: 0,
        })
    }

    pub(super) fn node(&self) -> NodeId {
        self.node
    }

    pub(super) fn invalid_values(&self) -> u64 {
        self.invalid_values
    }

    /// Пан пальца по кругу. Возвращает `true` — событие этого круга.
    pub(super) fn handle_gesture(
        &mut self,
        space: &SectionSpace,
        ctx: &Ctx<'_>,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        if event.intent != ScreenGesture::CalibrationWheelPadPan {
            return Ok(false);
        }
        let Some(update) = PanUpdate::from_gesture(event.state, event.data).map_err(element("пан круга колеса"))?
        else {
            return Ok(true);
        };
        if !self.enabled && !self.pad.is_finger_driven() {
            // Нет фактов: круг тусклый и пальца не берёт.
            return Ok(true);
        }
        if matches!(update, PanUpdate::Began { .. }) {
            self.dragged_zone = Some(ctx.state.calibration().selected_wheel());
            self.last_sent = None;
        }
        let was_driven = self.pad.is_finger_driven();
        let moved = self
            .pad
            .apply_pan(space.eye_pan(update))
            .map_err(element("круг колеса"))?;
        if let Some(point) = moved {
            self.emit_pushed(point, out);
        }
        if !self.pad.is_finger_driven() {
            self.dragged_zone = None;
            if was_driven {
                // Конец ведения: точка обязана снова равняться ФАКТАМ. Если библиотека отвергла или не изменила
                // значение, факт после жеста тот же, что до него, и дедупликация «по факту» оставила бы точку
                // там, где её бросил палец. Забываем виденный факт — ближайшая `sync` подтянет точку к фактам.
                self.seen_fact = None;
            }
        }
        Ok(true)
    }

    /// Точка пальца → действие `WheelPushDragged` (точка прижата к кругу, повтор того же толчка не шлётся).
    fn emit_pushed(&mut self, point: DiscPoint, out: &mut Outputs) {
        let (Some(zone), Ok(push)) = (
            self.dragged_zone,
            WheelPush::clamped_to_circle(point.x() as f32, point.y() as f32),
        ) else {
            self.invalid_values += 1;
            return;
        };
        if self.last_sent == Some((zone, push)) {
            return;
        }
        self.last_sent = Some((zone, push));
        out.actions.push(ScreenAction::WheelPushDragged { zone, push });
    }

    /// Выравнивает круг по фактам: доступность, точка выбранного колеса (без эха) и растр кольца.
    pub(super) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let facts = ctx.state.look_facts();
        self.enabled = facts.is_some();
        kit.set_interactive(self.node, self.enabled)?;
        let Some(facts) = facts else {
            return Ok(());
        };
        let zone = ctx.state.calibration().selected_wheel();
        let fact = (zone, facts.current.settings.wheels.wheel(zone).push);
        if self.seen_fact != Some(fact) {
            let point =
                DiscPoint::clamped(f64::from(fact.1.a()), f64::from(fact.1.b())).map_err(element("точка колеса"))?;
            // Пока палец ведёт, установка отбрасывается, а факт остаётся «невиданным» и подтянется после отпускания.
            if self.pad.set_point(point) == ProgrammaticSet::Applied {
                self.seen_fact = Some(fact);
            }
        }
        // Растр пересобирается только при смене цветов или размера в px (масштаб поверхности мог смениться и без
        // пересборки раздела); сравнение идёт итератором, без выделения на каждый кадр.
        let outer_diameter_px = ctx.px.px(self.circle_diameter_pt).round() as u32;
        let stops = facts.wheel_ring.stops();
        let same_stops = stops.len() == self.ring_stops.len()
            && stops
                .iter()
                .zip(&self.ring_stops)
                .all(|(stop, known)| stop.srgb == *known);
        if !same_stops || self.ring_picture.is_none() || outer_diameter_px != self.outer_diameter_px {
            self.outer_diameter_px = outer_diameter_px;
            self.rebuild_ring(ctx, stops.iter().map(|stop| stop.srgb).collect())?;
        }
        Ok(())
    }

    /// Растр кольца: один раз на смену цветов (рендер кэширует его по версии картинки).
    fn rebuild_ring(&mut self, ctx: &Ctx<'_>, stops: Vec<[u8; 3]>) -> Result<(), WidgetError> {
        let thickness_px = ctx.px.px(ctx.config.calibration.widgets.wheel_ring_thickness_pt);
        let picture = ring_picture(
            &stops,
            RingGeometry {
                outer_diameter_px: self.outer_diameter_px,
                thickness_px,
            },
        )
        .map_err(element("кольцо оттенков"))?;
        self.ring_picture = Some(picture);
        self.ring_stops = stops;
        Ok(())
    }

    /// Рисует круг: тёмная подложка, кольцо оттенков, перекрестие и точка. Без фактов — тускло и без точки на кольце.
    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let Some(placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        let dim = if self.enabled { 1.0 } else { config.disabled_row_opacity };
        let Some(paint) = NodePaint::new(ctx, placement, dim)? else {
            return Ok(());
        };
        let pad_diameter_pt = 2.0 * self.pad.spec().radius_pt;
        let backdrop = DisplayColor {
            a: config.wheel_pad_fill_alpha,
            ..DisplayColor::BLACK
        };
        layers.push(Layer::Content, paint.disc(0.0, 0.0, pad_diameter_pt, backdrop));
        if let Some(picture) = &self.ring_picture {
            layers.push(
                Layer::Content,
                DrawItem::Image(ImageItem {
                    source: ImageSource::Picture(picture.clone()),
                    transform: paint.at(0.0, 0.0),
                    // Картинка построена ровно под этот размер: 1:1 пиксель в пиксель.
                    size_px: PxSize::new(self.outer_diameter_px as f32, self.outer_diameter_px as f32),
                    fit: ImageFit::Stretch,
                    clip: ImageClip::None,
                    opacity: paint.opacity(),
                }),
            );
        }
        layers.extend(
            Layer::Content,
            disc_pad_items(
                &paint.placement(),
                &self.pad,
                ctx.px,
                &ctx.tokens.disc_pad,
                &ctx.tokens.colors,
            )?,
        );
        Ok(())
    }
}
