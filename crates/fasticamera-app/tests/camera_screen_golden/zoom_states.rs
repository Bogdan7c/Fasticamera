//! Состояния экрана камеры, связанные с зумом и переключением объективов (сессия 13): лента упоров, колесо, фронтальная
//! камера, удержанный размытый кадр и карточка переворота. Как и в `states.rs`, экран доводится до вида действиями
//! и фактами камеры (ответ приложения), а не подменой состояния.

use camera_contract::fixtures::camera_id;
use camera_contract::ids::{LensFacing, LensRole};
use camera_contract::zoom::ZoomRatio;
use camera_screen::{ActiveLens, CameraFacts, LensFacts, LensSwitch, ScreenAction};
use ui_draw::frame_source::BackdropContinuity;

use crate::driver::{SETTLE_MS, ScreenDriver, live_facts, live_lens_facts, oneplus_zoom_catalog};

/// Камера роли `wide` в каталоге OnePlus 11.
const MAIN_CAMERA: &str = "2";
/// Камера роли `ultra_wide`.
const ULTRA_WIDE_CAMERA: &str = "3";
/// Фронтальная камера.
const FRONT_CAMERA: &str = "1";

/// Индекс упора ленты «0,6×» (самый широкий) в шкале задних камер.
pub const ULTRA_WIDE_STOP: usize = 0;
/// Сколько мс водитель идёт после действия переворота: вместе с двумя кадрами самого действия это ≈ треть оборота
/// (270 мс) — карточка ещё заметно шире нуля (ближе к половине оборота окно вырождается и не рисуется, см.
/// `transition_veil`).
pub const FLIP_CARD_AT_MS: i64 = 60;
/// Когда в снимке удержания размытие уже полное (автомат включает его за кадр).
const HELD_BLUR_AT_MS: i64 = 200;

/// Объективы работающей камеры роли `role` направления `facing`.
fn lens_facts_for(camera: &str, role: LensRole, facing: LensFacing, switch: LensSwitch) -> LensFacts {
    LensFacts {
        catalog: Some(oneplus_zoom_catalog()),
        active: Some(ActiveLens {
            camera: camera_id(camera),
            role,
            facing,
        }),
        switch,
        last_failure: None,
    }
}

fn facts_with(lens: LensFacts) -> CameraFacts {
    CameraFacts { lens, ..live_facts() }
}

/// Лента на 0,6×: пользователь тапнул самую широкую кнопку, движок переключился на ширик (факт камеры), переход доехал.
pub fn strip_with_ultra_wide_selected() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::ZoomStopTapped {
        stop: ULTRA_WIDE_STOP,
        at: driver.now(),
    });
    driver.act(ScreenAction::CameraFactsChanged(facts_with(lens_facts_for(
        ULTRA_WIDE_CAMERA,
        LensRole::UltraWide,
        LensFacing::Back,
        LensSwitch::Steady,
    ))));
    driver.run(SETTLE_MS);
    driver
}

/// Колесо зума открыто на 1,7×: палец лежит на ленте, значение ведётся пальцем (колесо открыто, пока палец держит).
pub fn wheel_open_at_one_point_seven() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::ZoomWheelTouched { at: driver.now() });
    driver.act(ScreenAction::ZoomWheelDragged {
        requested: ZoomRatio::new(1.7).expect("коэффициент"),
        at: driver.now(),
    });
    driver.run(SETTLE_MS / 4);
    driver
}

/// Фронтальная камера после переворота: движок отдал кадры фронталки, переход (оборот карточки) доехал. У фронталки одна
/// камера, шкалы выбора нет — ленты упоров на экране нет.
pub fn front_camera_without_strip() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::FlipCameraRequested { at: driver.now() });
    driver.act(ScreenAction::CameraFactsChanged(facts_with(lens_facts_for(
        FRONT_CAMERA,
        LensRole::Front,
        LensFacing::Front,
        LensSwitch::Steady,
    ))));
    driver.run(SETTLE_MS);
    driver
}

/// Удержанный кадр на смене объектива: пользователь тапнул 0,6×, движок переключается (`Switching`), рендер держит
/// последний кадр основной камеры — экран размывает его (стекло `Frost`).
pub fn held_frame_while_switching_to_ultra_wide() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::ZoomStopTapped {
        stop: ULTRA_WIDE_STOP,
        at: driver.now(),
    });
    driver.act(ScreenAction::CameraFactsChanged(facts_with(lens_facts_for(
        MAIN_CAMERA,
        LensRole::Wide,
        LensFacing::Back,
        LensSwitch::Switching {
            from: camera_id(MAIN_CAMERA),
            to: camera_id(ULTRA_WIDE_CAMERA),
        },
    ))));
    driver.set_backdrop(BackdropContinuity::HeldLastFrame { since: driver.now() });
    driver.run(HELD_BLUR_AT_MS);
    driver
}

/// Карточка переворота посреди оборота: после тапа по кнопке переворота прошла треть оборота, рендер держит кадр.
pub fn flip_card_mid_rotation() -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.set_backdrop(BackdropContinuity::HeldLastFrame { since: driver.now() });
    driver.act(ScreenAction::FlipCameraRequested { at: driver.now() });
    driver.act(ScreenAction::CameraFactsChanged(facts_with(LensFacts {
        switch: LensSwitch::Switching {
            from: camera_id(MAIN_CAMERA),
            to: camera_id(FRONT_CAMERA),
        },
        ..live_lens_facts()
    })));
    driver.run(FLIP_CARD_AT_MS);
    driver
}
