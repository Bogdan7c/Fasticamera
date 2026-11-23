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

mod calibration_goldens;
mod driver;
mod golden_files;
mod render_rig;
mod states;
mod thumbnail_goldens;
mod zoom_goldens;
mod zoom_states;

use camera_screen::settings::AspectRatio;
use camera_screen::tools::ToolId;
use camera_screen::{PanelState, ShutterVisibility};
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

/// Светлый текст или значок на тёмном стекле карточки (заголовок — белый α ≈ 0,6, риски шкалы — α 0,5…0,8).
/// Тёмное стекло карточки имеет яркость ≈ 0,15, поэтому порог 0,35 отделяет рисунок от фона.
fn light_on_dark_glass(pixel: [f32; 3]) -> bool {
    pixel.iter().all(|channel| *channel > 0.35)
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
/// Высота экрана в pt: низ экрана — точка привязки шторки и карточки (`bottom_anchored`).
fn screen_height_pt() -> f64 {
    f64::from(HEIGHT_PX) / f64::from(PX_PER_PT)
}

/// Верх карточки инструмента, px. `[derived]` T = высота экрана − отступ 8,2 − высота 183,1 (pt) = 791,4 pt.
fn card_top_px() -> f32 {
    ((screen_height_pt() - 8.2 - 183.1) * f64::from(PX_PER_PT)) as f32
}

/// Окно варианта `index` из `count` в ряду карточки: центр x = 220 + (index − (count−1)/2) · 75,7 pt, центр y = T+118 pt,
/// половина зоны 30 pt по x (шаг 75,7 не залезает в соседа) и 20 pt по y. `[derived]` из `CardLayout`.
fn card_option_window(index: usize, count: usize) -> Window {
    let scale = f64::from(PX_PER_PT);
    let center_x = (220.0 + (index as f64 - (count as f64 - 1.0) / 2.0) * 75.7) * scale;
    let center_y = f64::from(card_top_px()) + 118.0 * scale;
    (
        (center_x - 30.0 * scale) as u32,
        (center_y - 20.0 * scale) as u32,
        (center_x + 30.0 * scale) as u32,
        (center_y + 20.0 * scale) as u32,
    )
}

/// Окно значка вспышки в полосе над кадром.
const FLASH_ICON_WINDOW: Window = (370, 285, 460, 360);
/// Окно шкалы EV в карточке Exposure: риски T+116…T+129 pt (T — верх карточки 791,4 pt) = 2227…2259 px, с запасом.
const EXPOSURE_SCALE_WINDOW: Window = (240, 2190, 680, 2290);
/// Узкое окно маркера шкалы (жёлтый 2 × 26 pt по центру x = 540 px, середина рисок y ≈ 2243 px).
const EXPOSURE_MARKER_WINDOW: Window = (528, 2200, 552, 2290);
/// Окно крупного значения карточки («1.0»): центр x = 540 px, y = T+77 pt = 868,4 pt = 2131 px, цифры 20,7 pt = 51 px.
const CARD_VALUE_WINDOW: Window = (440, 2095, 640, 2170);
/// Окно заголовка карточки («TIMER»): центр y = T+30 pt = 821,4 pt = 2016 px, заглавные 10,3 pt = 25 px.
const CARD_TITLE_WINDOW: Window = (440, 1995, 640, 2040);
/// Окно левой кнопки карточки «×»/«‹»: центр (49; T+43) pt = (120; 2048) px, диск Ø 36 pt = 88 px.
const CARD_LEFT_BUTTON_WINDOW: Window = (90, 2018, 150, 2078);
/// Окно правой кнопки «↻» (только Exposure): центр (391; T+44) pt = (960; 2050) px.
const CARD_RIGHT_BUTTON_WINDOW: Window = (930, 2020, 990, 2080);
/// Окно «ручки» шторки: центр x = 540 px, y = (верх шторки 510,1 + 8,9 + 1,85) pt = 1278 px; 37,6 × 3,7 pt.
const SHEET_HANDLE_WINDOW: Window = (500, 1272, 580, 1284);
/// Окно подписи значения EV над значком «солнце» в полосе над кадром.
const EXPOSURE_STRIP_WINDOW: Window = (620, 285, 720, 410);
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
        // Кадр 4:3 = 186,0…772,7 pt (159,3…746,0 эталона + сдвиг 26,67 к нашему экрану) показывает «фото»; вне кадра —
        // чёрная маска: над кадром, под кадром и по краям полос.
        let frame_pt = driver
            .screen()
            .state()
            .layout()
            .expect("раскладка")
            .frame_rect(AspectRatio::FourByThree);
        assert!(
            (frame_pt.min_y() - 186.0).abs() < 0.05 && (frame_pt.max_y() - 772.67).abs() < 0.05,
            "кадр 4:3 по высоте: {}…{} pt",
            frame_pt.min_y(),
            frame_pt.max_y()
        );
        let (_, top, _, bottom) = frame_rect_px(&driver, AspectRatio::FourByThree);
        // Край кадра на самом деле нарисован там, где его считает раскладка: фото на 3 px внутри, маска на 3 px снаружи.
        assert!(snapshot.luma(700, top + 3) > 0.2, "первая строка кадра — фото");
        assert!(near_black(snapshot.at(700, top - 3)), "строка над кадром — маска");
        assert!(snapshot.luma(700, bottom - 3) > 0.2, "последняя строка кадра — фото");
        assert!(near_black(snapshot.at(3, bottom + 3)), "строка под кадром — маска");
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
        assert_sheet_has_handle_and_black_item_circles(&rest, &snapshot);
        check_golden("controls_menu", &snapshot);
    });
}

/// Новая шторка (iOS 27): «ручка» 37,6 × 3,7 pt белая α 0,75 у верха и чёрные круги пунктов поверх стекла.
/// Ручка — светлая полоска там, где в покое фото; круг пункта заметно темнее стекла рядом с ним (чёрный α 0,5).
fn assert_sheet_has_handle_and_black_item_circles(rest: &Snapshot, snapshot: &Snapshot) {
    let scale = f64::from(PX_PER_PT);
    let handle_pixels = snapshot.count_in(SHEET_HANDLE_WINDOW, |pixel| pixel.iter().all(|channel| *channel > 0.7));
    assert!(handle_pixels > 150, "«ручка» шторки: {handle_pixels} светлых px");
    let handle_in_rest = rest.count_in(SHEET_HANDLE_WINDOW, |pixel| pixel.iter().all(|channel| *channel > 0.7));
    assert_eq!(
        handle_in_rest, 0,
        "в покое ручки нет, значит светлая полоска принадлежит шторке"
    );
    // Верх шторки = высота экрана − 8,6 − 464 pt; центр первого ряда пунктов на 83 pt ниже, колонки 91 / 220 / 349 pt.
    let sheet_top_pt = screen_height_pt() - 8.6 - 464.0;
    let circle_y = ((sheet_top_pt + 83.0) * scale) as u32;
    // Точка внутри круга колонки 2 над значком (радиус круга 34 pt = 83 px, значок ±14 pt): 55 px выше центра.
    let inside_circle = snapshot.luma(540, circle_y - 55);
    // Стекло между колонками 1 и 2 (x = 155,5 pt) на той же высоте: круги Ø 68 pt по 34 pt в каждую сторону не доходят.
    let glass_next_to_it = snapshot.luma((155.5 * scale) as u32, circle_y - 55);
    assert!(
        inside_circle < glass_next_to_it * 0.75,
        "круг пункта чёрный поверх стекла: внутри {inside_circle}, стекло рядом {glass_next_to_it}"
    );
}

#[test]
fn timer_submenu() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let driver = states::timer_submenu();
        let snapshot = render(rig, &driver);
        assert_eq!(driver.screen().state().panel(), PanelState::Submenu(ToolId::Timer));
        assert_eq!(
            driver.screen().state().shutter_visibility(),
            ShutterVisibility::RaisedAboveCard
        );
        // Карточка TIMER у низа экрана: заголовок и крупное значение «Off» — светлый текст по центру.
        assert!(
            snapshot.count_in(CARD_TITLE_WINDOW, light_on_dark_glass) > 150,
            "заголовок TIMER"
        );
        assert!(
            snapshot.count_in(CARD_VALUE_WINDOW, near_white) > 150,
            "крупное значение «Off»"
        );
        // Левая кнопка — «‹» назад в меню (светлый шеврон на тёмном диске); правой кнопки у Timer нет.
        assert!(
            snapshot.count_in(CARD_LEFT_BUTTON_WINDOW, near_white) > 30,
            "кнопка «‹»"
        );
        assert_eq!(
            snapshot.count_in(CARD_RIGHT_BUTTON_WINDOW, near_white),
            0,
            "у Timer нет кнопки «↻»"
        );
        // Ряд вариантов Off / 3 / 5 / 10: выбран Off (значок timer) — жёлтый только в его зоне.
        assert!(
            snapshot.count_in(card_option_window(0, 4), focus_yellow) > 40,
            "выбранный вариант жёлтый"
        );
        for index in 1..4 {
            assert_eq!(
                snapshot.count_in(card_option_window(index, 4), focus_yellow),
                0,
                "вариант {index} не выбран и не жёлтый"
            );
            assert!(
                snapshot.count_in(card_option_window(index, 4), near_white) > 40,
                "вариант {index} читается белым"
            );
        }
        assert_raised_shutter_is_visible_above_the_card(&driver, &rest, &snapshot);
        check_golden("timer_submenu", &snapshot);
    });
}

/// Ряд затвора поднят на 100 pt, затвор виден (белый диск) и целиком выше верха карточки; на старом месте затвора
/// теперь стекло карточки, а не белый диск.
fn assert_raised_shutter_is_visible_above_the_card(driver: &ScreenDriver, rest: &Snapshot, snapshot: &Snapshot) {
    let (rest_x, rest_y) = shutter_px(&ScreenDriver::new());
    let (raised_x, raised_y) = shutter_px(driver);
    let raise_px = rest_y as f32 - raised_y as f32;
    assert!(
        (raise_px - 100.0 * PX_PER_PT).abs() < 3.0,
        "затвор поднят на 100 pt = {} px, поднят на {raise_px} px",
        100.0 * PX_PER_PT
    );
    assert_eq!(raised_x, rest_x, "поднятый затвор по x не сдвинулся");
    assert!(
        near_white(snapshot.at(raised_x, raised_y)),
        "диск поднятого затвора белый"
    );
    let ring_bottom = raised_y as f32 + SHUTTER_DIAMETER_PX / 2.0;
    assert!(
        ring_bottom < card_top_px(),
        "затвор под ПОДНЯТЫМ положением не заходит в карточку: низ кольца {ring_bottom}, верх карточки {}",
        card_top_px()
    );
    assert!(
        near_white(rest.at(rest_x, rest_y)),
        "в покое белый диск на старом месте"
    );
    assert!(
        !near_white(snapshot.at(rest_x, rest_y)),
        "на старом месте затвора теперь карточка, не диск"
    );
}

#[test]
fn exposure_panel_plus_one_ev() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let zero_driver = states::exposure_panel(0.0);
        let zero = render(rig, &zero_driver);
        let driver = states::exposure_panel(1.0);
        let snapshot = render(rig, &driver);
        assert_eq!(driver.screen().state().panel(), PanelState::ExposurePanel);
        assert_eq!(
            driver.screen().state().shutter_visibility(),
            ShutterVisibility::RaisedAboveCard
        );
        assert_eq!(
            driver.screen().state().exposure().steps(),
            3,
            "+1 EV = 3 шага по 1/3 EV"
        );
        // Шкала едет под значением: при +1/3 EV риски сдвинуты на шаг (20 pt) относительно жёлтого маркера.
        // При +1 EV (ровно 3 шага) рисунок рисок совпадает с нулевым — целые деления стоят у маркера.
        let one_third = render(rig, &states::exposure_panel(1.0 / 3.0));
        let scale = one_third.mean_difference_in(&zero, EXPOSURE_SCALE_WINDOW);
        // Окно шире самих рисок, поэтому средняя разница мала (≈ 0,004); неподвижная шкала даёт ровно 0.
        assert!(scale > 0.002, "деления шкалы сдвинулись на шаг: {scale}");
        // Затухание к краям: дальше ±6 шагов (6 · 20 pt = 295 px от маркера) рисок нет совсем; окно — левее x = 230 px.
        let far_left: Window = (60, 2200, 230, 2290);
        assert_eq!(
            snapshot.count_in(far_left, light_on_dark_glass),
            0,
            "за краем затухания (дальше ±6 шагов) рисок нет"
        );
        assert!(
            snapshot.count_in(EXPOSURE_SCALE_WINDOW, light_on_dark_glass) > 300,
            "риски шкалы видны вокруг маркера"
        );
        for (name, frame) in [("0", &zero), ("+1", &snapshot)] {
            assert!(
                frame.count_in(EXPOSURE_MARKER_WINDOW, focus_yellow) > 100,
                "жёлтый маркер шкалы при {name} EV"
            );
        }
        // Крупное значение карточки: «0.0» → «1.0» (другие цифры — другие пиксели), в обоих случаях светлый текст.
        assert!(
            zero.count_in(CARD_VALUE_WINDOW, near_white) > 150,
            "значение «0.0» в карточке"
        );
        assert!(
            snapshot.count_in(CARD_VALUE_WINDOW, near_white) > 150,
            "значение «1.0» в карточке"
        );
        let value = snapshot.mean_difference_in(&zero, CARD_VALUE_WINDOW);
        assert!(
            value > 0.005,
            "значение в карточке изменилось с «0.0» на «1.0»: {value}"
        );
        // Верхний ряд: два ряда текста «1.0 / EV» — жёлтые при EV ≠ 0, белые при 0 (видео A «0.0 EV»).
        let label = snapshot.mean_difference_in(&zero, EXPOSURE_STRIP_WINDOW);
        assert!(label > 0.005, "подпись «1.0 EV» над кадром: {label}");
        assert!(
            snapshot.count_in(EXPOSURE_STRIP_WINDOW, focus_yellow) > 200,
            "«1.0 EV» жёлтое"
        );
        assert_eq!(
            zero.count_in(EXPOSURE_STRIP_WINDOW, focus_yellow),
            0,
            "«0.0 EV» не жёлтое"
        );
        assert!(zero.count_in(EXPOSURE_STRIP_WINDOW, near_white) > 200, "«0.0 EV» белое");
        // Кнопки карточки: «×» слева (со значка — закрыть) и «↻» справа.
        assert!(
            snapshot.count_in(CARD_LEFT_BUTTON_WINDOW, near_white) > 30,
            "кнопка «×»"
        );
        assert!(
            snapshot.count_in(CARD_RIGHT_BUTTON_WINDOW, near_white) > 30,
            "кнопка «↻»"
        );
        assert_card_reaches_the_bottom_and_the_shutter_rides_above_it(&driver);
        assert_raised_shutter_is_visible_above_the_card(&driver, &rest, &snapshot);
        check_golden("exposure_plus1", &snapshot);
    });
}

/// Карточка Exposure — рамка 421 × 183,1 pt с отступом 8,2 pt от низа экрана и от боков по 9,5 pt (`CardLayout`).
fn assert_card_reaches_the_bottom_and_the_shutter_rides_above_it(driver: &ScreenDriver) {
    let (card_x0, card_y0, card_x1, card_y1) = driver.glass_panel_rect_px().expect("стекло карточки в списке");
    let scale = PX_PER_PT;
    assert!(
        (card_y0 - card_top_px()).abs() < 2.0,
        "верх карточки {card_y0}, ждали {}",
        card_top_px()
    );
    let bottom_gap_pt = (HEIGHT_PX as f32 - card_y1) / scale;
    assert!(
        (bottom_gap_pt - 8.2).abs() < 0.2,
        "отступ карточки от низа экрана {bottom_gap_pt} pt, ждали 8,2"
    );
    assert!(
        (card_x0 / scale - 9.5).abs() < 0.2,
        "левый отступ карточки {} pt, ждали 9,5",
        card_x0 / scale
    );
    assert!(
        ((WIDTH_PX as f32 - card_x1) / scale - 9.5).abs() < 0.2,
        "правый отступ карточки {} pt, ждали 9,5",
        (WIDTH_PX as f32 - card_x1) / scale
    );
    // Затвор едет над карточкой: его кольцо кончается выше верха стекла (не «под кольцом», как в старой панели).
    let (_, raised_y) = shutter_px(driver);
    let ring_bottom = raised_y as f32 + SHUTTER_DIAMETER_PX / 2.0;
    assert!(
        ring_bottom < card_y0,
        "кольцо затвора (низ {ring_bottom}) заходит в карточку (верх {card_y0})"
    );
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
fn rest_without_flash() {
    with_rig(|rig| {
        let rest = states::rest();
        let driver = states::rest_without_flash();
        // Со вспышкой значок есть, без неё — его нет в списке рисования, остальной ряд на месте.
        assert!(
            rest.icon_center_px(IconId::FlashOff).is_some(),
            "в эталоне со вспышкой значок есть"
        );
        assert!(
            driver.icon_center_px(IconId::FlashOff).is_none(),
            "без вспышки значка нет"
        );
        assert!(
            driver.icon_center_px(IconId::FlashOn).is_none(),
            "без вспышки значка нет"
        );
        let snapshot = render(rig, &driver);
        check_golden("rest_without_flash", &snapshot);
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
