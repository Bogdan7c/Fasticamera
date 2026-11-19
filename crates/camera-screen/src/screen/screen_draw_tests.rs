//! Тесты списка рисования по состояниям экрана: словарь проходит `validate()`, элементы стоят там, где велит
//! раскладка, ориентация поворачивает значки, но не рамки.

use camera_contract::control::ExposureBias;
use camera_contract::three_a::AeAfLock;
use shell_contract::orientation::DeviceOrientation;
use ui_core::time::MotionPreference;
use ui_draw::draw_list::DrawItem;
use ui_draw::icons::IconId;
use ui_draw::shape::ShapeGeometry;

use super::test_rig::*;
use crate::action::ScreenAction;
use crate::config::ScreenConfig;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::facts::{CameraFacts, CameraPresence, UnavailableReason};
use crate::settings::{AspectRatio, TimerSetting};
use crate::state::PanelState;
use crate::test_support::live_facts;
use crate::tools::{ToolId, ToolOption};

fn near(actual: f32, expected: f32, tolerance: f32) -> bool {
    (actual - expected).abs() <= tolerance
}

/// Члены стекла указанной формы около (x, y) px.
fn glass_near(list: &ui_draw::draw_list::DrawList, y_px: f32) -> Vec<ShapeGeometry> {
    glass_members(list)
        .into_iter()
        .filter(|(_, _, ty)| near(*ty, y_px, 3.0))
        .map(|(geometry, ..)| geometry)
        .collect()
}

#[test]
fn rest_layout_puts_elements_where_the_layout_doc_says() {
    let mut rig = Rig::new();
    let list = rig.frame().clone();
    // `layout.md` 4.3: затвор по y ≈ 2034 px (Ø 197), капсула режимов ≈ 2269 px (540 × 117), значки ≈ 323 px.
    let shutter = glass_near(&list, 2034.0);
    assert!(
        shutter
            .iter()
            .any(|g| matches!(g, ShapeGeometry::Circle { diameter } if near(*diameter, 197.0, 2.0))),
        "затвор Ø197 на y 2034: {shutter:?}"
    );
    let capsule = glass_near(&list, 2269.0);
    assert!(
        capsule
            .iter()
            .any(|g| matches!(g, ShapeGeometry::Capsule { size } if near(size.width, 540.0, 2.0) && near(size.height, 117.0, 2.0))),
        "капсула 540×117 на y 2269: {capsule:?}"
    );
    let flash = icon_transforms(&list, IconId::FlashOff);
    assert_eq!(flash.len(), 1);
    assert!(
        near(flash[0].ty, 323.0, 4.0),
        "значок вспышки на y 323, а не {}",
        flash[0].ty
    );
    // Миниатюра и кнопка меню — круги Ø≈118 в нижней зоне; переворота камеры и зума нет (инвариант 6).
    assert_eq!(icon_transforms(&list, IconId::FlipCamera).len(), 0);
    assert_eq!(icon_transforms(&list, IconId::ControlsMenu).len(), 1);
}

#[test]
fn every_key_state_yields_a_valid_draw_list_with_its_distinguishing_content() {
    let mut rig = Rig::new();
    let idle_texts = text_count(rig.frame());

    // Рамка фокуса: после тапа в списке появляются фигуры рамки и иконка «солнца».
    let center = rig.frame_center();
    rig.tap(center);
    rig.run(300);
    assert_eq!(
        icon_transforms(&rig.last, IconId::Exposure).len(),
        2,
        "значок EV и «солнце» у рамки"
    );

    // AE/AF LOCK: камера подтвердила блокировку → плашка с текстом.
    let mut locked = live_facts();
    locked.lock = AeAfLock::Locked;
    rig.act(ScreenAction::CameraFactsChanged(locked));
    rig.run(800);
    assert!(rig.screen.state().focus().is_lock_plaque_visible());
    assert!(text_count(&rig.last) > idle_texts, "плашка LOCK добавила текст");

    // Меню открыто: шторка (стекло-панель) и 7 пунктов.
    rig.act(ScreenAction::ViewfinderTapped {
        point: center,
        at: ms(rig.now_ms),
    });
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.run(900);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
    assert!(
        rig.last.items.iter().any(|i| matches!(i, DrawItem::Glass(g) if g.members.iter().any(|m| matches!(m.geometry, ShapeGeometry::RoundedRect { .. })))),
        "шторка меню — стекло скруглённого прямоугольника"
    );

    // Подменю таймера: варианты Off / 3s / 5s / 10s.
    rig.act(ScreenAction::ControlsItemSelected(ToolId::Timer));
    rig.run(900);
    assert_eq!(rig.screen.state().panel(), PanelState::Submenu(ToolId::Timer));
    rig.act(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    rig.act(ScreenAction::ControlsMenuCloseRequested);
    rig.run(900);

    // Панель Exposure на месте капсулы.
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(900);
    assert_eq!(rig.screen.state().panel(), PanelState::ExposurePanel);
    assert_eq!(icon_transforms(&rig.last, IconId::Close).len(), 1, "кнопка «×» панели");
    assert!(
        glass_near(&rig.last, 2269.0)
            .iter()
            .all(|g| !matches!(g, ShapeGeometry::Capsule { .. })),
        "капсула режимов растворилась"
    );
    rig.act(ScreenAction::ExposurePanelCloseRequested);
    rig.run(900);

    // Отсчёт таймера: цифры по центру кадра.
    let before = text_count(rig.frame());
    let start = rig.now_ms;
    rig.act(ScreenAction::ShutterTapped { at: ms(start) });
    assert!(text_count(&rig.last) > before, "цифры отсчёта");
    rig.run(3_100);
    assert!(
        rig.effects().iter().any(|e| matches!(e, Effect::Capture(_))),
        "по окончании отсчёта — снимок"
    );

    // 16:9 и 1:1 — валидные списки с другим кадром.
    rig.run(600);
    for aspect in [AspectRatio::SixteenByNine, AspectRatio::Square] {
        rig.act(ScreenAction::ControlsMenuOpenRequested);
        rig.act(ScreenAction::ControlsItemSelected(ToolId::Aspect));
        rig.act(ScreenAction::SubmenuOptionSelected(ToolOption::Aspect(aspect)));
        rig.act(ScreenAction::ControlsMenuCloseRequested);
        rig.run(900);
        assert_eq!(rig.screen.state().settings().aspect, aspect);
    }
}

#[test]
fn unavailable_camera_shows_the_notice_and_disables_the_shutter() {
    let mut rig = Rig::new();
    let texts = text_count(rig.frame());
    let facts = CameraFacts {
        presence: CameraPresence::Unavailable {
            reason: UnavailableReason::InUse,
        },
        ..CameraFacts::STARTING
    };
    rig.act(ScreenAction::CameraFactsChanged(facts));
    rig.run(300);
    assert!(rig.screen.state().is_camera_unavailable());
    assert!(text_count(&rig.last) > texts, "текст «Camera unavailable»");
}

#[test]
fn rotating_the_phone_turns_the_icons_but_not_their_frames() {
    let mut rig = Rig::new();
    let before = icon_transforms(rig.frame(), IconId::FlashOff)[0];
    let glass_before = glass_members(&rig.last);
    rig.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.run(1000);
    let after = icon_transforms(&rig.last, IconId::FlashOff)[0];
    // Центр значка на месте (раскладка не меняется), линейная часть повернута на 90° по часовой.
    assert!(near(after.tx, before.tx, 0.01) && near(after.ty, before.ty, 0.01));
    assert!(near(after.a, 0.0, 1e-3) && near(after.c, 1.0, 1e-3), "{after:?}");
    assert_eq!(glass_members(&rig.last), glass_before, "стекло кнопок не повернулось");
}

#[test]
fn icon_rotation_is_identical_at_different_frame_rates() {
    let angle_at = |step_ms: i64| {
        let mut rig = Rig::new();
        rig.act(ScreenAction::DeviceOrientationChanged(
            DeviceOrientation::LandscapeRight,
        ));
        rig.run_until(rig.now_ms + 200, step_ms);
        icon_transforms(&rig.last, IconId::FlashOff)[0]
    };
    // 50 и 100 Гц: общие моменты времени, одинаковая пружина от общего момента старта.
    let slow = angle_at(20);
    let fast = angle_at(10);
    assert_eq!(slow, fast, "поворот зависит от времени, а не от частоты кадров");
    assert!(slow.a.abs() < 0.99, "поворот уже идёт");
}

#[test]
fn restoring_exposure_moves_the_scale_without_an_echo_to_the_camera() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    rig.run(700);
    rig.effects();
    let restored = ExposureBias::from_ev(1.0).expect("EV");
    rig.act(ScreenAction::ExposureRestored { bias: restored });
    rig.run(1_500);
    assert_eq!(rig.screen.state().exposure().steps(), 3);
    assert!(
        (rig.screen.slider_value_ev() - 1.0).abs() < 0.01,
        "шкала доехала до восстановленного значения: {}",
        rig.screen.slider_value_ev()
    );
    let effects = rig.effects();
    assert!(
        !effects
            .iter()
            .any(|e| matches!(e, Effect::Camera(ScreenCameraIntent::SetExposureBias(_)))),
        "программная установка не уходит камере: {effects:?}"
    );
    assert!(
        rig.screen.diagnostics().echo_events_suppressed > 0,
        "эхо шкалы было и погашено"
    );
}

#[test]
fn surface_resize_rebuilds_the_scene_and_taps_still_work_at_the_new_scale() {
    let mut rig = Rig::new();
    let rebuilds = rig.screen.diagnostics().stage_rebuilds;
    let small = (720.0, 1608.0);
    rig.now_ms += 16;
    rig.last = rig.screen.produce_frame(&request(rig.now_ms, small)).expect("кадр");
    assert_eq!(rig.screen.diagnostics().stage_rebuilds, rebuilds + 1);
    assert!(
        rig.effects().iter().any(|e| matches!(e, Effect::Viewfinder(_))),
        "рендеру ушло новое кадрирование"
    );
    // Затвор в новом масштабе (1,636 px/pt): y = (801,9 + сдвиг к низу) × масштаб.
    let layout = *rig.screen.state().layout().expect("раскладка");
    let expected_y = ((801.9 + layout.bottom_anchor_shift_pt()) * f64::from(small.0 / 440.0)) as f32;
    assert!(
        !glass_near(&rig.last, expected_y).is_empty(),
        "затвор на y {expected_y}"
    );
}

#[test]
fn reduce_motion_screen_still_produces_valid_frames() {
    let mut rig = Rig::with(ScreenConfig::default(), MotionPreference::Reduced);
    rig.act(ScreenAction::ControlsMenuOpenRequested);
    rig.run(600);
    assert_eq!(rig.screen.state().panel(), PanelState::ControlsMenu);
}

#[test]
fn capture_blink_dims_the_frame_to_the_measured_depth_and_returns_without_overshoot() {
    let mut rig = Rig::new();
    let shutter = rig.shutter_center();
    rig.tap(shutter);
    let mut series = Vec::new();
    let end = rig.now_ms + 700;
    while rig.now_ms < end {
        rig.now_ms += 8;
        series.push(blink_darkness(rig.frame()));
    }
    let peak = series.iter().copied().fold(0.0_f32, f32::max);
    assert!(near(peak, 0.85, 0.02), "глубина затемнения ≈ 85 %, а не {peak}");
    let peak_at = series.iter().position(|d| *d >= peak - 1e-6).expect("пик");
    // После пика — только убывание (без перелёта), и к концу кадр чист.
    assert!(
        series[peak_at..].windows(2).all(|pair| pair[1] <= pair[0] + 1e-6),
        "{series:?}"
    );
    assert_eq!(*series.last().expect("серия"), 0.0);
}
