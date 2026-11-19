//! Golden-кадры экрана камеры на GPU ПК (сессия 12, пакет P6b).
//!
//! Цепочка целиком, как в приложении: настоящие касания/действия → `CameraScreen` → `DrawList` →
//! настоящий `UiRenderer` (Vulkan, Mesa RADV) поверх фиксированного синтетического «фото» 1080 × 2412 →
//! readback → (1) смысловые проверки пикселей, не зависящие от эталона, и (2) сверка с эталоном
//! `test-assets/camera-screen/golden/*.pam` с допуском (формат и команды — в README каталога).
//!
//! Композиция здесь, а не в `camera-screen`: экран GPU-crate-ы не видит (guardrail), сшивает их composition root.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение и есть способ сообщить о провале"
)]

mod driver;
mod golden_files;
mod render_rig;
mod states;

use camera_screen::PanelState;
use camera_screen::settings::AspectRatio;
use camera_screen::tools::ToolId;
use driver::{HEIGHT_PX, PX_PER_PT, ScreenDriver, WIDTH_PX};
use golden_files::{check_golden, is_update_mode};
use render_rig::{RenderRig, Snapshot, ViewfinderRectPx, assert_no_validation_errors, gpu_context_or_skip};
use states::SHUTTER_DIAMETER_PX;
use ui_draw::icons::IconId;

/// Рисует список водителя и возвращает снимок; проверка validation — после отпускания стенда в конце теста.
fn render(rig: &mut RenderRig, driver: &ScreenDriver) -> Snapshot {
    let (x0, y0, x1, y1) = frame_rect_px(driver, driver.screen().state().settings().aspect);
    rig.render(
        driver.list(),
        ViewfinderRectPx {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        },
    )
}

/// Выполняет `body` на стенде или молча выходит при осознанном пропуске GPU; в конце — validation без ошибок.
fn with_rig(body: impl FnOnce(&mut RenderRig)) {
    let Some(context) = gpu_context_or_skip() else { return };
    let mut rig = RenderRig::new(&context);
    body(&mut rig);
    drop(rig);
    assert_no_validation_errors(&context);
}

fn near_white(pixel: [f32; 3]) -> bool {
    pixel.iter().all(|channel| *channel > 0.9)
}

/// Жёлтый фокуса `#FFCC00` (токен `focus_yellow`) с запасом на сглаживание и альфу.
fn focus_yellow(pixel: [f32; 3]) -> bool {
    pixel[0] > 0.85 && (0.65..0.95).contains(&pixel[1]) && pixel[2] < 0.25
}

fn near_black(pixel: [f32; 3]) -> bool {
    pixel.iter().all(|channel| *channel < 0.03)
}

/// Окно в px: `(x0, y0, x1, y1)`.
type Window = (u32, u32, u32, u32);

/// Окно рамки фокуса вокруг центра кадра 4:3: рамка 225 px и «солнце» справа от неё.
const FOCUS_WINDOW: Window = (400, 1030, 780, 1300);
/// Окно плашки «AE/AF LOCK» у верхней границы кадра 4:3 (288 × 73 px).
const LOCK_PLAQUE_WINDOW: Window = (390, 475, 690, 560);
/// Окно цифры отсчёта в центре кадра 4:3.
const COUNTDOWN_WINDOW: Window = (420, 1030, 660, 1300);
/// Окно затвора (кольцо Ø197 вокруг центра (540, 2034)).
const SHUTTER_WINDOW: Window = (440, 1930, 640, 2140);
/// Окно значка вспышки в полосе над кадром.
const FLASH_ICON_WINDOW: Window = (370, 285, 460, 360);
/// Окно шкалы EV в панели Exposure (нижняя часть панели, где идут деления).
const EXPOSURE_SCALE_WINDOW: Window = (240, 2190, 680, 2290);
/// Окно подписи значения EV над значком «солнце» в полосе над кадром.
const EXPOSURE_STRIP_WINDOW: Window = (620, 285, 720, 410);
/// Окно нижних пунктов подменю таймера (серые «таблетки» 3s/5s/10s).
const SUBMENU_ROWS_WINDOW: Window = (130, 1960, 950, 2110);
/// Окно надписи «Camera unavailable» в центре кадра 4:3.
const UNAVAILABLE_NOTICE_WINDOW: Window = (380, 1140, 700, 1200);

/// Прямоугольник кадра в px: `(x0, y0, x1, y1)`.
fn frame_rect_px(driver: &ScreenDriver, aspect: AspectRatio) -> (u32, u32, u32, u32) {
    let rect = driver.screen().state().layout().expect("раскладка").frame_rect(aspect);
    let scale = f64::from(PX_PER_PT);
    (
        (rect.min_x() * scale).round() as u32,
        (rect.min_y() * scale).round() as u32,
        (rect.max_x() * scale).round() as u32,
        (rect.max_y() * scale).round() as u32,
    )
}

/// Центр затвора в покое, px (целые).
fn shutter_px(driver: &ScreenDriver) -> (u32, u32) {
    let (x, y) = driver
        .glass_circle_center_px(SHUTTER_DIAMETER_PX)
        .expect("затвор в списке");
    (x.round() as u32, y.round() as u32)
}

#[test]
fn rest_4x3() {
    with_rig(|rig| {
        let driver = states::rest();
        let snapshot = render(rig, &driver);
        let (sx, sy) = shutter_px(&driver);
        // Затвор: белый диск в центре кольца.
        assert!(
            near_white(snapshot.at(sx, sy)),
            "центр затвора белый: {:?}",
            snapshot.at(sx, sy)
        );
        // Кадр 4:3 (y 449..1889) показывает «фото»; вне кадра — чёрная маска: над кадром, под кадром и по краям полос.
        let (_, top, _, bottom) = frame_rect_px(&driver, AspectRatio::FourByThree);
        assert!(snapshot.luma(700, top + 250) > 0.2, "внутри кадра фото");
        assert!(
            near_black(snapshot.at(3, top - 20)),
            "маска над кадром: {:?}",
            snapshot.at(3, top - 20)
        );
        assert!(near_black(snapshot.at(1076, top - 20)), "маска над кадром справа");
        assert!(
            near_black(snapshot.at(3, bottom + 20)),
            "маска под кадром: {:?}",
            snapshot.at(3, bottom + 20)
        );
        // Значки над кадром, кнопка меню и капсула режимов на месте; рамки фокуса и плашки LOCK нет.
        assert!(driver.icon_center_px(IconId::FlashOff).is_some());
        assert!(driver.icon_center_px(IconId::ControlsMenu).is_some());
        assert_eq!(
            snapshot.count_in(FOCUS_WINDOW, focus_yellow),
            0,
            "рамки фокуса в покое нет"
        );
        assert_eq!(
            snapshot.count_in(LOCK_PLAQUE_WINDOW, focus_yellow),
            0,
            "плашки LOCK в покое нет"
        );
        check_golden("rest_4x3", &snapshot);
    });
}

#[test]
fn focus_frame_after_tap() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let (driver, tap_point) = states::focus_frame_after_tap();
        let snapshot = render(rig, &driver);
        // Жёлтая рамка и «солнце» вокруг точки тапа; в покое в том же окне жёлтого нет.
        let tap_px = (
            (tap_point.x * f64::from(PX_PER_PT)) as u32,
            (tap_point.y * f64::from(PX_PER_PT)) as u32,
        );
        assert!(
            (FOCUS_WINDOW.0..FOCUS_WINDOW.2).contains(&tap_px.0)
                && (FOCUS_WINDOW.1..FOCUS_WINDOW.3).contains(&tap_px.1),
            "окно проверки охватывает точку тапа {tap_px:?}"
        );
        let yellow = snapshot.count_in(FOCUS_WINDOW, focus_yellow);
        assert!(yellow > 1_500, "жёлтая рамка вокруг точки тапа: {yellow} px");
        assert_eq!(rest.count_in(FOCUS_WINDOW, focus_yellow), 0);
        // Центр рамки пуст: внутри видно «фото», а не заливку.
        assert!(!focus_yellow(snapshot.at(540, 1169)), "центр рамки прозрачный");
        check_golden("focus_frame", &snapshot);
    });
}

#[test]
fn ae_af_lock() {
    with_rig(|rig| {
        let (driver, _) = states::ae_af_lock();
        let snapshot = render(rig, &driver);
        assert!(
            driver.screen().state().focus().is_lock_plaque_visible(),
            "камера подтвердила блокировку"
        );
        // Плашка «AE/AF LOCK»: жёлтая капсула 288 × 73 px (за вычетом тёмного текста — не меньше половины окна капсулы).
        let plaque = snapshot.count_in(LOCK_PLAQUE_WINDOW, focus_yellow);
        assert!(plaque > 12_000, "жёлтая плашка LOCK: {plaque} px");
        // Рамка фокуса с точкой блокировки остаётся на месте.
        assert!(
            snapshot.count_in(FOCUS_WINDOW, focus_yellow) > 1_500,
            "рамка фокуса осталась"
        );
        check_golden("ae_af_lock", &snapshot);
    });
}

#[test]
fn controls_menu_open() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let driver = states::controls_menu();
        let snapshot = render(rig, &driver);
        assert_eq!(driver.screen().state().panel(), PanelState::ControlsMenu);
        // Затвор ушёл под шторку: там, где был белый диск, теперь тёмная стеклянная панель.
        let (sx, sy) = shutter_px(&states::rest());
        assert!(near_white(rest.at(sx, sy)));
        assert!(
            !near_white(snapshot.at(sx, sy)) && snapshot.luma(sx, sy) < 0.4,
            "под шторкой не белый диск: {:?}",
            snapshot.at(sx, sy)
        );
        assert!(
            driver.glass_circle_center_px(SHUTTER_DIAMETER_PX).is_none(),
            "затвор не рисуется"
        );
        check_golden("controls_menu", &snapshot);
    });
}

#[test]
fn timer_submenu() {
    with_rig(|rig| {
        let menu = render(rig, &states::controls_menu());
        let driver = states::timer_submenu();
        let snapshot = render(rig, &driver);
        assert_eq!(driver.screen().state().panel(), PanelState::Submenu(ToolId::Timer));
        // Подменю заменило иконки меню строками-«таблетками» вариантов: низ шторки заметно изменился.
        let difference = snapshot.mean_difference_in(&menu, SUBMENU_ROWS_WINDOW);
        assert!(
            difference > 0.03,
            "строки вариантов отличаются от сетки меню: {difference}"
        );
        check_golden("timer_submenu", &snapshot);
    });
}

#[test]
fn exposure_panel_plus_one_ev() {
    with_rig(|rig| {
        let zero = render(rig, &states::exposure_panel(0.0));
        let driver = states::exposure_panel(1.0);
        let snapshot = render(rig, &driver);
        assert_eq!(driver.screen().state().panel(), PanelState::ExposurePanel);
        assert_eq!(
            driver.screen().state().exposure().steps(),
            3,
            "+1 EV = 3 шага по 1/3 EV"
        );
        // Шкала сдвинулась (деления в другом месте) и подпись значения в полосе над кадром изменилась.
        let scale = snapshot.mean_difference_in(&zero, EXPOSURE_SCALE_WINDOW);
        assert!(scale > 0.005, "деления шкалы сдвинулись: {scale}");
        let label = snapshot.mean_difference_in(&zero, EXPOSURE_STRIP_WINDOW);
        assert!(label > 0.005, "подпись +1.0 над кадром: {label}");
        assert!(
            snapshot.count_in(EXPOSURE_STRIP_WINDOW, focus_yellow) > 200,
            "значок и подпись EV жёлтые"
        );
        assert_exposure_panel_clears_the_shutter(&driver, &snapshot);
        check_golden("exposure_plus1", &snapshot);
    });
}

/// Панель Exposure не перекрывает затвор (`motion.md` 4.1: затвор при панели виден и работает): белый диск
/// затвора целиком над стеклом панели и не закрывает число и шкалу панели.
fn assert_exposure_panel_clears_the_shutter(driver: &ScreenDriver, snapshot: &Snapshot) {
    let (panel_x0, panel_y0, panel_x1, panel_y1) = driver.glass_panel_rect_px().expect("стекло панели в списке");
    let (shutter_x, shutter_y) = shutter_px(driver);
    let ring_bottom = shutter_y + (SHUTTER_DIAMETER_PX / 2.0) as u32;
    // Рамки: кольцо затвора кончается выше верха стекла панели.
    assert!(
        (ring_bottom as f32) < panel_y0,
        "кольцо затвора (низ {ring_bottom}) заходит в панель (верх {panel_y0})"
    );
    // Диск затвора на месте и белый; в тонкой полосе у самого верха панели (там нет содержимого панели)
    // от диска нет ни одного белого пикселя.
    assert!(near_white(snapshot.at(shutter_x, shutter_y)), "диск затвора виден");
    let top_strip: Window = (
        shutter_x - 90,
        panel_y0 as u32 + 2,
        shutter_x + 90,
        panel_y0 as u32 + 12,
    );
    assert_eq!(
        snapshot.count_in(top_strip, near_white),
        0,
        "белый диск затвора лежит поверх стекла панели"
    );
    // Число «+1.0» под диском: светлый текст занимает небольшую долю окна, а не залито белым диском.
    let value_window: Window = (
        shutter_x - 60,
        panel_y0 as u32 + 14,
        shutter_x + 60,
        panel_y0 as u32 + 74,
    );
    let window_area = (value_window.2 - value_window.0) * (value_window.3 - value_window.1);
    let white = snapshot.count_in(value_window, near_white) as u32;
    assert!(
        white > 100 && white < window_area * 2 / 5,
        "число панели: {white} белых px из {window_area} (мало — число закрыто, много — залито диском)"
    );
    // Бока панели читаются как стекло: левый и правый края внутри экрана.
    assert!(
        panel_x0 > 0.0 && panel_x1 < WIDTH_PX as f32,
        "панель с боковыми отступами"
    );
    assert!(panel_y1 < HEIGHT_PX as f32, "панель выше нижнего края экрана");
}

#[test]
fn aspect_16x9() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let driver = states::with_aspect(AspectRatio::SixteenByNine);
        let snapshot = render(rig, &driver);
        assert_eq!(driver.screen().state().settings().aspect, AspectRatio::SixteenByNine);
        // Кадр 16:9 выше: полоса y 251..449 — «фото» (в 4:3 тут маска), а ниже и выше кадра — чёрное.
        let (_, top, _, bottom) = frame_rect_px(&driver, AspectRatio::SixteenByNine);
        assert!(near_black(rest.at(900, 350)), "в 4:3 здесь маска");
        assert!(
            snapshot.luma(900, 350) > 0.2,
            "в 16:9 здесь уже кадр: {:?}",
            snapshot.at(900, 350)
        );
        assert!(near_black(snapshot.at(900, top - 40)), "маска над кадром 16:9");
        assert!(near_black(snapshot.at(900, bottom + 40)), "маска под кадром 16:9");
        // Нижняя полоса y 1889..2171 в 4:3 чёрная, в 16:9 — «фото» (белая полоса мишени).
        assert!(near_black(rest.at(750, 2100)));
        assert!(near_white(snapshot.at(750, 2100)));
        check_golden("aspect_16x9", &snapshot);
    });
}

#[test]
fn timer_countdown() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let driver = states::timer_countdown();
        let snapshot = render(rig, &driver);
        // Крупная белая цифра по центру кадра; в покое там только «фото» без белого.
        let digit = snapshot.count_in(COUNTDOWN_WINDOW, near_white);
        assert!(digit > 2_500, "цифра отсчёта: {digit} белых px");
        assert_eq!(rest.count_in(COUNTDOWN_WINDOW, near_white), 0);
        // Снимок ещё не запрошен: отсчёт идёт.
        assert!(!driver.screen().state().is_camera_unavailable());
        check_golden("timer_countdown", &snapshot);
    });
}

#[test]
fn camera_unavailable() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let driver = states::camera_unavailable();
        let snapshot = render(rig, &driver);
        assert!(driver.screen().state().is_camera_unavailable());
        // Затвор недоступен: серый диск вместо белого.
        let (sx, sy) = shutter_px(&driver);
        assert!(near_white(rest.at(sx, sy)));
        let luma = snapshot.luma(sx, sy);
        assert!((0.2..0.7).contains(&luma), "диск затвора серый, не белый: {luma}");
        // Надпись «Camera unavailable» по центру кадра.
        let notice = snapshot.mean_difference_in(&rest, UNAVAILABLE_NOTICE_WINDOW);
        assert!(notice > 0.01, "текст-уведомление: {notice}");
        check_golden("camera_unavailable", &snapshot);
    });
}

#[test]
fn landscape_left() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let driver = states::landscape_left();
        let snapshot = render(rig, &driver);
        assert!((driver.screen().state().icon_rotation_clockwise_degrees() - 90.0).abs() < 0.5);
        // Раскладка та же: затвор не сдвинулся и не повернулся; значок вспышки повернулся на месте.
        let shutter = snapshot.mean_difference_in(&rest, SHUTTER_WINDOW);
        assert!(shutter < 0.005, "затвор остался как в портрете: {shutter}");
        let flash = snapshot.mean_difference_in(&rest, FLASH_ICON_WINDOW);
        assert!(flash > 0.01, "значок вспышки повернулся: {flash}");
        check_golden("landscape_left", &snapshot);
    });
}

/// Сам сравниватель эталонов: подмена кадра заметной ошибкой должна валить сверку, шум в пределах допуска — нет.
#[test]
fn golden_comparison_detects_a_visible_change_and_tolerates_noise() {
    if is_update_mode() {
        return; // при перегенерации сверка с эталоном не выполняется
    }
    with_rig(|rig| {
        let snapshot = render(rig, &states::rest());
        // Шум на один шаг 8-бит (1/255) по всему кадру — допуск его прощает.
        let mut noisy = Snapshot {
            pixels: snapshot.pixels.clone(),
        };
        for pixel in &mut noisy.pixels {
            pixel[0] = (pixel[0] + 1.0 / 255.0).min(1.0);
        }
        check_golden("rest_4x3", &noisy);
        // Белый квадрат 12 × 12 px (две стороны блока эталона 6 × 6) — мелкая, но заметная ошибка: сверка обязана
        // упасть. Квадрат лежит на тёмной части «фото», поэтому разница с эталоном велика.
        let mut broken = Snapshot {
            pixels: snapshot.pixels.clone(),
        };
        for y in 900..912 {
            for x in 200..212 {
                broken.pixels[(y * WIDTH_PX + x) as usize] = [1.0, 1.0, 1.0, 1.0];
            }
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check_golden("rest_4x3", &broken)));
        assert!(outcome.is_err(), "сверка пропустила заметно изменённый кадр");
    });
}
