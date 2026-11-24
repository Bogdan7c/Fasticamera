//! Раздел калибровки на телефоне владельца (14k): безопасная зона (решение 226), обрезка списка рамкой окна
//! (решение 227), читаемые подписи (решение 228). Список прокручен на полстроки: у обоих краёв окна строка режется.

use camera_screen::ScreenAction;
use shell_contract::edge_insets::RawEdgeInsetsPx;
use shell_contract::orientation::DeviceOrientation;
use shell_contract::safe_area::SafeAreaInsets;
use ui_core::geometry::{Point, Rect};

use crate::calibration_goldens::{assert_calibration_zones, calibration, eye_zone_window, render_calibration};
use crate::driver::ScreenDriver;
use crate::driver::{HEIGHT_PX, SETTLE_MS, WIDTH_PX};
use crate::golden_files::check_golden;
use crate::render_rig::{RenderRig, ScopeImage, Snapshot};
use crate::{Window, with_rig};

/// Полосы телефона владельца, px.
const OWNER_INSETS: RawEdgeInsetsPx = RawEdgeInsetsPx {
    left: 0,
    top: 156,
    right: 0,
    bottom: 132,
};
/// Протяжка списка вверх глазами, pt: половина строки (68 pt) сверх порога распознавания пана.
const HALF_ROW_DRAG_PT: f64 = 44.0;
const HELD: [(DeviceOrientation, &str); 3] = [
    (DeviceOrientation::Portrait, "calibration_owner_portrait"),
    (DeviceOrientation::LandscapeLeft, "calibration_owner_landscape_left"),
    (DeviceOrientation::LandscapeRight, "calibration_owner_landscape_right"),
];

fn owner_phone(orientation: DeviceOrientation) -> ScreenDriver {
    let mut driver = calibration(orientation);
    driver.act(ScreenAction::SafeAreaChanged(
        SafeAreaInsets::try_from_insets(OWNER_INSETS).expect("зона"),
    ));
    driver.run(SETTLE_MS);
    driver
}

/// Колонка ручек, глазные координаты.
fn knobs(driver: &ScreenDriver) -> Rect {
    driver
        .screen()
        .state()
        .calibration()
        .layout()
        .expect("раскладка")
        .zones()
        .knobs
}

/// Настоящим пальцем ведёт список ручек вверх глазами на `drag_pt`.
fn scroll_list(driver: &mut ScreenDriver, drag_pt: f64) {
    let layout = *driver.screen().state().calibration().layout().expect("раскладка");
    let column = layout.zones().knobs;
    // Палец на подписи строки (левый край колонки), а не на линейке: линейку ведёт другой жест.
    let start = Point::new(column.min_x() + 40.0, column.center().y + 30.0);
    driver.drag(
        layout.eye_point_on_surface(start),
        layout.eye_point_on_surface(Point::new(start.x, start.y - drag_pt)),
    );
    driver.run(SETTLE_MS);
}

/// Сколько пикселей снимков различаются хоть чем-то внутри и вне окна `window`.
fn differing_inside_and_outside(first: &Snapshot, second: &Snapshot, window: Window) -> (usize, usize) {
    let (x0, y0, x1, y1) = window;
    let (mut inside, mut outside) = (0, 0);
    for y in 0..HEIGHT_PX {
        for x in 0..WIDTH_PX {
            if first.at(x, y) == second.at(x, y) {
                continue;
            }
            if (x0..x1).contains(&x) && (y0..y1).contains(&y) {
                inside += 1;
            } else {
                outside += 1;
            }
        }
    }
    (inside, outside)
}

fn shot(rig: &mut RenderRig, driver: &ScreenDriver) -> Snapshot {
    render_calibration(rig, driver, ScopeImage::StandIn)
}

#[test]
fn owner_phone_goldens_with_the_list_scrolled_by_half_a_row() {
    with_rig(|rig| {
        for (orientation, name) in HELD {
            let mut driver = owner_phone(orientation);
            scroll_list(&mut driver, HALF_ROW_DRAG_PT);
            let snapshot = shot(rig, &driver);
            check_golden(name, &snapshot);
            assert_calibration_zones(rig, &driver, &snapshot, name);
        }
    });
}

/// Замечание 1 владельца: прокрутка не меняет НИ ОДНОГО пикселя вне стекла колонки, сколько бы списка ни уехало.
#[test]
fn scrolling_changes_pixels_only_inside_the_column() {
    with_rig(|rig| {
        for (orientation, name) in HELD {
            let mut driver = owner_phone(orientation);
            let at_rest = shot(rig, &driver);
            for step in 1..=4 {
                scroll_list(&mut driver, HALF_ROW_DRAG_PT);
                let scrolled = shot(rig, &driver);
                let column = eye_zone_window(&driver, knobs(&driver));
                let (inside, outside) = differing_inside_and_outside(&at_rest, &scrolled, column);
                assert!(inside > 500, "{name}, шаг {step}: список едет ({inside} px)");
                assert_eq!(outside, 0, "{name}, шаг {step}: вне колонки не изменилось ничего");
            }
        }
    });
}
