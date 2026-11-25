//! Сценарий l: раскладка пальцем в портрете и обоих альбомах. Тап по кнопке света и кнопке попадает; касание, начатое
//! в полосе системного жеста, молчит; тап по кадру при открытой панели закрывает её и точку фокуса не трогает.

use camera_contract::three_a::PointOfInterest;
use camera_contract::white::WhiteOrigin;
use camera_screen::tools::ToolId;
use camera_screen::{PanelState, ScreenAction};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::system_gesture::{RawEdgeInsetsPx, SystemGestureZones};
use ui_core::geometry::Point;

use super::panel_view::strip_number;
use super::session::{CARD_SETTLE_UI_FRAMES, OPENING_KELVIN, WhiteSession};

const ORIENTATIONS: [(&str, DeviceOrientation); 3] = [
    ("портрет", DeviceOrientation::Portrait),
    ("альбом влево", DeviceOrientation::LandscapeLeft),
    ("альбом вправо", DeviceOrientation::LandscapeRight),
];
/// Размер поверхности стенда в пикселях и pt → px.
const SURFACE_WIDTH_PX: f64 = 1080.0;
const SURFACE_HEIGHT_PX: f64 = 2412.0;
const PX_PER_PT: f64 = SURFACE_WIDTH_PX / 440.0;
/// На сколько пикселей полоса системного жеста заходит за центр кнопки.
const ZONE_OVERLAP_PX: f64 = 40.0;

fn turn_to(session: &mut WhiteSession, orientation: DeviceOrientation) {
    session
        .stand
        .send_action(ScreenAction::DeviceOrientationChanged(orientation));
    session.stand.ui_frames(CARD_SETTLE_UI_FRAMES);
}

/// Полоса системного жеста у БЛИЖАЙШЕГО к точке края поверхности, заходящая за неё на `ZONE_OVERLAP_PX`.
fn zones_covering(point_pt: Point) -> SystemGestureZones {
    let (x, y) = (point_pt.x * PX_PER_PT, point_pt.y * PX_PER_PT);
    let distances = [
        (x, 0_usize),
        (SURFACE_HEIGHT_PX.min(y), 1),
        (SURFACE_WIDTH_PX - x, 2),
        (SURFACE_HEIGHT_PX - y, 3),
    ];
    let (nearest, edge) = distances
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .expect("четыре края");
    let thickness = (nearest + ZONE_OVERLAP_PX).ceil() as i32;
    let mut insets = RawEdgeInsetsPx {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    match edge {
        0 => insets.left = thickness,
        1 => insets.top = thickness,
        2 => insets.right = thickness,
        _ => insets.bottom = thickness,
    }
    SystemGestureZones::try_from_insets(insets).expect("полоса годится")
}

/// Палец попадает в кнопка и в кнопку света, тап по кадру закрывает панель и не наводит фокус — в каждом положении.
#[test]
fn the_button_and_the_light_button_are_hit_by_a_finger_in_portrait_and_both_landscapes() {
    for (name, orientation) in ORIENTATIONS {
        let Some(mut session) = WhiteSession::open() else {
            return;
        };
        turn_to(&mut session, orientation);
        // Поворот действительно применён: в альбоме число на кнопке повёрнуто на 90° (кнопка остаётся на месте, как
        // и положено верхнему ряду: поворачиваются только знаки).
        let number = strip_number(session.stand.ui_list(), OPENING_KELVIN).expect("число на кнопке");
        let rotated = number.transform.b.abs() > 0.5;
        assert_eq!(
            rotated,
            orientation != DeviceOrientation::Portrait,
            "{name}: поворот числа на кнопке"
        );
        session.open_card();
        assert_eq!(
            session.panel(),
            PanelState::Submenu(ToolId::White),
            "{name}: тап по кнопке открыл панель"
        );

        session.tap_light(0);
        let held = session.wait_origin("рука поставила Sun", WhiteOrigin::Manual);
        assert_eq!(held.setting, session.light_setting(0), "{name}: тап попал в кнопку Sun");
        session.assert_picture_is(held.setting, name);

        let focus_before = session.stand.status().three_a.point;
        let intents_before = session.stand.effect_counters().camera.sent;
        session.tap_frame();
        assert_eq!(
            session.panel(),
            PanelState::Closed,
            "{name}: тап по кадру закрыл панель"
        );
        assert_eq!(
            session.stand.status().three_a.point,
            focus_before,
            "{name}: точку фокуса не трогал"
        );
        assert_eq!(focus_before, PointOfInterest::WholeFrame);
        assert_eq!(
            session.stand.effect_counters().camera.sent,
            intents_before,
            "{name}: камере не ушло ни одного намерения"
        );
        session.stand.finish();
    }
}

/// Касание, начатое в полосе системного жеста, принадлежит системе: кнопка молчит (тап и долгое нажатие).
#[test]
fn a_touch_that_begins_in_the_system_gesture_band_over_the_button_is_silent_in_every_orientation() {
    for (name, orientation) in ORIENTATIONS {
        let Some(mut session) = WhiteSession::open() else {
            return;
        };
        turn_to(&mut session, orientation);
        let icon = session.white_button();
        let requests_before = session.source.requests().len();
        let intents_before = session.stand.effect_counters().camera.sent;

        session.stand.set_system_gesture_zones(zones_covering(icon));
        session.stand.tap(icon);
        session.stand.ui_frames(CARD_SETTLE_UI_FRAMES);
        assert_eq!(
            session.panel(),
            PanelState::Closed,
            "{name}: тап в полосе панель не открыл"
        );
        session.stand.hold(icon, 800);
        session.stand.ui_frames(10);
        assert_eq!(
            session.source.requests().len(),
            requests_before,
            "{name}: долгое нажатие белое не взяло"
        );
        assert_eq!(
            session.stand.effect_counters().camera.sent,
            intents_before,
            "{name}: камере ничего"
        );
        assert_eq!(session.stand.haptics_played(), vec![], "{name}: вибрации нет");

        // Контроль: без полосы то же касание работает (иначе молчание ничего не доказывает).
        session.stand.set_system_gesture_zones(SystemGestureZones::NONE);
        session.stand.tap(icon);
        session.stand.ui_frames(CARD_SETTLE_UI_FRAMES);
        assert_eq!(
            session.panel(),
            PanelState::Submenu(ToolId::White),
            "{name}: без полосы тап открывает панель"
        );
        session.stand.finish();
    }
}
