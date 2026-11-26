//! Соседство областей касания раздела калибровки (пункт 13 сессии 14n, решения 278–281).
//!
//! Два правила для каждого живого элемента раздела во всех положениях телефона, обоих видах колонки, на каждой
//! странице прибора и при открытой карточке меню:
//! 1. расширенная зона маленькой кнопки живая целиком — предки её не режут (мёртвых краёв нет, решение 278);
//! 2. в каждой точке действующей области касание ловит сам элемент или вложенный в него (правило «глубже —
//!    главнее» для вложенных); чужой сосед или пустота в чужой области — нарушение (решения 279, 280).

use look_contract::LookFacts;
use scope_contract::ScopeKind;
use shell_contract::orientation::DeviceOrientation;

use super::screen_calibration_tests::calibrating_with;
use super::test_rig::Rig;
use super::touch_map_test_support::{TouchMap, describe_conflicts};
use crate::action::ScreenAction;
use crate::calibration::{ControlViewKind, KnobsSide};
use crate::config::ScreenConfig;
use crate::look_test_support::look_facts;

const ORIENTATIONS: [DeviceOrientation; 3] = [
    DeviceOrientation::Portrait,
    DeviceOrientation::LandscapeLeft,
    DeviceOrientation::LandscapeRight,
];
/// Корень раздела калибровки в сцене.
const SECTION_ROOT: &str = "calibration_root";
const VIEWS: [ControlViewKind; 2] = [ControlViewKind::Scales, ControlViewKind::Wheels];

/// Проверяет оба правила на текущем кадре и возвращает текст нарушений (пусто — соседство чистое).
fn neighbourhood_problems(rig: &Rig) -> String {
    let scene = rig.screen.scene_for_tests().expect("сцена построена");
    let map = TouchMap::of_subtree(scene, SECTION_ROOT);
    let cut: Vec<String> = map
        .cut_expansions()
        .iter()
        .map(|area| format!("{} урезана {:?} → {:?}", area.label, area.declared, area.effective))
        .collect();
    let conflicts = map.conflicts();
    let mut problems = Vec::new();
    if !cut.is_empty() {
        problems.push(format!("мёртвые края: {}", cut.join("; ")));
    }
    if !conflicts.is_empty() {
        problems.push(format!("чужие точки: {}", describe_conflicts(&conflicts)));
    }
    problems.join("\n")
}

fn assert_clean(rig: &Rig, case: &str) {
    let problems = neighbourhood_problems(rig);
    assert!(problems.is_empty(), "{case}:\n{problems}");
}

const SIDES: [KnobsSide; 2] = [KnobsSide::Left, KnobsSide::Right];

fn section_on_side(side: KnobsSide, facts: LookFacts, orientation: DeviceOrientation, view: ControlViewKind) -> Rig {
    let mut config = ScreenConfig::default();
    config.calibration.layout.knobs_side = side;
    let mut rig = calibrating_with(config, facts, orientation);
    rig.act(ScreenAction::ControlViewSelected(view));
    rig.run(400);
    rig
}

fn section(facts: LookFacts, orientation: DeviceOrientation, view: ControlViewKind) -> Rig {
    section_on_side(KnobsSide::Left, facts, orientation, view)
}

#[test]
fn every_live_area_of_the_section_belongs_to_its_element_in_every_orientation_and_view() {
    for side in SIDES {
        for orientation in ORIENTATIONS {
            for view in VIEWS {
                let mut rig = section_on_side(side, look_facts(), orientation, view);
                for kind in ScopeKind::ALL {
                    rig.act(ScreenAction::ScopeKindSelected(kind));
                    rig.run(400);
                    assert_clean(&rig, &format!("{side:?}, {orientation:?}, {view:?}, {kind:?}"));
                }
            }
        }
    }
}

/// Подписи слоя открытого меню: затемнение, карточка пресетов и её кнопки и таблетки.
const MENU_LAYER: [&str; 6] = [
    "calib_menu_scrim",
    "calib_menu_card",
    "card_button",
    "calib_chip",
    "chip_row_window",
    "calib_action_chip",
];

#[test]
fn the_open_menu_is_modal_every_section_point_belongs_to_the_menu_layer() {
    // Решение 281: открытое меню модальное — ни капсула режимов, ни ручки под затемнением касание не получают.
    for (side, orientation) in SIDES
        .into_iter()
        .flat_map(|side| ORIENTATIONS.map(|orientation| (side, orientation)))
    {
        let mut rig = section_on_side(side, look_facts(), orientation, ControlViewKind::Scales);
        rig.act(ScreenAction::ControlsMenuOpenRequested);
        rig.run(600);
        let scene = rig.screen.scene_for_tests().expect("сцена построена");
        let map = TouchMap::of_subtree(scene, SECTION_ROOT);
        let leaks: Vec<String> = map
            .conflicts()
            .into_iter()
            .filter(|conflict| !MENU_LAYER.contains(&conflict.area))
            .filter(|conflict| !conflict.caught_by.is_some_and(|label| MENU_LAYER.contains(&label)))
            .map(|conflict| {
                format!(
                    "{} в ({:.1}, {:.1}) ловит {:?}",
                    conflict.area, conflict.at.x, conflict.at.y, conflict.caught_by
                )
            })
            .take(5)
            .collect();
        assert!(
            leaks.is_empty(),
            "{side:?} {orientation:?}: касание проходит мимо меню: {leaks:?}"
        );
        let menu_conflicts: Vec<_> = map
            .conflicts()
            .into_iter()
            .filter(|conflict| MENU_LAYER.contains(&conflict.area))
            .filter(|conflict| conflict.area != "calib_menu_scrim")
            .collect();
        assert!(
            menu_conflicts.is_empty(),
            "{side:?} {orientation:?}: внутри меню: {}",
            describe_conflicts(&menu_conflicts)
        );
    }
}

#[test]
fn the_touch_map_sees_every_kind_of_live_section_element() {
    // Опора самой проверки: если карта перестанет видеть элементы раздела, оба теста выше станут пустыми.
    let rig = section(look_facts(), DeviceOrientation::Portrait, ControlViewKind::Scales);
    let scene = rig.screen.scene_for_tests().expect("сцена построена");
    let map = TouchMap::of_subtree(scene, SECTION_ROOT);
    let labels: Vec<&str> = map.live().iter().map(|area| area.label).collect();
    for expected in [
        "calib_ruler",
        "calib_reset",
        "calib_band_dot",
        "calib_scope",
        "calib_scope_page_dot",
        "calib_clean",
        "mode_capsule",
    ] {
        assert!(
            labels.contains(&expected),
            "живой «{expected}» не найден среди {labels:?}"
        );
    }
}
