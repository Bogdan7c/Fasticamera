//! Общий стенд панели белого (план white-balance, сессия 02, пакет D0): палец → экран → движок → рендер → пиксели.
//!
//! Стенд — `white_panel_end_to_end/stand.rs` (`PanelStand`): настоящий движок на fake-бэкенде, настоящий бэкенд показа
//! (RADV, validation), настоящий экран камеры с исполнителем эффектов и портом на ручку ТОГО ЖЕ движка, палец через
//! канал входа экрана. Этот файл доказывает, что шов работает на том, что есть сегодня (до панели белого):
//! 1. тап пальцем по кадру → движок получает точку фокуса;
//! 2. тап по значку EV и протяжка шкалы → движок получает новый EV;
//! 3. белая, посланная прямо в ручку движка, пока экран живёт и гоняет кадры UI, даёт кадр побитово как эталон;
//! 4. ловушка: без порта к движку (1) и (2) НЕ проходят — тесты не зелёные «сами по себе».
//!
//! Стенды движка и рендера подключены по пути (второй копии не заводим), как в `white_end_to_end.rs`.

#![cfg(not(target_os = "android"))]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть сигнал о провале"
)]
#![allow(
    dead_code,
    reason = "стенды render-loop и PanelStand общие: каждый тестовый файл берёт свою часть"
)]

#[path = "../../render-loop/tests/awb_support/mod.rs"]
mod awb_support;
#[path = "../../render-loop/tests/support/mod.rs"]
mod support;
#[path = "../../render-loop/tests/vulkan_support/mod.rs"]
mod vulkan_support;
#[path = "../../render-loop/tests/white_engine/mod.rs"]
mod white_engine;
#[path = "../../render-loop/tests/white_rig/mod.rs"]
mod white_rig;

#[path = "white_panel_end_to_end/button_side.rs"]
mod button_side;
#[path = "white_end_to_end/stand.rs"]
mod engine_stand;
#[path = "white_end_to_end/fake_source.rs"]
mod fake_source;
#[path = "white_panel_end_to_end/lagging_facts.rs"]
mod lagging_facts;
#[path = "white_panel_end_to_end/layout.rs"]
mod layout;
#[path = "white_panel_end_to_end/lights_and_phone.rs"]
mod lights_and_phone;
#[path = "white_panel_end_to_end/long_press.rs"]
mod long_press;
#[path = "white_panel_end_to_end/look_over_white.rs"]
mod look_over_white;
#[path = "white_panel_end_to_end/memory_and_reading.rs"]
mod memory_and_reading;
#[path = "white_panel_end_to_end/neutrality.rs"]
mod neutrality;
#[path = "white_panel_end_to_end/panel_view.rs"]
mod panel_view;
#[path = "white_panel_end_to_end/refusals.rs"]
mod refusals;
#[path = "white_panel_end_to_end/scales.rs"]
mod scales;
#[path = "white_panel_end_to_end/screen_parts.rs"]
mod screen_parts;
#[path = "white_panel_end_to_end/session.rs"]
mod session;
#[path = "white_panel_end_to_end/stand.rs"]
mod stand;
#[path = "white_panel_end_to_end/touch_targets.rs"]
mod touch_targets;
#[path = "white_panel_end_to_end/waiting.rs"]
mod waiting;

use camera_contract::control::WhiteBalanceSetting;
use camera_contract::frame_passport::CaptureColorHints;
use camera_contract::three_a::PointOfInterest;
use camera_contract::white::{WhiteOrigin, WhiteSourceId, WhiteTransition};
use camera_screen::PanelState;
use engine_stand::{LIGHT_CHANGES_AT, StandOptions};
use fake_source::{FakeSourceControl, Reaction, registry_with};
use fasticamera::white_memory::WhiteMemoryHandoff;
use render_loop::config::WhiteBalanceConfig;
use stand::{CameraPort, PanelStand};
use white_engine::{held_setting, setting_of};
use white_rig::{FrameImage, rig_or_skip, scene_with_light_change};

const PHONE: WhiteSourceId = WhiteSourceId::PHONE;
/// Сколько кадров UI гоним, пока панель карточки доезжает (пружины экрана успокаиваются за ≈ 0,7 с).
const PANEL_SETTLE_FRAMES: usize = 45;

/// Стенд с молчащим «телефоном» (белая не вмешивается в тесты тапа и EV).
fn panel_stand(port: CameraPort) -> Option<PanelStand> {
    let source = FakeSourceControl::new(Reaction::Silent);
    let mut stand = PanelStand::start(
        StandOptions {
            sources: registry_with(PHONE, &source),
            memory: WhiteMemoryHandoff::default(),
        },
        port,
    )?;
    stand.resume();
    Some(stand)
}

fn no_hints(_frame: i64) -> CaptureColorHints {
    CaptureColorHints::default()
}

/// Эталонный прогон: белая названа с первого кадра, заявок нет (как в `white_end_to_end.rs`).
fn reference_images(setting: WhiteBalanceSetting, frames: i64) -> Vec<FrameImage> {
    let (mut rig, journal) = rig_or_skip(
        scene_with_light_change(200, LIGHT_CHANGES_AT),
        held_setting(setting),
        CaptureColorHints::default(),
        WhiteBalanceConfig::default(),
    )
    .expect("GPU есть (стенд уже поднимался)");
    let images = rig.drive(0..frames, &no_hints, &mut |_| {});
    rig.finish(&journal);
    images
}

/// Тап по центру кадра; возвращает статус движка после того, как экран успел отдать ему намерение.
fn tap_viewfinder(stand: &mut PanelStand) {
    let target = touch_targets::viewfinder_center(stand.screen());
    stand.tap(target);
    // Двойной тап и долгое нажатие экран различает по времени: даём ему догнать «окно ожидания».
    stand.ui_frames(30);
}

/// Тап по значку EV, затем протяжка шкалы влево на 60 pt за 320 мс.
fn open_ev_card_and_drag_ruler(stand: &mut PanelStand) {
    let icon = touch_targets::ev_strip_icon(stand.screen());
    stand.tap(icon);
    stand.ui_frames(PANEL_SETTLE_FRAMES);
    if stand.screen().panel() != PanelState::ExposurePanel {
        return;
    }
    let ruler = touch_targets::exposure_card_ruler(stand.screen());
    stand.drag(ruler, ui_core::geometry::Point::new(ruler.x - 60.0, ruler.y), 320);
    stand.ui_frames(PANEL_SETTLE_FRAMES);
}

#[test]
fn a_finger_tap_on_the_frame_gives_the_engine_a_point_of_focus() {
    let Some(mut stand) = panel_stand(CameraPort::LiveEngine) else {
        return;
    };
    assert_eq!(
        stand.status().three_a.point,
        PointOfInterest::WholeFrame,
        "до тапа точки нет"
    );
    tap_viewfinder(&mut stand);
    let status = stand.wait_status("движок получил точку фокуса", |status| {
        matches!(status.three_a.point, PointOfInterest::Point { .. })
    });
    assert!(matches!(status.three_a.point, PointOfInterest::Point { .. }));
    assert!(stand.effect_counters().camera.sent >= 1, "экран отправил намерение");
    assert_eq!(stand.effect_counters().camera.no_engine, 0);
    stand.finish();
}

#[test]
fn the_ev_card_ruler_dragged_by_a_finger_moves_the_engine_exposure_bias() {
    let Some(mut stand) = panel_stand(CameraPort::LiveEngine) else {
        return;
    };
    assert_eq!(
        stand.status().three_a.controls.exposure_bias.ev(),
        0.0,
        "до протяжки EV нулевой"
    );
    open_ev_card_and_drag_ruler(&mut stand);
    assert_eq!(
        stand.screen().panel(),
        PanelState::ExposurePanel,
        "карточка EV открылась тапом по значку"
    );
    let shown_steps = stand.screen().exposure().steps();
    assert!(shown_steps > 0, "протяжка влево подняла EV на экране: {shown_steps}");
    let status = stand.wait_status("движок получил новый EV", |status| {
        status.three_a.controls.exposure_bias.ev() > 0.0
    });
    let engine_ev = status.three_a.controls.exposure_bias.ev();
    // Движок получил ровно то значение, которое показывает экран (шаги по 1/3 EV).
    let screen_ev = stand
        .screen()
        .exposure()
        .bias(&camera_screen::ScreenConfig::default().exposure)
        .ev();
    assert!(
        (engine_ev - screen_ev).abs() < 1e-3,
        "движок {engine_ev}, экран {screen_ev}"
    );
    stand.finish();
}

#[test]
fn a_white_sent_straight_to_the_engine_shows_exactly_the_reference_frame_while_the_screen_keeps_drawing() {
    let Some(mut stand) = panel_stand(CameraPort::LiveEngine) else {
        return;
    };
    // Источник молчит: заявка ждёт, кадры скрыты. Экран при этом живёт и рисует.
    let mut images = stand.alternate(3, &no_hints);
    assert!(images.iter().all(Option::is_none), "до белой кадры скрыты");

    let manual = setting_of(3300.0);
    stand
        .handle()
        .set_white(manual, WhiteTransition::Immediate)
        .expect("команда принята");
    stand.wait_status("рука поставила белую", |status| {
        status.three_a.white.held.origin == WhiteOrigin::Manual
    });
    images.extend(stand.alternate(10, &no_hints));

    let reference = reference_images(manual, 13);
    let first = images.iter().position(Option::is_some).expect("картинка показана");
    for index in first..13 {
        assert_eq!(images[index], reference[index], "кадр {index} побитово как у эталона");
    }
    assert!(!stand.ui_list().items.is_empty(), "экран всё это время рисовал кадры");
    stand.finish();
}

#[test]
fn trap_without_the_engine_port_the_tap_and_the_ev_drag_reach_nobody() {
    let Some(mut stand) = panel_stand(CameraPort::Absent) else {
        return;
    };
    tap_viewfinder(&mut stand);
    open_ev_card_and_drag_ruler(&mut stand);
    let counters = stand.effect_counters().camera;
    assert!(
        counters.no_engine >= 1,
        "экран честно считает «движка нет»: {counters:?}"
    );
    assert_eq!(counters.sent, 0);
    let status = stand.status();
    assert_eq!(
        status.three_a.point,
        PointOfInterest::WholeFrame,
        "точка фокуса не дошла"
    );
    assert_eq!(status.three_a.controls.exposure_bias.ev(), 0.0, "EV не дошёл");
    stand.finish();
}
