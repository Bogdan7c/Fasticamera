//! Сценарий b: кнопка белого у затвора с обеих сторон (`screen.white.button_side`) настоящим пальцем в портрете и в
//! обоих альбомах. Тап открывает карточку, поднятая вместе с затвором кнопка остаётся рабочей и закрывает её, долгое
//! нажатие берёт белое телефона у настоящего движка; зона касания кнопки целиком выше нижней полосы системного
//! жеста и в покое, и поднятой, а касание ВНУТРИ полосы молчит.

use camera_contract::white::WhiteOrigin;
use camera_screen::{PanelState, ScreenAction, WhiteButtonSide};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::system_gesture::{RawEdgeInsetsPx, SystemGestureZones};
use ui_core::geometry::Point;

use super::session::{CARD_SETTLE_UI_FRAMES, PHONE, WhiteSession, lamp_after};
use super::white_engine::setting_of;

const ORIENTATIONS: [(&str, DeviceOrientation); 3] = [
    ("портрет", DeviceOrientation::Portrait),
    ("альбом влево", DeviceOrientation::LandscapeLeft),
    ("альбом вправо", DeviceOrientation::LandscapeRight),
];
const SURFACE_HEIGHT_PX: f64 = 2412.0;
const PX_PER_PT: f64 = 1080.0 / 440.0;
/// Нижняя полоса системного жеста телефона владельца, px (53,8 pt): шкала и кнопка обязаны лежать выше.
const BOTTOM_BAND_PX: i32 = 132;
/// Подъём ряда затвора над белой карточкой, pt (WB28).
const RAISE_OVER_WHITE_CARD_PT: f64 = 124.0;
const SIDES: [(&str, WhiteButtonSide); 2] = [("справа", WhiteButtonSide::Right), ("слева", WhiteButtonSide::Left)];

fn bottom_band() -> SystemGestureZones {
    SystemGestureZones::try_from_insets(RawEdgeInsetsPx {
        left: 0,
        top: 0,
        right: 0,
        bottom: BOTTOM_BAND_PX,
    })
    .expect("полоса годится")
}

/// Зона кнопки целиком выше нижней полосы (в портретных осях поверхности).
fn assert_zone_above_band(session: &WhiteSession, label: &str) {
    let zone = session.stand.white_targets().button;
    let bottom_px = zone.max_y() * PX_PER_PT;
    assert!(
        bottom_px + f64::from(BOTTOM_BAND_PX) <= SURFACE_HEIGHT_PX,
        "{label}: низ зоны кнопки {bottom_px} px заходит в полосу жеста {BOTTOM_BAND_PX} px"
    );
}

#[test]
fn the_button_works_on_both_sides_in_every_orientation_with_the_real_bottom_gesture_band() {
    for (side_name, side) in SIDES {
        for (orientation_name, orientation) in ORIENTATIONS {
            let name = format!("{side_name}, {orientation_name}");
            let Some(mut session) = WhiteSession::open_on_side(side) else {
                return;
            };
            session
                .stand
                .send_action(ScreenAction::DeviceOrientationChanged(orientation));
            session.stand.ui_frames(CARD_SETTLE_UI_FRAMES);
            session.stand.set_system_gesture_zones(bottom_band());

            // Раскладка: сторона, высота затвора и зона выше полосы жеста — в покое.
            let rest = session.stand.white_targets().button;
            assert_zone_above_band(&session, &format!("{name}, покой"));
            let shutter_x_pt = 220.0;
            match side {
                WhiteButtonSide::Right => assert!(rest.center().x > shutter_x_pt + 60.0, "{name}: {rest:?}"),
                WhiteButtonSide::Left => assert!(rest.center().x < shutter_x_pt - 60.0, "{name}: {rest:?}"),
            }

            // Короткий тап открывает карточку; белое не берётся.
            let requests_before = session.source.requests().len();
            session.tap_button();
            assert_eq!(
                session.panel(),
                PanelState::Submenu(camera_screen::tools::ToolId::White),
                "{name}"
            );
            assert_eq!(
                session.source.requests().len(),
                requests_before,
                "{name}: тап белое не берёт"
            );

            // Кнопка поехала вместе с затвором на 124 pt и осталась выше полосы жеста.
            let raised = session.stand.white_targets().button;
            assert!(
                ((rest.min_y() - raised.min_y()) - RAISE_OVER_WHITE_CARD_PT).abs() < 0.5,
                "{name}: подъём {rest:?} -> {raised:?}"
            );
            assert_zone_above_band(&session, &format!("{name}, поднята"));

            // Тап по поднятой кнопке закрывает карточку.
            session.tap_button();
            assert_eq!(
                session.panel(),
                PanelState::Closed,
                "{name}: поднятая кнопка закрыла карточку"
            );

            // Долгое нажатие берёт белое телефона: настоящий движок держит ответ источника.
            session.source.react_with(lamp_after(2, 4_500.0));
            session.long_press_button();
            session.frames(6);
            let held = session.wait_origin("белое взято долгим нажатием", WhiteOrigin::Source(PHONE));
            assert_eq!(held.setting, setting_of(4_500.0), "{name}");
            assert_eq!(session.panel(), PanelState::Closed, "{name}: панель не открылась");
            session.stand.finish();
        }
    }
}

#[test]
fn a_touch_that_begins_inside_the_bottom_band_below_the_button_is_silent() {
    let Some(mut session) = WhiteSession::open() else {
        return;
    };
    session.stand.set_system_gesture_zones(bottom_band());
    let zone = session.stand.white_targets().button;
    // Точка под кнопкой, уже внутри полосы жеста (ниже её верхнего края).
    let band_top_pt = (SURFACE_HEIGHT_PX - f64::from(BOTTOM_BAND_PX)) / PX_PER_PT;
    let inside_band = Point::new(zone.center().x, band_top_pt + 10.0);
    session.stand.tap(inside_band);
    session.stand.ui_frames(CARD_SETTLE_UI_FRAMES);
    assert_eq!(session.panel(), PanelState::Closed);
    // Контроль: сама кнопка при той же полосе работает.
    session.tap_button();
    assert_eq!(
        session.panel(),
        PanelState::Submenu(camera_screen::tools::ToolId::White)
    );
    session.stand.finish();
}
