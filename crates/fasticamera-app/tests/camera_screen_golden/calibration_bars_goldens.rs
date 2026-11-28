//! Golden-кадры вида BARS раздела калибровки (сессия 14t, пакет P3): портрет и альбом.
//!
//! Вид доводится настоящими действиями экрана (`ControlViewSelected(Bars)`, `WheelSelected(Gain)`), полоски приходят
//! фактами о виде: Gain.R правлена вверх, Lift.B — вниз, остальные нейтральны. Поэтому видно яркую «↻» только у
//! правленой полоски выбранной зоны и метки-кольца на вкладках GAIN и LIFT (зоны с правками), а вкладки GAMMA и OFFSET
//! без колец.
//!
//! Кроме сверки с эталоном тест независимо проверяет пиксели ОБОИХ положений телефона: подпись каждой шкалы красится
//! своим цветом канала и в чужой строке этого цвета нет (так ловится перепутанный порядок R, G, B).

use camera_screen::calibration::{ControlViewKind, bars_ruler_placements};
use camera_screen::{CameraScreen, ScreenAction};
use look_contract::{EditState, LookKnob, PrimaryChannel, WheelZone};
use shell_contract::orientation::DeviceOrientation;

use crate::calibration_goldens::{as_eye, assert_calibration_zones, look_facts_with_wheels, render_calibration};
use crate::driver::{PX_PER_PT, SETTLE_MS, ScreenDriver};
use crate::golden_files::check_golden;
use crate::render_rig::{ScopeImage, Snapshot};
use crate::with_rig;

/// Gain.R в голдене: заметно вверх.
const GAIN_RED: f32 = 0.4;
/// Lift.B в голдене: заметно вниз.
const LIFT_BLUE: f32 = -0.3;

/// Раздел калибровки в виде BARS: выбрана вкладка Gain, правлены Gain.R и Lift.B.
pub(crate) fn bars_view(orientation: DeviceOrientation) -> ScreenDriver {
    let mut facts = look_facts_with_wheels(
        as_eye(),
        EditState::EditedOnTop,
        None,
        look_contract::ColorWheels::NEUTRAL,
    );
    let primaries = facts.current.settings.primaries;
    facts.current.settings.primaries = primaries
        .with_value(
            WheelZone::Gain,
            PrimaryChannel::Red,
            LookKnob::new(GAIN_RED).expect("значение допустимо"),
        )
        .with_value(
            WheelZone::Lift,
            PrimaryChannel::Blue,
            LookKnob::new(LIFT_BLUE).expect("значение допустимо"),
        );
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::LookFactsChanged(Box::new(facts)));
    driver.act(ScreenAction::DeviceOrientationChanged(orientation));
    driver.act(ScreenAction::ModeSelected(camera_screen::modes::CaptureMode::Calibrate));
    driver.act(ScreenAction::ControlViewSelected(ControlViewKind::Bars));
    driver.act(ScreenAction::WheelSelected(WheelZone::Gain));
    driver.run(SETTLE_MS);
    driver
}

/// Каков цвет пикселя из подписей каналов (`widgets.bars.channel_colors` по умолчанию): `Some(канал)` — пиксель
/// явно красный, зелёный или синий; белое, серое, жёлтое и тёмное — `None`.
fn channel_of_pixel(pixel: [f32; 3]) -> Option<PrimaryChannel> {
    let [red, green, blue] = pixel;
    if red > 0.7 && green < 0.5 && blue < 0.5 {
        Some(PrimaryChannel::Red)
    } else if green > 0.6 && red < 0.5 && blue < 0.6 {
        Some(PrimaryChannel::Green)
    } else if blue > 0.8 && red < 0.55 && green < 0.7 {
        Some(PrimaryChannel::Blue)
    } else {
        None
    }
}

/// Сколько пикселей строки `index` (поверхность, px) окрашено в цвет `channel`.
fn channel_pixels_in_row(screen: &CameraScreen, snapshot: &Snapshot, index: usize, channel: PrimaryChannel) -> usize {
    let placements = bars_ruler_placements(
        screen.state().calibration().layout().expect("раскладка раздела"),
        &camera_screen::calibration::CalibrationWidgetsConfig::default(),
    )
    .expect("вид BARS открыт, шкалы помещаются");
    let row = placements[index].row;
    let scale = f64::from(PX_PER_PT);
    let (x0, y0) = ((row.min_x() * scale) as u32, (row.min_y() * scale) as u32);
    let (x1, y1) = ((row.max_x() * scale) as u32, (row.max_y() * scale) as u32);
    let mut count = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            if channel_of_pixel(snapshot.at(x, y)) == Some(channel) {
                count += 1;
            }
        }
    }
    count
}

/// Независимые от эталона проверки вида BARS: подпись каждой шкалы красится своим цветом, чужих цветов в строке нет.
fn assert_bars_view(driver: &ScreenDriver, snapshot: &Snapshot, context: &str) {
    for (index, own) in PrimaryChannel::ALL.into_iter().enumerate() {
        for channel in PrimaryChannel::ALL {
            let count = channel_pixels_in_row(driver.screen(), snapshot, index, channel);
            if channel == own {
                assert!(
                    count > 10,
                    "{context}: в строке {own:?} есть подпись цвета {own:?} ({count} px)"
                );
            } else {
                assert_eq!(count, 0, "{context}: в строке {own:?} нет цвета {channel:?}");
            }
        }
    }
}

#[test]
fn calibration_bars_portrait() {
    with_rig(|rig| {
        let driver = bars_view(DeviceOrientation::Portrait);
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_bars_portrait", &snapshot);
        assert_calibration_zones(rig, &driver, &snapshot, "полоски, портрет");
        assert_bars_view(&driver, &snapshot, "полоски, портрет");
    });
}

#[test]
fn calibration_bars_landscape_left() {
    with_rig(|rig| {
        let driver = bars_view(DeviceOrientation::LandscapeLeft);
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_bars_landscape_left", &snapshot);
        assert_calibration_zones(rig, &driver, &snapshot, "полоски, альбом, левый бок");
        assert_bars_view(&driver, &snapshot, "полоски, альбом, левый бок");
    });
}
