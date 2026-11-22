//! Golden-кадры миниатюры с настоящей картинкой снимка (сессия 14g, решения 154, 164, 165).
//!
//! Цепочка как в приложении: тап по затвору → эффект снимка с номером → «сохранено» вместе с картинкой
//! (`CameraScreen::handle_saved_with_picture`, так их доставляет учёт снимков) → растворение доведено до конца →
//! настоящий `UiRenderer` с основными цветами Display P3 → readback.
//!
//! Картинка — «снимок» 4:3 из двух половин: тёплая сверху, синяя снизу. Кроме сверки с эталоном каждый тест
//! проверяет пиксели сам: где внутри круга тёплое, а где синее. Поэтому тест падает при поломке (картинка не
//! нарисована, не повёрнута со значками, перевёрнута), даже если эталон перегенерировать.

use camera_screen::Effect;
use camera_screen::ScreenAction;
use camera_screen::shutter::ShotRequestId;
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::Point;
use ui_draw::picture::{PictureEncoding, PictureSize, UiPicture};

use crate::driver::{PX_PER_PT, SETTLE_MS, ScreenDriver};
use crate::golden_files::check_golden;
use crate::{render, shutter_px, with_rig};

/// Диаметр стеклянного круга миниатюры, px: Ø48 pt × 2,4545 ≈ 118 (`layout.md`, `bottom.thumbnail`).
const THUMBNAIL_DIAMETER_PX: f32 = 117.8;
/// Насколько от центра круга брать пробу: картинка занимает 86 % круга (решение 164, радиус ≈ 50 px),
/// проба на полпути к краю — внутри картинки и далеко от границы половин.
const PROBE_OFFSET_PX: u32 = 25;
/// Размер «снимка»: 4:3, как основной кадр.
const PICTURE_SIZE: PictureSize = PictureSize::new(64, 48);
/// Тёплая верхняя и синяя нижняя половины «снимка» (sRGB).
const WARM_TOP: [u8; 4] = [230, 150, 30, 255];
const BLUE_BOTTOM: [u8; 4] = [30, 80, 220, 255];

/// «Снимок»: верхняя половина тёплая, нижняя синяя — «верх» картинки виден по цвету.
fn two_halves_picture() -> UiPicture {
    let mut rgba = Vec::with_capacity(PICTURE_SIZE.pixel_count() as usize * 4);
    for row in 0..PICTURE_SIZE.height {
        let texel = if row < PICTURE_SIZE.height / 2 {
            WARM_TOP
        } else {
            BLUE_BOTTOM
        };
        for _ in 0..PICTURE_SIZE.width {
            rgba.extend_from_slice(&texel);
        }
    }
    UiPicture::from_rgba8(PICTURE_SIZE, PictureEncoding::Srgb, rgba).expect("картинка собирается")
}

/// Снимок настоящим тапом по затвору: номер берётся из эффекта экрана, как его получает служба снимков.
fn press_shutter(driver: &mut ScreenDriver) -> ShotRequestId {
    driver.drain_effects();
    let (x_px, y_px) = shutter_px(driver);
    driver.tap(Point::new(
        f64::from(x_px as f32 / PX_PER_PT),
        f64::from(y_px as f32 / PX_PER_PT),
    ));
    driver
        .drain_effects()
        .iter()
        .find_map(|effect| match effect {
            Effect::Capture(shot) => Some(shot.request),
            _ => None,
        })
        .expect("тап по затвору просит снимок")
}

/// Экран после сохранённого снимка с картинкой, растворение закончено.
fn saved_shot_with_picture(orientation: DeviceOrientation) -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    if orientation != DeviceOrientation::Portrait {
        driver.act(ScreenAction::DeviceOrientationChanged(orientation));
        driver.run(SETTLE_MS);
    }
    let request = press_shutter(&mut driver);
    driver.save_with_picture(request, two_halves_picture());
    driver.run(SETTLE_MS);
    driver
}

fn is_warm(pixel: [f32; 3]) -> bool {
    pixel[0] > 0.6 && pixel[2] < 0.35
}

fn is_blue(pixel: [f32; 3]) -> bool {
    pixel[2] > 0.6 && pixel[0] < 0.35
}

/// Центр стеклянного круга миниатюры, px (целые).
fn thumbnail_center_px(driver: &ScreenDriver) -> (u32, u32) {
    let (x, y) = driver
        .glass_circle_center_px(THUMBNAIL_DIAMETER_PX)
        .expect("круг миниатюры в списке");
    (x.round() as u32, y.round() as u32)
}

#[test]
fn thumbnail_with_the_saved_picture_upright() {
    with_rig(|rig| {
        let driver = saved_shot_with_picture(DeviceOrientation::Portrait);
        let snapshot = render(rig, &driver);
        let (center_x, center_y) = thumbnail_center_px(&driver);
        // Телефон вертикально: верх снимка — вверху круга.
        let above = snapshot.at(center_x, center_y - PROBE_OFFSET_PX);
        let below = snapshot.at(center_x, center_y + PROBE_OFFSET_PX);
        assert!(
            is_warm(above),
            "над центром миниатюры тёплая половина снимка: {above:?}"
        );
        assert!(is_blue(below), "под центром миниатюры синяя половина снимка: {below:?}");
        check_golden("thumbnail_picture", &snapshot);
    });
}

#[test]
fn thumbnail_picture_turns_with_the_icons_in_landscape() {
    with_rig(|rig| {
        let driver = saved_shot_with_picture(DeviceOrientation::LandscapeLeft);
        let snapshot = render(rig, &driver);
        let (center_x, center_y) = thumbnail_center_px(&driver);
        // Телефон повёрнут верхом влево: значки (и картинка, решение 165) повёрнуты на 90° по часовой —
        // верх снимка смотрит вправо, для глаза он снова сверху.
        let right = snapshot.at(center_x + PROBE_OFFSET_PX, center_y);
        let left = snapshot.at(center_x - PROBE_OFFSET_PX, center_y);
        assert!(is_warm(right), "справа от центра тёплая половина снимка: {right:?}");
        assert!(is_blue(left), "слева от центра синяя половина снимка: {left:?}");
        check_golden("thumbnail_picture_landscape_left", &snapshot);
    });
}
