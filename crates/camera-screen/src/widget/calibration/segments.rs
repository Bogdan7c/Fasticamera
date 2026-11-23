//! Ряд равных сегментов-капсул с типизированным значением: переключатель вида SCALES | WHEELS и вкладки колёс.
//!
//! Вид тот же, что у «таблеток» ряда имён пресетов (`chip_row`): белая заливка `chip_fill_alpha`, выбранный
//! сегмент — жёлтая подпись. Отличие от `ChipRow` одно, и оно принципиальное: у `ChipRow` номер таблетки — `u8`
//! и пул узлов, а здесь каждый сегмент несёт СВОЁ значение (`ControlViewKind`, `WheelZone`) прямо в типизированном
//! `ButtonId`. Поэтому «какой сегмент нажат» не расшифровывается из номера, а читается из самого намерения.
//!
//! Ряд владеет только устройством (узлы, касания, рисование); что значит нажатие и что выбрано сейчас, решает
//! владелец: он получает [`SegmentEvent::Activated`] и отдаёт выбранное и «помеченное» при рисовании.

use ui_core::geometry::Rect;
use ui_core::gesture::GestureEvent;
use ui_core::id::NodeId;
use ui_kit::tokens::TextRoleKind;

use super::paint::NodePaint;
use crate::widget::button::{ButtonEvent, PressButton};
use crate::widget::error::WidgetError;
use crate::widget::intent::{ButtonId, ScreenGesture};
use crate::widget::kit::SceneKit;
use crate::widget::layers::{DrawLayers, Layer, VisualIndex};
use crate::widget::text_runs::TextRuns;
use crate::widget::{Ctx, Outputs};

/// Что вышло из события жеста.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SegmentEvent<T> {
    /// Событие не этого ряда.
    NotMine,
    /// Событие этого ряда, но ничего не сработало (подсветка нажатия, отмена).
    Consumed,
    /// Тап по сегменту с этим значением.
    Activated(T),
}

/// Из чего строится ряд: значения с подписями, подпись узлов в дереве сцены, как значение становится `ButtonId`.
pub(super) struct SegmentSpec<'a, T> {
    /// Сегменты по порядку: значение и подпись (тексты UI английские).
    pub entries: &'a [(T, &'static str)],
    /// Подпись узлов сегментов в дереве сцены (по ней тесты находят ряд).
    pub node_label: &'static str,
    /// Типизированное нажатие сегмента.
    pub id_of: fn(T) -> ButtonId,
    /// Роль текста подписи: вкладки колёс уже, чем сегменты переключателя вида, и берут более мелкий шрифт.
    pub text_role: TextRoleKind,
}

/// Один сегмент: значение, подпись и кнопка.
struct Segment<T> {
    value: T,
    label: &'static str,
    id: ButtonId,
    button: PressButton,
}

/// Ряд сегментов.
pub(super) struct SegmentRow<T> {
    segments: Vec<Segment<T>>,
    /// Ширина капсулы сегмента, pt (все равны).
    segment_width_pt: f64,
    text_role: TextRoleKind,
}

impl<T: Copy + PartialEq> SegmentRow<T> {
    /// Строит ряд внутри `parent` (рамка `parent_rect`); `row` — рамка ряда в системе корня. Высота ряда — зона
    /// касания сегментов, капсула рисуется ниже ростом `segment_height_pt` по центру.
    pub(super) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        row: Rect,
        spec: SegmentSpec<'_, T>,
    ) -> Result<Self, WidgetError> {
        let SegmentSpec {
            entries,
            node_label,
            id_of,
            text_role,
        } = spec;
        let gap = ctx.config.calibration.widgets.segment_gap_pt;
        let count = entries.len().max(1) as f64;
        let segment_width_pt = ((row.size.width - gap * (count - 1.0)) / count).max(0.0);
        let mut segments = Vec::with_capacity(entries.len());
        for (index, (value, text)) in entries.iter().enumerate() {
            let frame = Rect::from_xywh(
                row.min_x() + index as f64 * (segment_width_pt + gap),
                row.min_y(),
                segment_width_pt,
                row.size.height,
            );
            let node = kit.add_node(parent, node_label, frame, parent_rect)?;
            let id = id_of(*value);
            let button = PressButton::attach(kit, id, node, frame, ctx)?;
            segments.push(Segment {
                value: *value,
                label: text,
                id,
                button,
            });
        }
        Ok(Self {
            segments,
            segment_width_pt,
            text_role,
        })
    }

    /// Тап по сегменту → его значение.
    pub(super) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
    ) -> Result<SegmentEvent<T>, WidgetError> {
        let Some(segment) = self
            .segments
            .iter_mut()
            .find(|segment| event.intent == ScreenGesture::Press(segment.id))
        else {
            return Ok(SegmentEvent::NotMine);
        };
        let data = kit.gesture_data_in_node(segment.button.node(), event.data);
        Ok(
            if segment.button.handle(event.state, data, event.time)? == ButtonEvent::Activated {
                SegmentEvent::Activated(segment.value)
            } else {
                SegmentEvent::Consumed
            },
        )
    }

    /// Касания сегментам разрешены только пока есть факты (как точкам полос и строкам ручек).
    pub(super) fn set_touchable(&self, kit: &mut SceneKit, touchable: bool) -> Result<(), WidgetError> {
        for segment in &self.segments {
            kit.set_interactive(segment.button.node(), touchable)?;
        }
        Ok(())
    }

    /// Рисует капсулы и подписи. `selected` — выбранный сегмент (жёлтая подпись); `marked` — сегменты с меткой-кольцом
    /// («правлено»); `dim` — множитель прозрачности всего ряда.
    pub(super) fn draw(
        &self,
        ctx: &Ctx<'_>,
        (visuals, texts): (&VisualIndex, &mut TextRuns),
        layers: &mut DrawLayers,
        (selected, marked, dim): (T, impl Fn(T) -> bool, f32),
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let colors = &ctx.tokens.colors;
        for segment in &self.segments {
            let Some(placement) = visuals.placement(segment.button.node()) else {
                continue;
            };
            let Some(paint) = NodePaint::new(ctx, placement, dim)? else {
                continue;
            };
            let capsule_fill = ui_draw::color::DisplayColor {
                a: config.chip_fill_alpha,
                ..colors.text_primary
            };
            layers.push(
                Layer::Content,
                paint.capsule(
                    (0.0, 0.0),
                    (self.segment_width_pt, config.segment_height_pt),
                    capsule_fill,
                ),
            );
            let is_selected = segment.value == selected;
            let text_color = if is_selected {
                colors.focus_yellow
            } else {
                colors.text_primary
            };
            let run = texts.run(ctx, segment.label, self.text_role)?;
            layers.push(Layer::Content, paint.text_centered(run, 0.0, 0.0, text_color));
            if marked(segment.value) {
                let marker = config.wheel_tab_marker_diameter_pt;
                let corner_inset = marker / 2.0 + config.wheel_tab_marker_width_pt;
                let center = (
                    self.segment_width_pt / 2.0 - corner_inset,
                    -config.segment_height_pt / 2.0 + corner_inset,
                );
                layers.push(
                    Layer::Content,
                    paint.ring(center, marker, config.wheel_tab_marker_width_pt, colors.focus_yellow),
                );
            }
        }
        Ok(())
    }
}

/// Активация сегмента → действие владельца (общий разбор для вызывающих).
pub(super) fn push_if_activated<T>(
    event: SegmentEvent<T>,
    out: &mut Outputs,
    to_action: impl FnOnce(T) -> crate::action::ScreenAction,
) -> bool {
    match event {
        SegmentEvent::NotMine => false,
        SegmentEvent::Consumed => true,
        SegmentEvent::Activated(value) => {
            out.actions.push(to_action(value));
            true
        }
    }
}
