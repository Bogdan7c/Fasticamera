//! Поза узла из ряда затвора: растворение под шторкой Controls и подъём над нижней карточкой.
//!
//! Ряд затвора — это сам затвор и кнопка белого (WB35). Они обязаны двигаться как одно целое: расстояние между
//! ними постоянно в каждом кадре анимации, поэтому правило видимости, высота подъёма, пружина и момент старта
//! живут здесь один раз, а не копируются в каждый виджет. Виджет отдаёт сюда свой узел и получает обратно
//! текущий визуальный сдвиг, по которому ставит рамку трекера нажатия.
//!
//! # Подъём
//! Рамка узла не меняется — меняется визуальный сдвиг `TranslationY` (пружина `CardMorph`, без перелёта).
//! Высота подъёма зависит от открытой карточки (обычная — 100 pt, белая — на 24 pt выше, WB28): смена карточки при
//! открытой панели переезжает той же пружиной, что и стекло карточки (`tool_card.rs`). `ui-core` учитывает сдвиг
//! узла в хит-тесте, поэтому зона касания едет вместе с картинкой.

use ui_core::time::MotionPreference;
use ui_kit::tokens::MotionToken;

use super::Ctx;
use super::error::WidgetError;
use super::kit::SceneKit;
use super::tool_card_content::open_card_tool;
use crate::state::ShutterVisibility;

/// Поза одного узла ряда затвора.
#[derive(Debug, Default)]
pub(crate) struct ShutterRowPose {
    /// К какой видимости едем сейчас (`None` — ещё не назначали).
    visibility: Option<ShutterVisibility>,
    /// Куда назначен сдвиг по вертикали, pt (`None` — ещё не назначали): подъём зависит и от видимости, и от
    /// открытой карточки, поэтому сравнивается само число.
    raise_target_pt: Option<f64>,
}

impl ShutterRowPose {
    /// Выравнивает узел по состоянию экрана и возвращает его текущий визуальный сдвиг по вертикали, pt
    /// (отрицательный — вверх). Под шторкой Controls узел сразу перестаёт ловить касания и растворяется пружиной.
    pub(crate) fn sync(
        &mut self,
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        node: ui_core::id::NodeId,
    ) -> Result<f64, WidgetError> {
        let visibility = ctx.state.shutter_visibility();
        if self.visibility != Some(visibility) {
            self.visibility = Some(visibility);
            let visible = visibility != ShutterVisibility::HiddenUnderDrawer;
            // Нажиматься перестаёт сразу, растворяется пружиной.
            kit.set_interactive(node, visible)?;
            kit.fade_to(
                node,
                if visible { 1.0 } else { 0.0 },
                ctx.spring(MotionToken::Snappy)?,
                ctx.now,
            )?;
        }
        let raised = visibility == ShutterVisibility::RaisedAboveCard;
        let offset_pt = if raised {
            -ctx.geometry.raised_shutter_offset_pt(open_card_tool(ctx.state))
        } else {
            0.0
        };
        if self.raise_target_pt.map(f64::to_bits) != Some(offset_pt.to_bits()) {
            self.raise_target_pt = Some(offset_pt);
            if ctx.motion == MotionPreference::Reduced {
                // Reduce Motion: без перемещений, узел сразу на новом месте (прозрачность по-прежнему плавная).
                kit.jump_vertically_to(node, offset_pt)?;
            } else {
                // Подъём и возврат: без перелёта, ≈ 0,3 с, как морф карточки `[measured: C t=54.0]`.
                kit.slide_vertically_to(node, offset_pt, ctx.spring(MotionToken::CardMorph)?, ctx.now)?;
            }
        }
        kit.vertical_offset_pt(node)
    }
}
