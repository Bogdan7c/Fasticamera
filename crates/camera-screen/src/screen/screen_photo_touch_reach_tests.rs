//! PHOTO после решения 278: расширенные зоны маленьких кнопок работают и за краем родителя. Картинка PHOTO от этого
//! не меняется, а касания меняются только там, где раньше был мёртвый край: ожившая полоса не должна лечь на
//! чужой элемент и не должна отнять у него ни одной точки.

use super::test_rig::Rig;
use super::touch_map_test_support::{TouchMap, describe_conflicts};
use crate::action::ScreenAction;

/// Корень PHOTO в сцене (в CALIBRATE он скрыт, а не удалён).
const PHOTO_ROOT: &str = "photo_root";

fn photo_states() -> Vec<(&'static str, Rig)> {
    let mut closed = Rig::new();
    closed.run(400);
    let mut menu = Rig::new();
    menu.act(ScreenAction::ControlsMenuOpenRequested);
    menu.run(600);
    let mut white = Rig::new();
    white.act(ScreenAction::WhiteButtonTapped);
    white.run(600);
    vec![("покой", closed), ("меню Controls", menu), ("карточка белого", white)]
}

#[test]
fn photo_expanded_areas_steal_no_point_from_another_element() {
    let mut checked_expanded = 0;
    for (case, rig) in photo_states() {
        let scene = rig.screen.scene_for_tests().expect("сцена построена");
        let map = TouchMap::of_subtree(scene, PHOTO_ROOT);
        let expanded: Vec<&'static str> = map
            .live()
            .iter()
            .filter(|area| area.expanded)
            .map(|area| area.label)
            .collect();
        checked_expanded += expanded.len();
        let conflicts = map.conflicts();
        // Нарушение — только то, что связано с расширенными зонами: расширенная кнопка ловит в чужой области или
        // сама теряет точки своей зоны. Прочие наложения PHOTO (видоискатель под кнопками) решение 278 не меняет.
        let involving_expanded: Vec<_> = conflicts
            .into_iter()
            .filter(|conflict| {
                expanded.contains(&conflict.area) || conflict.caught_by.is_some_and(|label| expanded.contains(&label))
            })
            .filter(|conflict| conflict.area != "viewfinder")
            .collect();
        assert!(
            involving_expanded.is_empty(),
            "{case}: {}",
            describe_conflicts(&involving_expanded)
        );
    }
    assert!(
        checked_expanded > 0,
        "в проверенных состояниях PHOTO нет ни одной расширенной кнопки — проверка пуста"
    );
}
