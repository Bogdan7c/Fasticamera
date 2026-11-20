//! Golden-кадры зума и перехода между объективами (сессия 13): лента упоров, колесо, фронтальная камера без ленты,
//! удержанный размытый кадр и карточка переворота. Как и остальные golden экрана: кроме сверки с эталоном каждый тест
//! проверяет пиксели и намерения экрана независимо от него, поэтому тест падает при поломке даже после
//! перегенерации эталонов.
//!
//! Лента на 1× — это `rest_4x3` (в покое экран всегда показывает ленту «.6 / 1× / 2»), отдельного эталона у неё нет.

use camera_contract::ids::LensFacing;
use camera_screen::Effect;

use crate::driver::WIDTH_PX;
use crate::golden_files::check_golden;
use crate::{Window, focus_yellow, near_black, render, states, with_rig, zoom_states};

/// Лента упоров в нижней части кадра 4:3 (центр (540; 1808) px, выбранный упор — круг Ø≈90 px).
const STRIP_WINDOW: Window = (420, 1740, 680, 1880);
/// Круг выбранного упора ленты и полоса фона слева от него на той же высоте.
const STRIP_BUBBLE_POINT: (u32, u32) = (540, 1775);
const STRIP_BACKGROUND_POINT: (u32, u32) = (450, 1775);
/// Жёлтый маркер текущего значения на верху колеса: треугольник 3,3 × 8,7 pt ≈ 8 × 21 px (основание вверху, вершина
/// вниз, к центру диска), у середины колеса.
const WHEEL_MARKER_WINDOW: Window = (520, 1560, 560, 1600);
/// Верхняя и нижняя половины маркера (по высоте): у треугольника сверху пикселей заметно больше, чем снизу.
const WHEEL_MARKER_UPPER_HALF: Window = (520, 1570, 560, 1581);
const WHEEL_MARKER_LOWER_HALF: Window = (520, 1581, 560, 1592);
/// Шахматка в левом нижнем углу «фото»: самые контрастные пиксели картинки (размытие их гасит).
const CHECKER_WINDOW: Window = (0, 1000, 340, 1600);
/// Строка кадра на высоте верха шахматки: по ней меряется ширина карточки переворота.
const CARD_ROW_Y: u32 = 1000;

/// Пиксель светлой (≈ 0,85) или тёмной (≈ 0,13) клетки шахматки; после размытия все клетки — серая середина.
fn checker_extreme(pixel: [f32; 3]) -> bool {
    pixel.iter().all(|channel| *channel > 0.78) || pixel.iter().all(|channel| *channel < 0.2)
}

/// Последнее намерение объектива среди эффектов экрана.
fn last_lens_intent(effects: &[Effect]) -> Option<camera_contract::zoom::LensIntent> {
    effects.iter().rev().find_map(|effect| match effect {
        Effect::Lens(intent) => Some(*intent),
        _ => None,
    })
}

#[test]
fn strip_with_ultra_wide_selected() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let mut driver = zoom_states::strip_with_ultra_wide_selected();
        let snapshot = render(rig, &driver);

        // Экран просил у камеры именно ширик (0,59×) и сам показывает это значение.
        let intent = last_lens_intent(&driver.drain_effects()).expect("тап по упору публикует намерение объектива");
        assert_eq!(intent.facing, LensFacing::Back);
        assert!(
            (intent.ratio.value() - 0.59).abs() < 0.01,
            "намерение на упор ширика: {:?}",
            intent.ratio
        );
        assert!((driver.screen().state().zoom().ratio().value() - 0.59).abs() < 0.01);

        // Выбранный упор — жёлтая подпись на тёмном круге; в покое (1×) подпись на другом упоре, картинка ленты другая.
        assert!(
            snapshot.count_in(STRIP_WINDOW, focus_yellow) > 300,
            "жёлтая подпись выбранного упора"
        );
        let bubble = snapshot.luma(STRIP_BUBBLE_POINT.0, STRIP_BUBBLE_POINT.1);
        let background = snapshot.luma(STRIP_BACKGROUND_POINT.0, STRIP_BACKGROUND_POINT.1);
        assert!(
            bubble < background * 0.6,
            "тёмный круг выбранного упора: круг {bubble}, фон {background}"
        );
        assert!(
            snapshot.mean_difference_in(&rest, STRIP_WINDOW) > 0.01,
            "лента на 0,6× отличается от ленты на 1×"
        );
        check_golden("zoom_strip_ultra_wide", &snapshot);
    });
}

#[test]
fn wheel_open_at_one_point_seven() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let mut driver = zoom_states::wheel_open_at_one_point_seven();
        let snapshot = render(rig, &driver);

        assert!(
            driver.screen().state().zoom().is_wheel_open(),
            "палец лежит на колесе: оно открыто"
        );
        let intent = last_lens_intent(&driver.drain_effects()).expect("движение колеса публикует намерение");
        assert!(
            (intent.ratio.value() - 1.7).abs() < 0.01,
            "намерение ведётся пальцем: {:?}",
            intent.ratio
        );
        let marker_pixels = snapshot.count_in(WHEEL_MARKER_WINDOW, focus_yellow);
        assert!(
            (40..140).contains(&marker_pixels),
            "жёлтый треугольник текущего значения (≈ 67 px), а найдено {marker_pixels}"
        );
        // Именно треугольник вершиной вниз, а не прямоугольник: сверху пикселей заметно больше, чем снизу.
        let upper = snapshot.count_in(WHEEL_MARKER_UPPER_HALF, focus_yellow);
        let lower = snapshot.count_in(WHEEL_MARKER_LOWER_HALF, focus_yellow);
        assert!(
            lower > 0 && upper > lower + 10,
            "маркер сужается к центру диска: сверху {upper} px, снизу {lower} px"
        );
        assert_eq!(
            rest.count_in(WHEEL_MARKER_WINDOW, focus_yellow),
            0,
            "в покое колеса, а с ним и маркера нет"
        );
        check_golden("zoom_wheel_1_7", &snapshot);
    });
}

#[test]
fn front_camera_shows_no_strip() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let mut driver = zoom_states::front_camera_without_strip();
        let snapshot = render(rig, &driver);

        assert_eq!(driver.screen().state().zoom().facing(), LensFacing::Front);
        let intent = last_lens_intent(&driver.drain_effects()).expect("переворот публикует намерение объектива");
        assert_eq!(
            intent.facing,
            LensFacing::Front,
            "камеру просили перевернуть на фронтальную"
        );
        // Ленты нет: ни жёлтой подписи, ни тёмного круга, нижняя часть кадра — чистое «фото».
        assert_eq!(snapshot.count_in(STRIP_WINDOW, focus_yellow), 0);
        let bubble = snapshot.luma(STRIP_BUBBLE_POINT.0, STRIP_BUBBLE_POINT.1);
        let background = snapshot.luma(STRIP_BACKGROUND_POINT.0, STRIP_BACKGROUND_POINT.1);
        assert!(
            (bubble - background).abs() < 0.05,
            "под лентой одно «фото»: {bubble} против {background}"
        );
        assert!(
            rest.count_in(STRIP_WINDOW, focus_yellow) > 100,
            "в покое лента на месте"
        );
        // Кнопка переворота остаётся на экране.
        assert!(driver.icon_center_px(ui_draw::icons::IconId::FlipCamera).is_some());
        check_golden("zoom_front_no_strip", &snapshot);
    });
}

#[test]
fn held_frame_is_blurred_while_the_lens_switches() {
    with_rig(|rig| {
        let rest = render(rig, &states::rest());
        let driver = zoom_states::held_frame_while_switching_to_ultra_wide();
        let snapshot = render(rig, &driver);

        // Шахматка «фото» превращена размытием в серую рябь: крайних (светлых и тёмных клеток) пикселей почти не остаётся.
        let sharp = rest.count_in(CHECKER_WINDOW, checker_extreme);
        let blurred = snapshot.count_in(CHECKER_WINDOW, checker_extreme);
        assert!(sharp > 100_000, "в покое шахматка резкая: {sharp} крайних px");
        assert!(blurred < sharp / 10, "размытие гасит контраст: {blurred} из {sharp}");
        // Лента и остальной UI поверх размытия остаются чёткими.
        assert!(
            snapshot.count_in(STRIP_WINDOW, focus_yellow) > 300,
            "подпись упора поверх размытия"
        );
        check_golden("zoom_held_frame_blur", &snapshot);
    });
}

#[test]
fn flip_card_is_a_narrowed_blurred_frame_on_black() {
    with_rig(|rig| {
        let driver = zoom_states::flip_card_mid_rotation();
        let snapshot = render(rig, &driver);

        let width = WIDTH_PX;
        let lit: Vec<u32> = (0..width)
            .filter(|x| !near_black(snapshot.at(*x, CARD_ROW_Y)))
            .collect();
        let (left, right) = (
            *lit.first().expect("карточка видна"),
            *lit.last().expect("карточка видна"),
        );
        let card_width = right - left + 1;
        assert!(
            (width * 4 / 10..width * 9 / 10).contains(&card_width),
            "карточка уже кадра, но не вырождена: {card_width} из {width} px"
        );
        assert_eq!(lit.len() as u32, card_width, "карточка — сплошное окно без дыр");
        assert!(
            left.abs_diff(width - 1 - right) < 6,
            "карточка по центру: слева {left}, справа {}",
            width - 1 - right
        );
        assert!(
            near_black(snapshot.at(left / 2, CARD_ROW_Y)),
            "вне карточки чёрный фон, а не прежний кадр"
        );
        check_golden("zoom_flip_card", &snapshot);
    });
}
