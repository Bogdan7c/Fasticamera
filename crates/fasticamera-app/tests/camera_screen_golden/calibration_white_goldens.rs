//! Golden-кадры карточки белого в разделе калибровки (14n, решения 290–294): капсула «K» в полоске прибора и нижняя
//! карточка на поверхности раздела, портрет и альбом. Кадры доводятся настоящими действиями и касаниями экрана
//! (тап по капсуле), рисуются настоящим `UiRenderer`; кроме эталона тест проверяет пиксели независимо от него.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{HeldWhite, WhiteOrigin};
use camera_screen::calibration::SectionCard;
use camera_screen::modes::CaptureMode;
use camera_screen::{CameraFacts, ScreenAction, WhiteFacts};
use look_contract::EditState;
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::{Rect, Size};

use crate::calibration_goldens::{as_eye, eye_zone_window, look_facts, render_calibration};
use crate::driver::{SETTLE_MS, ScreenDriver, live_facts};
use crate::golden_files::check_golden;
use crate::render_rig::ScopeImage;
use crate::{focus_yellow, with_rig};

/// Раздел с ручным белым 4300 K: число в капсуле жёлтое (как у ручного белого в PHOTO).
fn section_with_white(orientation: DeviceOrientation) -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::LookFactsChanged(Box::new(look_facts(
        as_eye(),
        EditState::Clean,
        None,
    ))));
    driver.act(ScreenAction::CameraFactsChanged(CameraFacts {
        white: WhiteFacts {
            held: HeldWhite {
                setting: WhiteBalanceSetting {
                    temperature: ColorTemperature::from_kelvin(4300.0).expect("температура"),
                    tint: Tint::from_duv(0.004).expect("оттенок"),
                },
                origin: WhiteOrigin::Manual,
            },
            ..WhiteFacts::STARTING
        },
        ..live_facts()
    }));
    driver.act(ScreenAction::DeviceOrientationChanged(orientation));
    driver.act(ScreenAction::ModeSelected(CaptureMode::Calibrate));
    driver.run(SETTLE_MS);
    driver
}

/// То же, карточка открыта настоящим тапом по капсуле.
fn white_card(orientation: DeviceOrientation) -> ScreenDriver {
    let mut driver = section_with_white(orientation);
    let capsule = driver
        .screen()
        .calibration_touch_targets()
        .and_then(|targets| targets.white_capsule)
        .expect("капсула белого в полоске");
    driver.tap(capsule);
    driver.run(SETTLE_MS);
    assert_eq!(driver.screen().state().calibration().card(), SectionCard::White);
    driver
}

fn assert_white_section(
    (driver, orientation): (&ScreenDriver, DeviceOrientation),
    rig: &mut crate::render_rig::RenderRig,
    context: &str,
) {
    let card = render_calibration(rig, driver, ScopeImage::StandIn);
    let closed_driver = section_with_white(orientation);
    let closed = render_calibration(rig, &closed_driver, ScopeImage::StandIn);
    let layout = *driver.screen().state().calibration().layout().expect("раскладка");
    // Без карточки подпись CALIBRATE в капсуле режимов жёлтая (опорный кадр проверки), а нижний ряд под карточкой
    // не рисуется: что именно скрыто, доказывают тесты экрана (`screen_white_section_tests`: нет текста CLEAN и значка
    // меню в списке рисования), здесь — что карточка закрыла низ целиком.
    let capsule = eye_zone_window(
        driver,
        Rect::centered_at(layout.carousel_center(), Size::new(80.0, 24.0)),
    );
    assert!(
        closed.count_in(capsule, focus_yellow) > 40,
        "{context}: без карточки CALIBRATE в капсуле жёлтое"
    );
    // Карточка заметно меняет низ раздела, а верх (колонка ручек, чистая зона) остаётся как был.
    let bottom = eye_zone_window(driver, layout.zones().bottom_row);
    assert!(
        card.mean_difference_in(&closed, bottom) > 0.02,
        "{context}: карточка закрыла низ"
    );
    let clean = eye_zone_window(driver, layout.zones().clean_preview);
    assert!(
        card.mean_difference_in(&closed, clean) < 1e-6,
        "{context}: чистая зона превью карточкой не тронута"
    );
}

#[test]
fn calibration_white_card_portrait() {
    with_rig(|rig| {
        let driver = white_card(DeviceOrientation::Portrait);
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_white_card_portrait", &snapshot);
        assert_white_section((&driver, DeviceOrientation::Portrait), rig, "портрет");
    });
}

#[test]
fn calibration_white_card_landscape_left() {
    with_rig(|rig| {
        let driver = white_card(DeviceOrientation::LandscapeLeft);
        let snapshot = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_white_card_landscape_left", &snapshot);
        assert_white_section((&driver, DeviceOrientation::LandscapeLeft), rig, "альбом, левый бок");
    });
}
