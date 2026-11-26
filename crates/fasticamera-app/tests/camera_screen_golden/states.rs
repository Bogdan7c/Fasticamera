//! Ключевые состояния экрана камеры: каждое доводится до вида настоящими касаниями и действиями.
//! Время детерминировано: все прогоны идут кадрами по 16 мс от нуля, а перед снимком анимации доводятся до покоя
//! (`SETTLE_MS`), кроме двух осознанных промежуточных моментов (рамка фокуса и отсчёт таймера — см. их доки).

use camera_screen::settings::{AspectRatio, TimerSetting};
use camera_screen::tools::{ToolId, ToolOption};
use camera_screen::{CameraFacts, CameraPresence, ScreenAction, UnavailableReason};
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::Point;
use ui_draw::icons::IconId;

use crate::driver::{PX_PER_PT, SETTLE_MS, ScreenDriver, locked_facts_at};

/// Тап по пикселю-центру иконки: так пользователь жмёт кнопку, а не вызывает действие мимо жеста.
fn tap_icon(driver: &mut ScreenDriver, icon: IconId) {
    let (x_px, y_px) = driver.icon_center_px(icon).expect("иконка кнопки есть в списке");
    driver.tap(Point::new(f64::from(x_px / PX_PER_PT), f64::from(y_px / PX_PER_PT)));
}

/// Покой 4:3.
pub fn rest() -> ScreenDriver {
    ScreenDriver::new()
}

/// Покой 4:3 в том виде, в каком его показывает продукт до сессии 14d: камера не сообщила вспышку, значка вспышки
/// в ряду над кадром нет (инвариант 6, решение 138). Остальные golden-кадры сняты со вспышкой (`driver::live_facts`).
pub fn rest_without_flash() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::CameraFactsChanged(CameraFacts {
        flash: camera_screen::FlashAvailability::Unavailable,
        ..crate::driver::live_facts()
    }));
    driver.run(SETTLE_MS);
    driver
}

/// Тап по центру кадра. Снимок через 400 мс: рамка уже появилась (пружина доехала), но ещё не потускнела
/// (потускнение — через `dim_after_s` токенов, секунды).
pub fn focus_frame_after_tap() -> (ScreenDriver, Point) {
    let mut driver = ScreenDriver::new();
    let center = driver.frame_center();
    driver.tap(center);
    driver.run(400);
    (driver, center)
}

/// Долгий тап по центру кадра и подтверждение блокировки камерой (факт `Locked` с точкой интереса).
pub fn ae_af_lock() -> (ScreenDriver, Point) {
    let mut driver = ScreenDriver::new();
    let center = driver.frame_center();
    driver.long_press(center);
    let target = ScreenDriver::lock_target_in(&driver.drain_effects()).expect("долгий тап просит блокировку у камеры");
    driver.act(ScreenAction::CameraFactsChanged(locked_facts_at(target, 1)));
    driver.run(SETTLE_MS);
    (driver, center)
}

/// Меню Controls, открытое тапом по кнопке меню.
pub fn controls_menu() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    tap_icon(&mut driver, IconId::ControlsMenu);
    driver.run(SETTLE_MS);
    driver
}

/// Подменю таймера: пункт Timer в меню Controls.
pub fn timer_submenu() -> ScreenDriver {
    let mut driver = controls_menu();
    driver.act(ScreenAction::ControlsItemSelected(ToolId::Timer));
    driver.run(SETTLE_MS);
    driver
}

/// Панель Exposure: значок в полосе над кадром, затем «камера» восстанавливает +1 EV.
pub fn exposure_panel(ev: f64) -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::StripToolTapped(ToolId::Exposure));
    driver.run(SETTLE_MS);
    if ev != 0.0 {
        driver.restore_exposure(ev);
        driver.run(SETTLE_MS);
    }
    driver
}

/// Выбор соотношения кадра через меню Controls → Aspect, меню закрыто.
pub fn with_aspect(aspect: AspectRatio) -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::ControlsMenuOpenRequested);
    driver.act(ScreenAction::ControlsItemSelected(ToolId::Aspect));
    driver.act(ScreenAction::SubmenuOptionSelected(ToolOption::Aspect(aspect)));
    driver.act(ScreenAction::ControlsMenuCloseRequested);
    driver.run(SETTLE_MS);
    driver
}

/// Отсчёт таймера на 3 с: таймер выбран в меню, затвор нажат тапом. Снимок через 300 мс после нажатия —
/// на экране цифра «3» (до смены на «2» ещё 700 мс).
pub fn timer_countdown() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::ControlsMenuOpenRequested);
    driver.act(ScreenAction::ControlsItemSelected(ToolId::Timer));
    driver.act(ScreenAction::SubmenuOptionSelected(ToolOption::Timer(
        TimerSetting::Seconds3,
    )));
    driver.act(ScreenAction::ControlsMenuCloseRequested);
    driver.run(SETTLE_MS);
    let (x_px, y_px) = driver.element_circle_center_px(SHUTTER_DIAMETER_PX).expect("затвор");
    let shutter = Point::new(f64::from(x_px / PX_PER_PT), f64::from(y_px / PX_PER_PT));
    driver.tap(shutter);
    driver.run(300);
    driver
}

/// Камера занята другим приложением.
pub fn camera_unavailable() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::CameraFactsChanged(CameraFacts {
        presence: CameraPresence::Unavailable {
            reason: UnavailableReason::InUse,
        },
        // Раскладка golden — со значком вспышки (см. `driver::live_facts`).
        flash: camera_screen::FlashAvailability::Available,
        ..CameraFacts::STARTING
    }));
    driver.run(SETTLE_MS);
    driver
}

/// Телефон повёрнут в альбомную ориентацию (левый бок вниз).
pub fn landscape_left() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    driver.run(SETTLE_MS);
    driver
}

/// Диаметр затвора, px: Ø80,1 pt (`layout.md` 4.3) × 2,4545 ≈ 197.
pub const SHUTTER_DIAMETER_PX: f32 = 197.0;
