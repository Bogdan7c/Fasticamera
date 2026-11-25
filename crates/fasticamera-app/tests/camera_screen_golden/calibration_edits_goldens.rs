//! Golden-кадры правок в разделе калибровки (14l, решения 234, 251 и 200): целые числа шкалы −200…+200, метка значения
//! пресета на линейке и кольцо «есть правки» у точки полосы.
//!
//! Два кадра на портрете: верх списка (числа со знаком, метки пресета у правленых ручек) и конец списка (ряд точек
//! BANDS: полоса с правками, сохранёнными в пресете, и полоса с правками поверх пресета).

use camera_screen::ScreenAction;
use camera_screen::modes::CaptureMode;
use look_contract::{BandAxis, EditState, GlobalKnob, HueBandId, LookFacts, LookKnob};
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::Point;

use crate::calibration_goldens::{as_eye, look_facts_with_wheels, render_calibration};
use crate::driver::{SETTLE_MS, ScreenDriver};
use crate::golden_files::check_golden;
use crate::render_rig::ScopeImage;
use crate::with_rig;
use look_contract::ColorWheels;

fn knob(value: f32) -> LookKnob {
    LookKnob::new(value).expect("значение теста в диапазоне")
}

fn band(id: &str) -> HueBandId {
    HueBandId::new(id).expect("идентификатор полосы допустим")
}

/// Факты с правками: контраст +30 (0,15) при пресете +60 (0,30; метка пресета правее), тени −74 (−0,37) при пресете 0 (метка левее),
/// полоса `sky` правлена и правка лежит в пресете, полоса `red` правлена поверх пресета.
fn edited_facts() -> LookFacts {
    let mut facts = look_facts_with_wheels(as_eye(), EditState::EditedOnTop, None, ColorWheels::NEUTRAL);
    facts.preset.settings = facts.preset.settings.with_knob(GlobalKnob::Contrast, knob(0.30));
    facts.current.settings = facts
        .current
        .settings
        .with_knob(GlobalKnob::Contrast, knob(0.15))
        .with_knob(GlobalKnob::Shadows, knob(-0.37));
    let sky = facts
        .current
        .settings
        .bands
        .for_band(&band("sky"))
        .with_axis(BandAxis::Saturation, knob(-0.2));
    facts.current.settings.bands = facts.current.settings.bands.with_adjustment(sky.clone());
    facts.preset.settings.bands = facts.preset.settings.bands.with_adjustment(sky);
    let red = facts
        .current
        .settings
        .bands
        .for_band(&band("red"))
        .with_axis(BandAxis::Hue, knob(0.5));
    facts.current.settings.bands = facts.current.settings.bands.with_adjustment(red);
    facts
}

fn edited_section() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::LookFactsChanged(Box::new(edited_facts())));
    driver.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::Portrait));
    driver.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    driver.run(SETTLE_MS);
    driver
}

/// Настоящим пальцем листает список ручек до конца (вертикальная протяжка по подписи строки).
fn scroll_list_to_end(driver: &mut ScreenDriver) {
    let layout = *driver.screen().state().calibration().layout().expect("раскладка");
    let column = layout.zones().knobs;
    for _ in 0..4 {
        let start = Point::new(column.min_x() + 40.0, column.center().y + 120.0);
        driver.drag(
            layout.eye_point_on_surface(start),
            layout.eye_point_on_surface(Point::new(start.x, start.y - 300.0)),
        );
        driver.run(SETTLE_MS);
    }
}

#[test]
fn calibration_edits_tone_portrait() {
    with_rig(|rig| {
        let driver = edited_section();
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_edits_tone_portrait", &snapshot);
    });
}

#[test]
fn calibration_edits_bands_portrait() {
    with_rig(|rig| {
        let mut driver = edited_section();
        scroll_list_to_end(&mut driver);
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_edits_bands_portrait", &snapshot);
    });
}
