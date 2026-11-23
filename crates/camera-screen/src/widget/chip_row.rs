//! Ряд «таблеток» — коротких подписей в капсулах, одна из которых может быть выбрана (решения 84, 201).
//!
//! Один элемент на три места, чтобы не плодить похожий код:
//! * ряд имён пресетов в карточке Styles экрана PHOTO (решение 201: «ряд прокручивается»);
//! * ряд имён пресетов и ряд стандарта осей вектороскопа в карточках меню раздела калибровки;
//! * ряд действий пресета (NEW / RENAME / SAVE / DELETE) там же.
//!
//! # Что делает ряд и чего не делает
//! * Ряд владеет только устройством: узлами сцены, раскладкой таблеток по ширине, прокруткой и рисованием. ЧТО
//!   написано, какая таблетка выбрана и что значит нажатие — решает владелец (карточка), он отдаёт набор
//!   [`ChipFace`] и получает [`RowEvent::Activated`] с номером таблетки.
//! * Таблетки раскладываются подряд; пока все помещаются — стоят по центру окна, иначе ряд едет за пальцем
//!   (`ui_kit::scroll_list::ScrollList` вдоль горизонтальной оси), а у краёв окна таблетки гаснут по доле видимой
//!   части. Узел окна ограничивает касания своей рамкой: таблетку, уехавшую за край, нажать нельзя.
//! * Узлов ровно `chip_pool_size` (конфиг): больше имён — лишние не показываются, а их число доступно через
//!   [`ChipRow::overflowed_chips`] для диагностики, а не теряется молча.
//! * Ряд живёт в одном из двух «домов»: на экране PHOTO (оси жестов совпадают с осями экрана) и внутри раздела
//!   калибровки (повёрнутый корень: жесты переводятся в глазные оси через [`SectionSpace`]).
//!
//! # Жизненный цикл при закрытии карточки
//! Как у `OptionRow`: касания снимаются сразу ([`ChipRow::stop_touches`]), а содержимое забывается позже
//! ([`ChipRow::forget`]), когда карточка дорисовала растворение.

use ui_core::geometry::{Point, Rect, Size};
use ui_core::gesture::{GestureEvent, PanSpec, RecognizerSpec};
use ui_core::id::NodeId;
use ui_draw::color::{DisplayColor, Opacity};
use ui_draw::draw_list::{DrawItem, ShapeItem};
use ui_draw::geometry::PxSize;
use ui_draw::shape::ShapeGeometry;
use ui_draw::transform::Affine2;
use ui_kit::pan::PanUpdate;
use ui_kit::scroll_list::{ScrollAxis, ScrollList};
use ui_kit::tokens::TextRoleKind;
use ui_kit::view::{LabelPainter, centered_text};

use super::Ctx;
use super::button::{ButtonEvent, PressButton};
use super::calibration::space::{EyeAxis, SectionSpace};
use super::error::{WidgetError, element};
use super::intent::{ButtonId, ScreenGesture};
use super::kit::SceneKit;
use super::layers::{DrawLayers, Layer, VisualIndex};
use super::text_runs::{TextRuns, displayable_text};

/// Как выглядит и ведёт себя таблетка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChipTone {
    /// Обычная: белая подпись.
    Plain,
    /// Выбранная (активный пресет, текущий стандарт осей) или взведённая (DELETE после первого тапа): жёлтая подпись.
    Highlighted,
    /// Недоступная сейчас (SAVE без правок, RENAME у встроенного): приглушена и не ловит касаний.
    Disabled,
}

/// Одна таблетка: подпись и вид.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChipFace {
    pub text: String,
    pub tone: ChipTone,
}

impl ChipFace {
    pub(crate) fn new(text: impl Into<String>, tone: ChipTone) -> Self {
        Self {
            text: text.into(),
            tone,
        }
    }
}

/// Чем ряд отличается от других таких же: идентификаторы нажатий и пан.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ChipRowIds {
    /// Нажатие таблетки по её номеру в ряду.
    pub chip: fn(u8) -> ButtonId,
    /// Пан ряда; `None` — ряд не прокручивается (ряд действий всегда помещается).
    pub pan: Option<ScreenGesture>,
    /// Подпись узлов таблеток в дереве сцены (по ней тесты и диагностика находят ряд).
    pub chip_label: &'static str,
}

/// Что вышло из события жеста.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowEvent {
    /// Событие не этого ряда.
    NotMine,
    /// Событие этого ряда, но ничего не сработало (подсветка, пан).
    Consumed,
    /// Тап по таблетке с этим номером (порядок как в [`ChipRow::set_faces`]).
    Activated(usize),
}

/// Место таблетки: левый край относительно левого края окна и ширина.
#[derive(Debug, Clone, Copy)]
struct ChipSpan {
    left_pt: f64,
    width_pt: f64,
}

struct ChipSlot {
    button: PressButton,
}

/// Прозрачность и сдвиг содержимого карточки, в которой стоит ряд (из её морфа).
#[derive(Debug, Clone, Copy)]
pub(crate) struct RowPaint {
    pub alpha: f32,
    pub slide_px: f32,
}

/// Ряд таблеток.
pub(crate) struct ChipRow {
    /// Пространство раздела; `None` — ряд на экране PHOTO.
    space: Option<SectionSpace>,
    ids: ChipRowIds,
    window_rect: Rect,
    /// Узел окна: ловит пан и ограничивает касания таблеток своей рамкой, поэтому выключен вместе с рядом.
    window: NodeId,
    content: NodeId,
    slots: Vec<ChipSlot>,
    faces: Vec<ChipFace>,
    /// Подписи в том виде, как их прислал владелец (до замены символов вне атласа): по ним ряд узнаёт, что состав не менялся.
    raw_texts: Vec<String>,
    spans: Vec<ChipSpan>,
    list: ScrollList,
    /// Разрешены ли касания (владелец снимает их, пока карточка закрывается).
    touchable: bool,
    /// Сколько таблеток не поместилось в пул узлов.
    overflowed: usize,
}

impl ChipRow {
    /// Строит ряд с центром окна `center` и шириной окна `width_pt` внутри `(parent, parent_rect)`. Высота окна —
    /// не меньше цели касания HIG (44 pt): окно ограничивает касания, и таблетки не должны быть тоньше.
    pub(crate) fn build(
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        (parent, parent_rect): (NodeId, Rect),
        (center, width_pt): (Point, f64),
        (ids, space): (ChipRowIds, Option<SectionSpace>),
    ) -> Result<Self, WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let touch_side = ctx.geometry.config().card.button_touch_side_pt;
        let window_rect = Rect::centered_at(center, Size::new(width_pt, config.chip_height_pt.max(touch_side)));
        let window = kit.add_node(parent, "chip_row_window", window_rect, parent_rect)?;
        if let Some(pan) = ids.pan {
            let direction = match space {
                Some(space) => space.pan_direction(EyeAxis::Horizontal),
                None => ui_core::gesture::PanDirection::Horizontal,
            };
            kit.add_recognizer(
                window,
                RecognizerSpec::Pan(PanSpec {
                    direction,
                    ..PanSpec::default()
                }),
                pan,
            )?;
        }
        kit.set_interactive(window, false)?;
        let content = kit.add_node(window, "chip_row_content", window_rect, window_rect)?;
        let mut slots = Vec::with_capacity(config.chip_pool_size);
        for index in 0..config.chip_pool_size {
            let slot_id = u8::try_from(index).map_err(|_| {
                WidgetError::Inconsistent(format!("в ряду таблеток {index} узлов, номер не влезает в u8"))
            })?;
            let rect = Rect::centered_at(center, Size::new(config.chip_height_pt, config.chip_height_pt));
            let node = kit.add_node(content, ids.chip_label, rect, window_rect)?;
            let button = PressButton::attach(kit, (ids.chip)(slot_id), node, rect, ctx)?;
            kit.ensure_min_touch_side(node, rect, touch_side)?;
            kit.set_interactive(node, false)?;
            slots.push(ChipSlot { button });
        }
        let list = ScrollList::along(
            ScrollAxis::Horizontal,
            width_pt,
            0.0,
            &ctx.environment(width_pt),
            ctx.now,
        )
        .map_err(element("ряд таблеток"))?;
        Ok(Self {
            space,
            ids,
            window_rect,
            window,
            content,
            slots,
            faces: Vec::new(),
            raw_texts: Vec::new(),
            spans: Vec::new(),
            list,
            touchable: false,
            overflowed: 0,
        })
    }

    /// Сколько таблеток не поместилось в пул узлов (диагностика).
    pub(crate) fn overflowed_chips(&self) -> usize {
        self.overflowed
    }

    /// Задаёт содержимое ряда. Если подписи те же, меняются только виды (без перекладки и без прокрутки); иначе ряд
    /// перекладывается, а окно сразу показывает первую выделенную таблетку (активный пресет не должен прятаться за
    /// краем при открытии карточки).
    pub(crate) fn set_faces(
        &mut self,
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        mut faces: Vec<ChipFace>,
    ) -> Result<(), WidgetError> {
        let pool = self.slots.len();
        self.overflowed = faces.len().saturating_sub(pool);
        faces.truncate(pool);
        let same_texts =
            faces.len() == self.raw_texts.len() && faces.iter().zip(&self.raw_texts).all(|(new, old)| &new.text == old);
        if same_texts {
            // Подписи те же: оставляем уже подготовленные к рисованию, меняются только виды.
            for (face, old) in faces.iter_mut().zip(&self.faces) {
                face.text.clone_from(&old.text);
            }
            self.faces = faces;
            return self.apply_interactivity(kit);
        }
        let raw_texts: Vec<String> = faces.iter().map(|face| face.text.clone()).collect();
        for face in &mut faces {
            face.text = displayable_text(ctx, &face.text);
        }
        let config = &ctx.config.calibration.widgets;
        let painter = LabelPainter::new(&ctx.tokens.typography, ctx.px);
        let mut widths = Vec::with_capacity(faces.len());
        for face in &faces {
            let run = painter.layout(&face.text, TextRoleKind::CardOption)?;
            let text_width_pt = f64::from(run.advance_px / ctx.px.get());
            widths.push((text_width_pt + 2.0 * config.chip_padding_pt).max(config.chip_height_pt));
        }
        let total_pt = widths.iter().sum::<f64>() + faces.len().saturating_sub(1) as f64 * config.chip_gap_pt;
        let window_width = self.window_rect.size.width;
        let fits = total_pt <= window_width;
        // Помещается — ряд стоит по центру окна; иначе начинается у левого края и едет за пальцем.
        let mut left = if fits { (window_width - total_pt) / 2.0 } else { 0.0 };
        self.spans = widths
            .iter()
            .map(|width| {
                let span = ChipSpan {
                    left_pt: left,
                    width_pt: *width,
                };
                left += width + config.chip_gap_pt;
                span
            })
            .collect();
        let content_width = if fits { window_width } else { total_pt };
        let content_rect = Rect::from_xywh(
            self.window_rect.min_x(),
            self.window_rect.min_y(),
            content_width,
            self.window_rect.size.height,
        );
        kit.move_node(self.content, content_rect, self.window_rect)?;
        let chip_top = self.window_rect.center().y - config.chip_height_pt / 2.0;
        for (slot, span) in self.slots.iter_mut().zip(&self.spans) {
            let rect = Rect::from_xywh(
                self.window_rect.min_x() + span.left_pt,
                chip_top,
                span.width_pt,
                config.chip_height_pt,
            );
            kit.move_node(slot.button.node(), rect, content_rect)?;
            slot.button.set_frame(rect)?;
            kit.ensure_min_touch_side(
                slot.button.node(),
                rect,
                ctx.geometry.config().card.button_touch_side_pt,
            )?;
        }
        let reveal_center = faces
            .iter()
            .position(|face| face.tone == ChipTone::Highlighted)
            .and_then(|index| self.spans.get(index))
            .map_or(0.0, |span| span.left_pt + span.width_pt / 2.0 - window_width / 2.0);
        self.list = ScrollList::along(
            ScrollAxis::Horizontal,
            content_width,
            reveal_center,
            &ctx.environment(window_width),
            ctx.now,
        )
        .map_err(element("ряд таблеток"))?;
        self.faces = faces;
        self.raw_texts = raw_texts;
        self.apply_interactivity(kit)
    }

    /// Разрешает или запрещает касания всему ряду; занятые доступные таблетки ловят их, когда разрешено.
    pub(crate) fn set_touchable(&mut self, kit: &mut SceneKit, touchable: bool) -> Result<(), WidgetError> {
        self.touchable = touchable;
        self.apply_interactivity(kit)
    }

    /// Касания сняты, содержимое осталось (карточка ещё растворяется).
    pub(crate) fn stop_touches(&mut self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        self.set_touchable(kit, false)
    }

    /// Ряд забывает содержимое: карточка закрылась целиком.
    pub(crate) fn forget(&mut self) {
        self.faces.clear();
        self.raw_texts.clear();
        self.spans.clear();
        self.overflowed = 0;
    }

    fn apply_interactivity(&mut self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        kit.set_interactive(self.window, self.touchable)?;
        for (index, slot) in self.slots.iter().enumerate() {
            let usable = self
                .faces
                .get(index)
                .is_some_and(|face| face.tone != ChipTone::Disabled);
            kit.set_interactive(slot.button.node(), self.touchable && usable)?;
        }
        Ok(())
    }

    /// Событие жеста ряда. `kit` нужен, чтобы перевести положение пальца в систему рамки таблетки (ряд едет и,
    /// внутри раздела, повёрнут).
    pub(crate) fn handle_gesture(
        &mut self,
        kit: &SceneKit,
        event: &GestureEvent<ScreenGesture>,
    ) -> Result<RowEvent, WidgetError> {
        if Some(event.intent) == self.ids.pan {
            if let Some(update) =
                PanUpdate::from_gesture(event.state, event.data).map_err(element("пан ряда таблеток"))?
            {
                let update = self.space.map_or(update, |space| space.eye_pan(update));
                self.list
                    .apply_pan(event.time, update)
                    .map_err(element("ряд таблеток"))?;
            }
            return Ok(RowEvent::Consumed);
        }
        let Some(index) = (0..self.slots.len()).find(|index| {
            u8::try_from(*index).is_ok_and(|id| event.intent == ScreenGesture::Press((self.ids.chip)(id)))
        }) else {
            return Ok(RowEvent::NotMine);
        };
        let Some(slot) = self.slots.get_mut(index) else {
            return Ok(RowEvent::NotMine);
        };
        let data = kit.gesture_data_in_node(slot.button.node(), event.data);
        let activated = slot.button.handle(event.state, data, event.time)? == ButtonEvent::Activated;
        let usable = self
            .faces
            .get(index)
            .is_some_and(|face| face.tone != ChipTone::Disabled);
        Ok(if activated && usable {
            RowEvent::Activated(index)
        } else {
            RowEvent::Consumed
        })
    }

    /// Ставит ряд за списком: смещение прокрутки — в узел содержимого.
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>) -> Result<(), WidgetError> {
        kit.jump_horizontally_to(self.content, -self.list.offset(ctx.now))
    }

    /// Рисует таблетки: заливка-капсула и подпись. `paint` — прозрачность и сдвиг карточки; таблетки у края окна
    /// гаснут по доле видимой части (в квадрате, как подписи карусели).
    pub(crate) fn draw(
        &self,
        ctx: &Ctx<'_>,
        visuals: &VisualIndex,
        layers: &mut DrawLayers,
        texts: &mut TextRuns,
        paint: RowPaint,
    ) -> Result<(), WidgetError> {
        let config = &ctx.config.calibration.widgets;
        let colors = &ctx.tokens.colors;
        let offset = self.list.offset(ctx.now);
        let window_width = self.window_rect.size.width;
        let slide = Affine2::translate(0.0, paint.slide_px);
        for ((slot, face), span) in self.slots.iter().zip(&self.faces).zip(&self.spans) {
            let Some(placement) = visuals.placement(slot.button.node()) else {
                continue;
            };
            let left = span.left_pt - offset;
            let visible = ((left + span.width_pt).min(window_width) - left.max(0.0)) / span.width_pt;
            let edge_fade = (visible.clamp(0.0, 1.0) * visible.clamp(0.0, 1.0)) as f32;
            let tone_factor = if face.tone == ChipTone::Disabled {
                config.disabled_row_opacity
            } else {
                1.0
            };
            let alpha = (placement.opacity * paint.alpha * edge_fade * tone_factor).clamp(0.0, 1.0);
            if alpha <= 0.0 {
                continue;
            }
            let opacity = Opacity::new(alpha)
                .map_err(|_| WidgetError::Inconsistent("прозрачность таблетки вне 0..1".to_owned()))?;
            let host = slide.then(placement.transform);
            layers.push(
                Layer::Content,
                DrawItem::Shape(ShapeItem {
                    geometry: ShapeGeometry::Capsule {
                        size: PxSize::new(placement.size.width, placement.size.height),
                    },
                    transform: host,
                    fill: DisplayColor {
                        a: config.chip_fill_alpha,
                        ..colors.text_primary
                    },
                    stroke: None,
                    shadow: None,
                    opacity,
                }),
            );
            let color = if face.tone == ChipTone::Highlighted {
                colors.focus_yellow
            } else {
                colors.text_primary
            };
            let run = texts.run(ctx, &face.text, TextRoleKind::CardOption)?;
            layers.push(Layer::Content, centered_text(run, host, color, opacity));
        }
        Ok(())
    }
}
