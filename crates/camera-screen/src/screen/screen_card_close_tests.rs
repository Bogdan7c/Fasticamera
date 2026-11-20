//! Закрытие и повторное открытие нижней карточки, Reduce Motion: варианты гаснут вместе с карточкой, морф
//! «из шторки» не переиспользуется для карточки на месте, при Reduce Motion ничего не перемещается.

use ui_core::time::MotionPreference;
use ui_draw::draw_list::{DrawItem, DrawList};
use ui_draw::shape::ShapeGeometry;
use ui_kit::tokens::TextRoleKind;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::state::PanelState;
use crate::tools::ToolId;

/// Допуск сравнения координат, px.
const PX_TOLERANCE: f32 = 1.5;

/// Меню → карточка инструмента (действиями, без тапов) и покой.
fn open_card_from_menu(rig: &mut Rig, tool: ToolId) {
    rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
    rig.run(700);
    rig.act(ScreenAction::ControlsItemSelected(tool));
    rig.run(700);
}

/// Прозрачность первого нарисованного текста `text` роли `role`; `None`, если его в списке нет.
fn text_opacity(list: &DrawList, text: &str, role: TextRoleKind) -> Option<f32> {
    let expected = run_of(text, role);
    list.items.iter().find_map(|item| match item {
        DrawItem::Text(drawn) if drawn.run == expected => Some(drawn.opacity.get()),
        _ => None,
    })
}

/// Вертикальная позиция (px) первого нарисованного текста `text` роли `role`.
fn text_y(list: &DrawList, text: &str, role: TextRoleKind) -> Option<f32> {
    let expected = run_of(text, role);
    list.items.iter().find_map(|item| match item {
        DrawItem::Text(drawn) if drawn.run == expected => Some(drawn.transform.ty),
        _ => None,
    })
}

/// Центр по y (px) кольца затвора: стеклянный круг Ø > 150 px.
fn shutter_ring_y(list: &DrawList) -> f32 {
    list.items
        .iter()
        .find_map(|item| match item {
            DrawItem::Glass(group) => group.members.iter().find_map(|member| match member.geometry {
                ShapeGeometry::Circle { diameter } if diameter > 150.0 => Some(member.transform.ty),
                _ => None,
            }),
            _ => None,
        })
        .expect("кольцо затвора в списке рисования")
}

/// Закрывающие действия, от которых ряд вариантов обязан гаснуть вместе с карточкой.
fn closing_actions() -> [(&'static str, ScreenAction); 2] {
    [
        ("«‹» назад в меню", ScreenAction::SubmenuBackRequested),
        ("свайп вниз", ScreenAction::ControlsMenuCloseRequested),
    ]
}

#[test]
fn option_row_fades_out_together_with_the_card_instead_of_vanishing_on_the_first_frame() {
    for (name, close) in closing_actions() {
        let mut rig = Rig::new();
        open_card_from_menu(&mut rig, ToolId::Aspect);
        let settled = text_opacity(&rig.last, "16:9", TextRoleKind::CardOption).expect("вариант виден в покое");
        assert!(settled > 0.99, "{name}: в покое вариант непрозрачен, а не {settled}");

        rig.act(close);
        let mut opacities = Vec::new();
        for _ in 0..5 {
            rig.now_ms += FRAME_MS;
            rig.frame();
            opacities.push(text_opacity(&rig.last, "16:9", TextRoleKind::CardOption));
        }
        // Пока карточка гаснет, вариант на месте: не пропал в первый же кадр.
        assert!(
            opacities.iter().all(Option::is_some),
            "{name}: вариант пропал во время закрытия: {opacities:?}"
        );
        let opacities: Vec<f32> = opacities.into_iter().flatten().collect();
        assert!(
            opacities.windows(2).all(|pair| pair[1] <= pair[0] + 1e-4),
            "{name}: прозрачность не убывает: {opacities:?}"
        );
        assert!(
            opacities.last() < opacities.first() && opacities[0] > 0.2,
            "{name}: гаснет постепенно: {opacities:?}"
        );

        // Карточка закрылась целиком: вариантов в списке нет и они не ловят касаний.
        rig.run(900);
        assert_eq!(
            text_opacity(&rig.last, "16:9", TextRoleKind::CardOption),
            None,
            "{name}"
        );
    }
}

#[test]
fn a_card_opened_from_the_icon_while_a_menu_card_is_closing_appears_in_place_at_once() {
    let mut rig = Rig::new();
    open_card_from_menu(&mut rig, ToolId::Exposure);
    rig.act(ScreenAction::ControlsMenuCloseRequested);
    // Карточка из меню на полпути закрытия: её рамка ещё далеко от конечной.
    rig.run(64);
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);
    assert!(!rig.screen.state().card_returns_to_menu(), "теперь карточка со значка");

    // С первого же кадра стекло карточки стоит на своём месте и в своём размере (проявление на месте),
    // а не продолжает путь шторка → карточка.
    let card = rig.rect_with(|g| g.card());
    let on_target = rig.last.items.iter().any(|item| match item {
        DrawItem::Glass(group) => group.members.iter().any(|member| match member.geometry {
            ShapeGeometry::RoundedRect { size, .. } => {
                near(size.width, px(card.size.width), PX_TOLERANCE)
                    && near(size.height, px(card.size.height), PX_TOLERANCE)
                    && near(member.transform.ty, px(card.center().y), PX_TOLERANCE)
            }
            _ => false,
        }),
        _ => false,
    });
    assert!(on_target, "стекло карточки не на цели с первого кадра");
}

#[test]
fn reduce_motion_raises_the_shutter_at_once_while_full_motion_glides() {
    let raised = |rig: &Rig| px(rig.rect_with(raised_shutter).center().y);
    let base = |rig: &Rig| px(rig.shutter_center().y);

    let mut reduced = Rig::with(ScreenConfig::default(), MotionPreference::Reduced);
    reduced.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    let reduced_y = shutter_ring_y(&reduced.last);
    assert!(
        near(reduced_y, raised(&reduced), PX_TOLERANCE),
        "Reduce Motion: затвор сразу наверху: {reduced_y} против {}",
        raised(&reduced)
    );

    // Контроль: без Reduce Motion на первом кадре затвор ещё внизу (иначе проверка ничего не доказывает).
    let mut full = Rig::new();
    full.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    let full_y = shutter_ring_y(&full.last);
    assert!(
        near(full_y, base(&full), PX_TOLERANCE),
        "Full: на первом кадре внизу: {full_y}"
    );

    // Возврат тоже мгновенный.
    reduced.run(700);
    reduced.act(ScreenAction::ExposurePanelCloseRequested);
    assert!(near(shutter_ring_y(&reduced.last), base(&reduced), PX_TOLERANCE));
}

#[test]
fn reduce_motion_card_content_fades_in_place_without_sliding() {
    let title_ys = |motion| -> Vec<f32> {
        let mut rig = Rig::with(ScreenConfig::default(), motion);
        rig.screen.handle_action(ScreenAction::ControlsMenuOpenRequested);
        rig.run(700);
        rig.act(ScreenAction::ControlsItemSelected(ToolId::Timer));
        let mut ys = Vec::new();
        for _ in 0..24 {
            rig.now_ms += FRAME_MS;
            rig.frame();
            ys.extend(text_y(&rig.last, "TIMER", TextRoleKind::CardTitle));
        }
        ys
    };
    let spread = |ys: &[f32]| ys.iter().copied().fold(f32::MIN, f32::max) - ys.iter().copied().fold(f32::MAX, f32::min);

    let full = title_ys(MotionPreference::Full);
    assert!(
        full.len() > 4 && spread(&full) > 0.5,
        "контроль: при Full заголовок въезжает: {full:?}"
    );
    let reduced = title_ys(MotionPreference::Reduced);
    assert!(reduced.len() > 4, "заголовок проявляется: {reduced:?}");
    assert!(
        spread(&reduced) < 0.01,
        "Reduce Motion: заголовок не двигается: {reduced:?}"
    );
}

#[test]
fn reduce_motion_focus_frame_has_no_zoom_on_appearance() {
    let first_scale = |motion| {
        let mut rig = Rig::with(ScreenConfig::default(), motion);
        let center = rig.frame_center();
        rig.tap(center);
        rig.screen.focus_frame_scale()
    };
    assert!(
        first_scale(MotionPreference::Full) > 1.2,
        "контроль: при Full рамка сжимается с 1,7"
    );
    assert_eq!(first_scale(MotionPreference::Reduced), 1.0);
}

#[test]
fn option_labels_are_drawn_and_the_big_value_follows_the_state() {
    use crate::settings::AspectRatio;
    use crate::tools::ToolOption;
    let mut rig = Rig::new();
    open_card_from_menu(&mut rig, ToolId::Aspect);
    for label in ["4:3", "16:9", "1:1"] {
        assert_eq!(text_count_of(&rig.last, label, TextRoleKind::CardOption), 1, "{label}");
    }
    // Крупное значение — «4:3» (ролью CardValue), пока не выбрали другое.
    assert_eq!(text_count_of(&rig.last, "4:3", TextRoleKind::CardValue), 1);
    rig.act(ScreenAction::SubmenuOptionSelected(ToolOption::Aspect(
        AspectRatio::SixteenByNine,
    )));
    rig.run(100);
    // Карточка открыта, значение сменилось вместе с настройкой (строка значения не «залипла»).
    assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::Aspect));
    assert_eq!(text_count_of(&rig.last, "16:9", TextRoleKind::CardValue), 1);
    assert_eq!(text_count_of(&rig.last, "4:3", TextRoleKind::CardValue), 0);
}
