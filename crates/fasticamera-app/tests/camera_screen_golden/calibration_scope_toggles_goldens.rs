//! Golden-кадры страниц с переключателями в полоске прибора (сессия 14s, решения 331, 332, 335): HISTOGRAM с режимом
//! PARADE и CIE со шкалой u'v', портрет. Прибор в стенде настоящий не считается: на зону картинки кладётся подставное
//! «фото» (`ScopeImage::StandIn`), как у остальных golden раздела. Кроме сверки с эталоном каждый тест проверяет
//! пиксели независимо от него: смена режима или шкалы меняет ТОЛЬКО полоску прибора, остальной экран не трогает.

use camera_screen::ScreenAction;
use scope_contract::{CieDiagram, HistogramMode, ScopeKind};
use shell_contract::orientation::DeviceOrientation;

use crate::calibration_goldens::{calibration, differing_pixels, eye_zone_window, render_calibration};
use crate::driver::{HEIGHT_PX, SETTLE_MS, ScreenDriver, WIDTH_PX};
use crate::golden_files::check_golden;
use crate::render_rig::ScopeImage;
use crate::with_rig;

/// Сколько пикселей полоски обязано отличаться между двумя выборами переключателя: подсвеченный сегмент переехал.
const MIN_STRIP_DIFFERENCE_PX: usize = 200;

/// Раздел калибровки, портрет, на странице `kind`.
fn on_page(kind: ScopeKind) -> ScreenDriver {
    let mut driver = calibration(DeviceOrientation::Portrait);
    driver.act(ScreenAction::ScopeKindSelected(kind));
    driver.run(SETTLE_MS);
    driver
}

/// Окно полоски прибора в px поверхности.
fn strip_window(driver: &ScreenDriver) -> crate::Window {
    let strip = driver
        .screen()
        .state()
        .calibration()
        .layout()
        .expect("раскладка раздела")
        .scope_strip();
    eye_zone_window(driver, strip)
}

/// Две раскладки одной страницы с разным выбором переключателя отличаются в полоске и больше нигде.
fn assert_only_the_strip_differs(
    driver: &ScreenDriver,
    first: &crate::render_rig::Snapshot,
    second: &crate::render_rig::Snapshot,
) {
    let (x0, y0, x1, y1) = strip_window(driver);
    assert!(
        differing_pixels(first, second, (x0, y0, x1, y1)) >= MIN_STRIP_DIFFERENCE_PX,
        "выбор переключателя виден в полоске"
    );
    assert_eq!(
        differing_pixels(first, second, (0, 0, WIDTH_PX, y0)),
        0,
        "выше полоски кадры одинаковы"
    );
    assert_eq!(
        differing_pixels(first, second, (0, y1, WIDTH_PX, HEIGHT_PX)),
        0,
        "ниже полоски (картинка прибора, ручки, нижний ряд) кадры одинаковы"
    );
}

#[test]
fn calibration_histogram_parade_portrait() {
    with_rig(|rig| {
        let mut driver = on_page(ScopeKind::Histogram);
        let rgb = render_calibration(rig, &driver, ScopeImage::StandIn);
        driver.act(ScreenAction::HistogramModeSelected(HistogramMode::Parade));
        driver.run(SETTLE_MS);
        assert_eq!(
            driver.screen().state().calibration().histogram_mode(),
            HistogramMode::Parade
        );
        let parade = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_histogram_parade_portrait", &parade);
        assert_only_the_strip_differs(&driver, &rgb, &parade);
    });
}

#[test]
fn calibration_cie_uv_portrait() {
    with_rig(|rig| {
        let mut driver = on_page(ScopeKind::Cie);
        let xy = render_calibration(rig, &driver, ScopeImage::StandIn);
        driver.act(ScreenAction::CieDiagramSelected(CieDiagram::UvPrime1976));
        driver.run(SETTLE_MS);
        assert_eq!(
            driver.screen().state().calibration().cie_diagram(),
            CieDiagram::UvPrime1976
        );
        let uv = render_calibration(rig, &driver, ScopeImage::StandIn);
        check_golden("calibration_cie_uv_portrait", &uv);
        assert_only_the_strip_differs(&driver, &xy, &uv);
    });
}
