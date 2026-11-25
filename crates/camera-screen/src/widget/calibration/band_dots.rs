//! Ряд цветных точек полос оттенка (решение 200): тап выбирает полосу; у выбранной белое кольцо; у полосы с
//! правками (значения не нейтральны) жёлтое кольцо вплотную к точке; у полосы, чем-то отличающейся от пресета,
//! под точкой стоит метка (идиома выдумана: у Apple такого ряда нет).
//!
//! Почему «правки» — это «не нейтрально», а не «отличается от пресета»: рабочий вид владельца — сохранённый пресет,
//! в котором цветовые правки уже есть; относительно пресета у такой полосы различий нет, и метка «отличается от
//! пресета» молчала, хотя полоса правлена (дефект телефонной проверки 14l). Решение 200 обещало кольцо у точки с
//! правками — оно показывает, где правки есть вообще; метка под точкой по-прежнему говорит про отличие от пресета.
//!
//! Цвета и состав полос приходят фактами (`LookFacts::bands`): экран ничего не знает о том, как полоса считается.
//! Если состав полос поменялся, раздел пересобирается целиком (ключ сборки включает идентификаторы полос).

use look_contract::{BandAxis, BandFacts, HueBandId, LookControl};
use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;

use super::paint::{NodePaint, srgb8};
use crate::action::ScreenAction;
use crate::widget::button::{ButtonEvent, PressButton};
use crate::widget::error::WidgetError;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::{Ctx, Outputs};

/// Одна точка полосы.
struct BandDot {
    band: HueBandId,
    swatch: [u8; 3],
    button: PressButton,
    /// Центр точки от центра ряда, pt.
    center_offset_x: f64,
}

/// Ряд точек.
pub(super) struct BandDots {
    node: NodeId,
    frame: Rect,
    dots: Vec<BandDot>,
}

impl BandDots {
    /// Строит ряд внутри `parent`; `row` — рамка ряда в системе корня.
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        row: Rect,
        bands: &[BandFacts],
    ) -> Result<Self, WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let diameter = config.band_dot_diameter_pt;
        let node = kit.add_node(parent, "calib_band_row", row, parent_rect)?;
        let pitch = dot_pitch(row.size.width, bands.len(), diameter, config.band_dot_gap_pt);
        let first_offset = -pitch * (bands.len().saturating_sub(1)) as f64 / 2.0;
        let mut dots = Vec::with_capacity(bands.len());
        for (index, band) in bands.iter().enumerate() {
            let center_offset_x = first_offset + pitch * index as f64;
            let center_x = row.center().x + center_offset_x;
            let frame = Rect::from_xywh(
                center_x - diameter / 2.0,
                row.center().y - diameter / 2.0,
                diameter,
                diameter,
            );
            let dot_node = kit.add_node(node, "calib_band_dot", frame, row)?;
            let id = u8::try_from(index)
                .map_err(|_| WidgetError::Inconsistent(format!("полос слишком много для ряда точек: {index}")))?;
            let button = PressButton::attach(kit, ButtonId::CalibrationBandDot(id), dot_node, frame, ctx)?;
            // Точка Ø 24 мала для пальца: зона касания — не меньше квадрата кнопок карточки (HIG ≥ 44 pt).
            kit.ensure_min_touch_side(dot_node, frame, ctx.geometry.config().card.button_touch_side_pt)?;
            dots.push(BandDot {
                band: band.id.clone(),
                swatch: band.swatch.srgb,
                button,
                center_offset_x,
            });
        }
        Ok(Self { node, frame: row, dots })
    }

    pub(super) fn frame(&self) -> Rect {
        self.frame
    }

    /// Тап по точке выбирает её полосу. `false` — событие чужое.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
        out: &mut Outputs,
    ) -> Result<bool, WidgetError> {
        let ScreenGesture::Press(ButtonId::CalibrationBandDot(index)) = event.intent else {
            return Ok(false);
        };
        let Some(dot) = self.dots.get_mut(usize::from(index)) else {
            return Err(WidgetError::Inconsistent(format!("точки полосы {index} нет в ряду")));
        };
        let data = kit.gesture_data_in_node(dot.button.node(), event.data);
        if dot.button.handle(event.state, data, event.time)? == ButtonEvent::Activated {
            out.actions.push(ScreenAction::BandSelected(dot.band.clone()));
        }
        Ok(true)
    }

    pub(super) fn sync(&self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        let enabled = ctx.state.look_facts().is_some();
        for dot in &self.dots {
            kit.set_interactive(dot.button.node(), enabled)?;
        }
        Ok(())
    }

    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
    ) -> Result<(), WidgetError> {
        let Some(placement) = visuals.placement(self.node) else {
            return Ok(());
        };
        let Some(paint) = NodePaint::new(ctx, placement, 1.0)? else {
            return Ok(());
        };
        let config = &ctx.config.calibration.widgets;
        let colors = &ctx.tokens.colors;
        let facts = ctx.state.look_facts();
        let selected = ctx.state.calibration().selected_band();
        let diameter = config.band_dot_diameter_pt;
        for dot in &self.dots {
            let center = (dot.center_offset_x, 0.0);
            layers.push(
                Layer::Content,
                paint.disc(center.0, center.1, diameter, srgb8(dot.swatch)),
            );
            let has_edits = facts.is_some_and(|facts| !facts.current.settings.bands.for_band(&dot.band).is_neutral());
            if has_edits {
                // Жёлтое кольцо у самой точки; кольцо выбора (белое) лежит снаружи и не перекрывает его.
                let edited_diameter =
                    diameter + 2.0 * config.band_edited_ring_gap_pt + config.band_edited_ring_width_pt;
                layers.push(
                    Layer::Content,
                    paint.ring(
                        center,
                        edited_diameter,
                        config.band_edited_ring_width_pt,
                        colors.focus_yellow,
                    ),
                );
            }
            if selected == Some(&dot.band) {
                let ring_diameter = diameter + 2.0 * config.band_ring_gap_pt + config.band_ring_width_pt;
                layers.push(
                    Layer::Content,
                    paint.ring(center, ring_diameter, config.band_ring_width_pt, colors.text_primary),
                );
            }
            let differs = facts.is_some_and(|facts| {
                BandAxis::ALL.iter().any(|axis| {
                    facts.control_differs(&LookControl::Band {
                        band: dot.band.clone(),
                        axis: *axis,
                    })
                })
            });
            if differs {
                let marker_y = diameter / 2.0
                    + config.band_ring_gap_pt
                    + config.band_ring_width_pt
                    + config.band_marker_diameter_pt / 2.0;
                layers.push(
                    Layer::Content,
                    paint.disc(center.0, marker_y, config.band_marker_diameter_pt, colors.focus_yellow),
                );
            }
        }
        Ok(())
    }
}

/// Шаг между центрами точек: обычный (диаметр + промежуток), но не шире ряда — иначе точки сближаются.
fn dot_pitch(row_width_pt: f64, count: usize, diameter_pt: f64, gap_pt: f64) -> f64 {
    let natural = diameter_pt + gap_pt;
    if count < 2 {
        return natural;
    }
    let fits = (row_width_pt - diameter_pt) / (count - 1) as f64;
    natural.min(fits.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::dot_pitch;

    #[test]
    fn pitch_is_natural_when_the_row_is_wide_and_shrinks_to_fit_otherwise() {
        assert_eq!(dot_pitch(200.0, 3, 24.0, 10.0), 34.0);
        // 8 точек Ø 24 в ряду 196: естественно 8×34 − 10 = 262 > 196, шаг сжимается до (196 − 24) / 7.
        let squeezed = dot_pitch(196.0, 8, 24.0, 10.0);
        assert!((squeezed - 172.0 / 7.0).abs() < 1e-9, "{squeezed}");
        assert_eq!(dot_pitch(100.0, 1, 24.0, 10.0), 34.0, "одна точка шага не имеет");
    }
}
