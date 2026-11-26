//! Golden-кадры вида колёс раздела калибровки (сессия 14b2, пакет P4): портрет и альбом.
//!
//! Вид доводится настоящими действиями экрана (`ControlViewSelected(Wheels)`, `WheelSelected`), колёса приходят
//! фактами о виде: Lift тронут ТОЛЬКО толчком, Gamma — ТОЛЬКО кольцом яркости, остальные нейтральны. Поэтому видно
//! и яркие «↻» выбранного колеса (толчок отличается от пресета), и метку-кольцо на вкладке Gamma (колесо изменено,
//! но не выбрано). Ободок — настоящие цвета OKLab из каталога приложения, а не однотонная заглушка.
//!
//! Кроме сверки с эталоном каждый тест независимо проверяет пиксели ОБОИХ положений телефона: цвета ободка в
//! четырёх «глазных» направлениях (так ловится перевёрнутое кольцо под повёрнутым корнем) и положение шайбы.

use camera_screen::calibration::ControlViewKind;
use camera_screen::{CameraScreen, ScreenAction};
use look_contract::{ColorWheel, ColorWheels, EditState, LookKnob, WheelPush, WheelZone};
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::Point;

use crate::calibration_goldens::{as_eye, assert_calibration_zones, look_facts_with_wheels, render_calibration};
use crate::driver::{PX_PER_PT, SETTLE_MS, ScreenDriver};
use crate::golden_files::check_golden;
use crate::render_rig::{RenderRig, ScopeImage, Snapshot};
use crate::{Window, with_rig};

/// Точка толчка Lift в голдене: вправо и вверх «глазами», сила ≈ 0,64 — шайба заметно в стороне от центра.
const LIFT_PUSH: (f32, f32) = (0.5, 0.4);
/// Кольцо яркости Gamma в голдене: значение заметно отличается от нуля.
const GAMMA_RING: f32 = 0.5;
/// Половина стороны окна замера цвета ободка, px: усредняем маленький квадрат, а не один пиксель.
const PROBE_HALF_PX: i64 = 2;
/// Половина стороны окна шайбы, pt: шайба меньше окна, но окно меньше расстояния до центра.
const PUCK_WINDOW_HALF_PT: f64 = 8.0;

/// Колёса голдена: Lift — только толчок, Gamma — только кольцо.
pub(crate) fn golden_wheels() -> ColorWheels {
    let lift = ColorWheel {
        push: WheelPush::new(LIFT_PUSH.0, LIFT_PUSH.1).expect("точка в круге"),
        ring: LookKnob::NEUTRAL,
    };
    let gamma = ColorWheel {
        push: WheelPush::NEUTRAL,
        ring: LookKnob::new(GAMMA_RING).expect("значение допустимо"),
    };
    ColorWheels::NEUTRAL
        .with_wheel(WheelZone::Lift, lift)
        .with_wheel(WheelZone::Gamma, gamma)
}

/// Раздел калибровки в виде колёс: выбран Lift, колёса — `wheels`.
pub(crate) fn wheels_view(orientation: DeviceOrientation, wheels: ColorWheels) -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::LookFactsChanged(Box::new(look_facts_with_wheels(
        as_eye(),
        EditState::EditedOnTop,
        None,
        wheels,
    ))));
    driver.act(ScreenAction::DeviceOrientationChanged(orientation));
    driver.act(ScreenAction::ModeSelected(camera_screen::modes::CaptureMode::Calibrate));
    driver.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    driver.act(ScreenAction::WheelSelected(WheelZone::Lift));
    driver.run(SETTLE_MS);
    driver
}

/// Где круг колеса: центр и радиусы (pt поверхности) плюс «глазной» центр для расчёта направлений.
struct CircleProbe {
    /// Центр круга в глазных координатах (x вправо, y вниз, как в раскладке раздела).
    eye_center: Point,
    /// Радиус области толчка, pt (до внутреннего края ободка).
    pad_radius_pt: f64,
    /// Радиус середины ободка, pt.
    ring_mid_radius_pt: f64,
}

impl CircleProbe {
    fn of(screen: &CameraScreen) -> Self {
        let placement = screen.wheel_pad_placement().expect("вид колёс открыт, круг помещается");
        let layout = screen.state().calibration().layout().expect("раскладка раздела");
        let surface_center = placement.circle.center();
        let outer_radius = placement.circle.size.width / 2.0;
        Self {
            eye_center: layout.surface_point_in_eye(surface_center),
            pad_radius_pt: placement.pad_radius_pt,
            ring_mid_radius_pt: (outer_radius + placement.pad_radius_pt) / 2.0,
        }
    }

    /// Пиксель поверхности точки круга, смещённой от центра на `(right_pt, up_pt)` «ГЛАЗАМИ» (вправо, вверх).
    fn pixel_at_eye_offset(&self, screen: &CameraScreen, right_pt: f64, up_pt: f64) -> (i64, i64) {
        let layout = screen.state().calibration().layout().expect("раскладка раздела");
        let eye_point = Point::new(self.eye_center.x + right_pt, self.eye_center.y - up_pt);
        let surface = layout.eye_point_on_surface(eye_point);
        let scale = f64::from(PX_PER_PT);
        ((surface.x * scale).round() as i64, (surface.y * scale).round() as i64)
    }
}

/// Средний цвет квадрата `±PROBE_HALF_PX` вокруг пикселя.
fn mean_color(snapshot: &Snapshot, center: (i64, i64)) -> [f32; 3] {
    let mut sum = [0.0_f32; 3];
    let mut count = 0.0_f32;
    for y in (center.1 - PROBE_HALF_PX)..=(center.1 + PROBE_HALF_PX) {
        for x in (center.0 - PROBE_HALF_PX)..=(center.0 + PROBE_HALF_PX) {
            let pixel = snapshot.at(x as u32, y as u32);
            for channel in 0..3 {
                sum[channel] += pixel[channel];
            }
            count += 1.0;
        }
    }
    sum.map(|value| value / count)
}

/// Цвет ободка в направлении «глазами»: `(right, up)` — единичное направление.
fn ring_color(snapshot: &Snapshot, screen: &CameraScreen, probe: &CircleProbe, direction: (f64, f64)) -> [f32; 3] {
    let pixel = probe.pixel_at_eye_offset(
        screen,
        direction.0 * probe.ring_mid_radius_pt,
        direction.1 * probe.ring_mid_radius_pt,
    );
    mean_color(snapshot, pixel)
}

/// Окно вокруг точки круга, смещённой на `(right_pt, up_pt)` «глазами».
fn window_at(screen: &CameraScreen, probe: &CircleProbe, right_pt: f64, up_pt: f64) -> Window {
    let (x, y) = probe.pixel_at_eye_offset(screen, right_pt, up_pt);
    let half = (PUCK_WINDOW_HALF_PT * f64::from(PX_PER_PT)).round() as i64;
    (
        (x - half) as u32,
        (y - half) as u32,
        (x + half) as u32,
        (y + half) as u32,
    )
}

/// Независимые от эталона проверки вида колёс.
fn assert_wheels_view(rig: &mut RenderRig, driver: &ScreenDriver, snapshot: &Snapshot, context: &str) {
    let screen = driver.screen();
    let probe = CircleProbe::of(screen);

    // Ободок: оттенок 0° (+a, вправо) краснее/пурпурнее, 90° (+b, вверх) желтее, 270° (вниз) синее. Проверка идёт
    // по ГЛАЗНЫМ направлениям, поэтому в альбоме перевёрнутое (не повёрнутое вместе с корнем) кольцо её не пройдёт.
    let right = ring_color(snapshot, screen, &probe, (1.0, 0.0));
    assert!(
        right[0] > right[1] && right[0] >= right[2],
        "{context}: справа ободок краснее/пурпурнее: {right:?}"
    );
    let up = ring_color(snapshot, screen, &probe, (0.0, 1.0));
    assert!(
        up[0] > up[2] && up[1] > up[2],
        "{context}: сверху ободок желтее: {up:?}"
    );
    let down = ring_color(snapshot, screen, &probe, (0.0, -1.0));
    assert!(
        down[2] > down[0] && down[2] > down[1],
        "{context}: снизу ободок синее: {down:?}"
    );

    // Шайба: стоит в стороне толчка (вправо-вверх), а не в центре и не зеркально. Тот же экран с нейтральными
    // колёсами рисует шайбу в центре, поэтому разница с ним видна ровно там, где шайба есть в одном из кадров.
    let neutral = render_calibration(
        rig,
        &wheels_view(driver_orientation(driver), ColorWheels::NEUTRAL),
        ScopeImage::StandIn,
    );
    let offset_right = f64::from(LIFT_PUSH.0) * probe.pad_radius_pt;
    let offset_up = f64::from(LIFT_PUSH.1) * probe.pad_radius_pt;
    let at_push = window_at(screen, &probe, offset_right, offset_up);
    let mirrored = window_at(screen, &probe, -offset_right, -offset_up);
    assert!(
        snapshot.mean_difference_in(&neutral, at_push) > 0.01,
        "{context}: шайба толчка стоит в стороне толчка (вправо-вверх)"
    );
    assert!(
        snapshot.mean_difference_in(&neutral, mirrored) < 0.003,
        "{context}: в зеркальной точке (влево-вниз) шайбы нет ни в одном из кадров"
    );
}

fn driver_orientation(driver: &ScreenDriver) -> DeviceOrientation {
    driver
        .screen()
        .state()
        .calibration()
        .layout()
        .expect("раскладка раздела")
        .eye_orientation()
}

#[test]
fn calibration_wheels_portrait() {
    with_rig(|rig| {
        let driver = wheels_view(DeviceOrientation::Portrait, golden_wheels());
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_wheels_portrait", &snapshot);
        assert_calibration_zones(rig, &driver, &snapshot, "колёса, портрет");
        assert_wheels_view(rig, &driver, &snapshot, "колёса, портрет");
    });
}

#[test]
fn calibration_wheels_landscape_left() {
    with_rig(|rig| {
        let driver = wheels_view(DeviceOrientation::LandscapeLeft, golden_wheels());
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_wheels_landscape_left", &snapshot);
        assert_calibration_zones(rig, &driver, &snapshot, "колёса, альбом, левый бок");
        assert_wheels_view(rig, &driver, &snapshot, "колёса, альбом, левый бок");
    });
}
