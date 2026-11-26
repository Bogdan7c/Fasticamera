//! «Картинка под пальцем» (решение 273): пока палец ведёт ручку или колесо, в колонке остаётся только ведомый орган
//! управления, остальное растворяется, чтобы палец не закрывал вид.
//!
//! # Владение и данные
//! Состояние живёт в колонке ручек (виджет), а не в `ScreenState`: это переходный вид одного жеста, он не должен
//! переживать пересборку раздела (поворот телефона, смена вида) — новая колонка рождается с чистым `FingerFocus`, и
//! колонка не бывает «застрявшей» в растворённом виде.
//!
//! # Как растворяется
//! Прозрачность в сцене перемножается вдоль дерева, поэтому растворить родителя нельзя: исчез бы и ведомый орган.
//! Вместо этого растворяются **корни ветвей, не содержащих ведомых узлов** (`fade_roots`): обход от панели вниз; узел —
//! предок ведомого — остаётся, его не ведомые дети растворяются каждый сам. Анимация — пружина токена движения
//! `CardAppear` через `SceneKit::fade_to` (Reduce Motion применяют токены). Рисуется ведомое на своей подложке
//! («поверхность раздела» без стекла), которая проявляется настолько, насколько растворилось остальное.
//!
//! # Касания
//! Растворённые узлы не принимают новых касаний: колонка после синхронизации выключает им хит-тест, а перед
//! следующей возвращает (`restore_interactivity`) — настоящее состояние («строка доступна / выключена») выставляет
//! синхронизация самих строк. Зоны касания ведомого органа не меняются ни на пиксель: его узлы не трогаются.

use ui_core::id::NodeId;

use ui_kit::tokens::MotionToken;

use crate::widget::Ctx;
use crate::widget::error::{WidgetError, element};
use crate::widget::kit::SceneKit;

/// Какой орган управления ведёт палец.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DrivenControl {
    /// Строка-линейка с номером слота (`RowTarget::slot`).
    Row(u8),
    /// Блок колеса: круг толчка, шкала кольца и строка названия с числами (вкладки растворяются).
    Wheels,
}

/// Переходное состояние «палец ведёт орган».
#[derive(Debug, Default)]
pub(super) struct FingerFocus {
    /// Ведомый орган и его узлы, пока идёт жест; `None` — палец ничего не ведёт.
    held: Option<(DrivenControl, Vec<NodeId>)>,
    /// Корни ветвей, растворённые сейчас (или растворявшиеся последний раз). Остаются в списке после отпускания,
    /// пока колонка живёт: возвращённая прозрачность на них идемпотентна, а первый из них служит датчиком.
    faded: Vec<NodeId>,
    /// Корням уже выключен хит-тест: перед синхронизацией строк его надо вернуть.
    interactivity_disabled: bool,
    /// Сколько раз палец взял орган, у которого не нашлось ни одного ведомого узла (диагностика, должно быть 0).
    refused_without_nodes: u64,
}

/// Итог попытки начать «картинку под пальцем».
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FocusBegin {
    /// Фокус начат: остальное растворится.
    Started,
    /// Ведомых узлов нет (слот органа не найден): фокус НЕ начат. С пустым списком растворилась бы вся панель, включая
    /// окно списка, и палец остался бы без управления.
    RefusedWithoutNodes,
}

impl FingerFocus {
    /// Палец взял орган `driven`, ведомые узлы — `nodes`. Пустой список фокус не начинает (см. [`FocusBegin`]) и
    /// считается в диагностике, а не глотается молча.
    pub(super) fn begin(&mut self, driven: DrivenControl, nodes: Vec<NodeId>) -> FocusBegin {
        if nodes.is_empty() {
            self.refused_without_nodes += 1;
            return FocusBegin::RefusedWithoutNodes;
        }
        self.held = Some((driven, nodes));
        FocusBegin::Started
    }

    /// Сколько раз фокус отказан из-за пустого списка ведомых узлов (диагностика, на нормальном пути 0).
    pub(super) fn refused_without_nodes(&self) -> u64 {
        self.refused_without_nodes
    }

    /// Палец отпустил орган `driven`. Отпускание ДРУГОГО органа (второй палец, сорвавшийся жест) фокус не снимает.
    pub(super) fn end(&mut self, driven: DrivenControl) {
        if self.held.as_ref().is_some_and(|(held, _)| *held == driven) {
            self.held = None;
        }
    }

    /// Ведомые узлы сейчас (пусто, если палец ничего не ведёт): им рисуется подложка.
    pub(super) fn driven_nodes(&self) -> &[NodeId] {
        self.held.as_ref().map_or(&[], |(_, nodes)| nodes.as_slice())
    }

    /// Датчик растворения: узел, прозрачность которого равна прозрачности «остального» (`None` — ещё ни разу не
    /// растворяли, остальное в покое).
    pub(super) fn probe(&self) -> Option<NodeId> {
        self.faded.first().copied()
    }

    /// Перед синхронизацией строк: вернуть корням хит-тест (строки сами выставят доступное им состояние).
    pub(super) fn restore_interactivity(&mut self, kit: &mut SceneKit) -> Result<(), WidgetError> {
        if self.interactivity_disabled {
            for root in &self.faded {
                kit.set_interactive(*root, true)?;
            }
            self.interactivity_disabled = false;
        }
        Ok(())
    }

    /// После синхронизации строк: привести растворение к желаемому. Нужен жест — корни растворяются к
    /// `others_opacity`, а их касания выключаются; жеста нет — всё возвращается к 1.
    pub(super) fn reconcile(
        &mut self,
        kit: &mut SceneKit,
        ctx: &Ctx<'_>,
        panel: NodeId,
        others_opacity: f64,
    ) -> Result<(), WidgetError> {
        let spring = ctx.spring(MotionToken::CardAppear)?;
        let Some((_, driven_nodes)) = &self.held else {
            for root in &self.faded {
                kit.fade_to(*root, 1.0, spring, ctx.now)?;
            }
            return Ok(());
        };
        let roots = fade_roots(kit, panel, driven_nodes)?;
        // Корни прошлого жеста, которых в новом нет (жест сменился на другой орган), возвращаются.
        for old in self.faded.iter().filter(|old| !roots.contains(old)) {
            kit.fade_to(*old, 1.0, spring, ctx.now)?;
        }
        for root in &roots {
            kit.fade_to(*root, others_opacity, spring, ctx.now)?;
            kit.set_interactive(*root, false)?;
        }
        self.interactivity_disabled = true;
        self.faded = roots;
        Ok(())
    }
}

/// Корни растворяемых ветвей: обход от `panel` вниз; предки ведомых узлов остаются, их остальные дети — корни.
fn fade_roots(kit: &SceneKit, panel: NodeId, driven_nodes: &[NodeId]) -> Result<Vec<NodeId>, WidgetError> {
    let tree = kit.scene().tree();
    // Предки ведомых узлов (включая сами ведомые): ветви без них растворяются целиком.
    let mut keep = Vec::new();
    for driven in driven_nodes {
        let mut current = Some(*driven);
        while let Some(node) = current {
            if node == panel {
                break;
            }
            keep.push(node);
            current = tree.parent(node).map_err(element("дерево сцены"))?;
        }
    }
    let mut roots = Vec::new();
    let mut pending = vec![panel];
    while let Some(node) = pending.pop() {
        for child in tree.children(node).map_err(element("дерево сцены"))? {
            if driven_nodes.contains(child) {
                continue;
            }
            if keep.contains(child) {
                pending.push(*child);
            } else {
                roots.push(*child);
            }
        }
    }
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use ui_core::geometry::Insets;
    use ui_core::scene::{SceneSettings, SurfaceGeometry, UiScene};

    use super::*;
    use crate::widget::intent::ScreenGesture;

    fn scene_node() -> NodeId {
        let surface = SurfaceGeometry {
            width_px: 100,
            height_px: 100,
            safe_area: Insets::ZERO,
        };
        let mut scene: UiScene<ScreenGesture> = UiScene::new(surface, SceneSettings::default()).expect("сцена");
        let root = scene.tree().root();
        scene.insert_child(root, "ведомый").expect("узел")
    }

    #[test]
    fn a_grab_with_nodes_starts_the_focus_and_the_release_of_the_same_organ_ends_it() {
        let mut focus = FingerFocus::default();
        let node = scene_node();
        assert_eq!(focus.begin(DrivenControl::Row(2), vec![node]), FocusBegin::Started);
        assert_eq!(focus.driven_nodes(), [node]);
        focus.end(DrivenControl::Row(3));
        assert_eq!(
            focus.driven_nodes(),
            [node],
            "отпускание другого органа фокус не снимает"
        );
        focus.end(DrivenControl::Row(2));
        assert!(focus.driven_nodes().is_empty());
        assert_eq!(focus.refused_without_nodes(), 0);
    }

    #[test]
    fn a_grab_without_driven_nodes_is_refused_counted_and_keeps_the_previous_focus_untouched() {
        let mut focus = FingerFocus::default();
        assert_eq!(
            focus.begin(DrivenControl::Row(9), Vec::new()),
            FocusBegin::RefusedWithoutNodes,
            "слот не найден: растворять нечего без ведомого"
        );
        assert!(focus.driven_nodes().is_empty(), "фокус не начат");
        assert_eq!(focus.refused_without_nodes(), 1, "отказ виден в диагностике");
        // Второй палец с пустым списком не стирает фокус первого.
        let node = scene_node();
        assert_eq!(focus.begin(DrivenControl::Row(1), vec![node]), FocusBegin::Started);
        assert_eq!(
            focus.begin(DrivenControl::Row(9), Vec::new()),
            FocusBegin::RefusedWithoutNodes
        );
        assert_eq!(focus.driven_nodes(), [node]);
        assert_eq!(focus.refused_without_nodes(), 2);
    }
}
