//! Нижний ряд (миниатюра, капсула режимов, кнопка меню) уходит из кадра, пока открыта нижняя карточка
//! (решение 103): узлы растворяются пружиной и сразу перестают ловить касания. Затвор в ряд не входит:
//! он поднимается над карточкой (`shutter_button.rs`).

use ui_core::id::NodeId;
use ui_kit::tokens::MotionToken;

use super::Ctx;
use super::error::WidgetError;
use super::kit::SceneKit;

/// Пружина растворения ряда: без перелёта (прозрачность не выходит за 0..1), ≈ 0,2 с `[measured: C t=47.93–48.12]`.
const ROW_FADE_MOTION: MotionToken = MotionToken::CardAppear;

/// Следит за тем, чтобы узел ряда растворялся ровно тогда, когда состояние экрана прячет нижний ряд.
#[derive(Debug, Default)]
pub(crate) struct BottomRowFade {
    /// Куда едем сейчас (`None` — ещё не назначали).
    hidden: Option<bool>,
}

impl BottomRowFade {
    /// Выравнивает прозрачность узла по состоянию. Возвращает `true`, если ряд должен быть виден и нажимаем
    /// (виджет добавляет к этому свои условия нажимаемости).
    pub(crate) fn sync(&mut self, kit: &mut SceneKit, ctx: &Ctx<'_>, node: NodeId) -> Result<bool, WidgetError> {
        let hidden = ctx.state.is_bottom_row_hidden();
        if self.hidden != Some(hidden) {
            self.hidden = Some(hidden);
            kit.fade_to(
                node,
                if hidden { 0.0 } else { 1.0 },
                ctx.spring(ROW_FADE_MOTION)?,
                ctx.now,
            )?;
        }
        Ok(!hidden)
    }
}
