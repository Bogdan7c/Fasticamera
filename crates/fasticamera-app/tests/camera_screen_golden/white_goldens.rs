//! Golden-кадры карточки белого (white-balance 02, пакет D6): настоящие касания → экран → список рисования →
//! настоящий рендер UI на GPU. Рядом с эталоном каждый тест проверяет пиксели независимо от него (цвет числа на кнопке,
//! карточка на месте, поворот в альбоме), поэтому тест падает при поломке даже после перегенерации эталонов.
//!
//! Мерцание ожидания зависит от времени (кадр без точной метки времени не воспроизвести бит-в-бит), поэтому эталона
//! «ожидание» нет: мерцание закрыто сквозным тестом `white_panel_end_to_end` (прозрачность меняется между кадрами).

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{HeldWhite, WhiteOrigin, WhiteSourceId};
use camera_contract::white_reading::WhiteReading;
use camera_screen::{CameraFacts, PanelState, ScreenAction, WhiteAxis, WhiteFacts, WhiteFollowFacts};
use shell_contract::orientation::DeviceOrientation;
use ui_core::geometry::Point;

use crate::driver::{PX_PER_PT, SETTLE_MS, ScreenDriver, live_facts};
use crate::golden_files::check_golden;
use crate::{Window, focus_yellow, light_on_dark_glass, render, with_rig};

/// Полуширина окна вокруг кнопки белого, px (круг Ø 44 pt = 108 px, число и «K» помещаются).
const BUTTON_WINDOW_HALF_PX: f32 = 60.0;

fn white_of(kelvin: f64, duv: f64) -> WhiteBalanceSetting {
    WhiteBalanceSetting {
        temperature: ColorTemperature::from_kelvin(kelvin).expect("температура"),
        tint: Tint::from_duv(duv).expect("оттенок"),
    }
}

/// Факты живой камеры с белым `held` и показанием телефона `reading`.
fn facts_with(held: HeldWhite, reading: Option<WhiteReading>) -> CameraFacts {
    CameraFacts {
        white: WhiteFacts {
            held,
            phone_reading: reading,
            ..WhiteFacts::STARTING
        },
        ..live_facts()
    }
}

/// Экран с этими фактами, карточка белого открыта тапом по кнопке.
fn driver_with_open_card(facts: CameraFacts) -> ScreenDriver {
    let mut driver = ScreenDriver::new();
    driver.act(ScreenAction::CameraFactsChanged(facts));
    driver.run(SETTLE_MS);
    open_card(&mut driver);
    driver
}

fn open_card(driver: &mut ScreenDriver) {
    let button = driver
        .screen()
        .white_touch_targets()
        .map(|targets| targets.button.center())
        .expect("раскладка экрана построена");
    driver.tap(button);
    driver.run(SETTLE_MS);
    assert_eq!(
        driver.screen().state().panel(),
        PanelState::Submenu(camera_screen::tools::ToolId::White),
        "карточка белого открылась тапом"
    );
}

/// Окно вокруг кнопки белого у затвора (px): там, где она стоит сейчас (поднята, если карточка открыта).
fn button_window(driver: &ScreenDriver) -> Window {
    let button = driver
        .screen()
        .white_touch_targets()
        .map(|targets| targets.button.center())
        .expect("раскладка экрана построена");
    let scale = f64::from(PX_PER_PT);
    window_around(Point::new(button.x * scale, button.y * scale))
}

fn window_around(center_px: Point) -> Window {
    let half = f64::from(BUTTON_WINDOW_HALF_PX);
    (
        (center_px.x - half).max(0.0) as u32,
        (center_px.y - half).max(0.0) as u32,
        (center_px.x + half) as u32,
        (center_px.y + half) as u32,
    )
}

fn source_held(kelvin: f64, duv: f64) -> HeldWhite {
    HeldWhite {
        setting: white_of(kelvin, duv),
        origin: WhiteOrigin::Source(WhiteSourceId::PHONE),
    }
}

fn manual_held(kelvin: f64, duv: f64) -> HeldWhite {
    HeldWhite {
        setting: white_of(kelvin, duv),
        origin: WhiteOrigin::Manual,
    }
}

fn measured(kelvin: f64) -> Option<WhiteReading> {
    Some(WhiteReading::Measured {
        white: white_of(kelvin, 0.0),
    })
}

/// Окно карточки целиком (нижняя четверть экрана): нужно только для сравнения кадров между собой.
const CARD_AREA: Window = (0, 1900, 1080, 2412);

#[test]
fn white_card_phone() {
    with_rig(|rig| {
        let driver = driver_with_open_card(facts_with(source_held(3_140.0, 0.0164), measured(3_100.0)));
        let snapshot = render(rig, &driver);
        check_golden("white_card_phone", &snapshot);

        // Телефон дал 3140 К: число на кнопке БЕЛОЕ (жёлтых пикселей в окне кнопки нет), светлые пиксели есть.
        let strip = button_window(&driver);
        assert_eq!(
            snapshot.count_in(strip, focus_yellow),
            0,
            "белое телефона — не жёлтое число"
        );
        assert!(
            snapshot.count_in(strip, light_on_dark_glass) > 30,
            "число на кнопке нарисовано"
        );
        // Карточка на месте: светлые знаки (кнопки, числа, риски) заполняют её низ.
        assert!(
            snapshot.count_in(CARD_AREA, light_on_dark_glass) > 2_000,
            "карточка нарисована"
        );
        // Оценка «≈ 3100» есть в списке рисования (её знак «≈» нарисован из атласа, кадр не упал).
        let estimate = camera_screen::white_display::phone_estimate_text(
            measured(3_100.0),
            &camera_screen::ScreenConfig::default().white,
        )
        .expect("оценка");
        assert!(estimate.starts_with('\u{2248}'));
    });
}

/// Факты живой камеры с белым `held`, показанием телефона и слежением `follow`.
fn facts_following(held: HeldWhite, reading: Option<WhiteReading>, follow: WhiteFollowFacts) -> CameraFacts {
    CameraFacts {
        white: WhiteFacts {
            held,
            phone_reading: reading,
            follow,
            ..WhiteFacts::STARTING
        },
        ..live_facts()
    }
}

/// Окно вокруг центра кнопки карточки (px); центр приходит в pt экрана.
fn row_button_window(center_pt: Point) -> Window {
    let scale = f64::from(PX_PER_PT);
    window_around(Point::new(center_pt.x * scale, center_pt.y * scale))
}

#[test]
fn white_card_auto() {
    with_rig(|rig| {
        // Белое взято у телефона (число совпало бы с «Phone»), слежение ВКЛЮЧЕНО: горит одна «Auto».
        let held = source_held(3_140.0, 0.0164);
        let driver = driver_with_open_card(facts_following(held, measured(3_100.0), WhiteFollowFacts::On));
        let snapshot = render(rig, &driver);
        check_golden("white_card_auto", &snapshot);
        let off = driver_with_open_card(facts_following(held, measured(3_100.0), WhiteFollowFacts::Off));
        let off_shot = render(rig, &off);

        let card = driver
            .screen()
            .white_touch_targets()
            .and_then(|targets| targets.card)
            .expect("карточка открыта");
        let auto = row_button_window(card.auto_button);
        let phone = row_button_window(card.phone_button);
        // «Auto» жёлтая только при слежении, «Phone» — только без него.
        assert!(
            snapshot.count_in(auto, focus_yellow) > 40,
            "«Auto» горит жёлтым при слежении"
        );
        assert_eq!(
            snapshot.count_in(phone, focus_yellow),
            0,
            "«Phone» при слежении не горит"
        );
        assert_eq!(off_shot.count_in(auto, focus_yellow), 0, "без слежения «Auto» не горит");
        assert!(
            off_shot.count_in(phone, focus_yellow) > 40,
            "без слежения «Phone» горит"
        );
        // Число у затвора белое и живое, под ним «AUTO» вместо «K»: светлые пиксели есть, жёлтых нет, а кнопка
        // отличается от той же кнопки без слежения (там под числом «K»).
        let button = button_window(&driver);
        assert_eq!(snapshot.count_in(button, focus_yellow), 0, "число при слежении белое");
        assert!(
            snapshot.count_in(button, light_on_dark_glass) > 30,
            "число на кнопке нарисовано"
        );
        assert!(
            snapshot.mean_difference_in(&off_shot, button) > 0.0,
            "подпись под числом сменилась («AUTO» вместо «K»)"
        );
    });
}

#[test]
fn white_card_manual() {
    with_rig(|rig| {
        let phone = driver_with_open_card(facts_with(source_held(5_600.0, 0.003), measured(3_100.0)));
        let phone_shot = render(rig, &phone);
        let driver = driver_with_open_card(facts_with(manual_held(5_600.0, 0.003), measured(3_100.0)));
        let snapshot = render(rig, &driver);
        check_golden("white_card_manual", &snapshot);

        // Ручное белое: число на кнопке жёлтое; то же белое от телефона — не жёлтое.
        let strip = button_window(&driver);
        assert!(snapshot.count_in(strip, focus_yellow) > 20, "ручное число жёлтое");
        assert_eq!(phone_shot.count_in(strip, focus_yellow), 0, "белое телефона не жёлтое");
        // Карточки одинаковой раскладки, различие — только происхождение (подсветка кнопки «Phone» и число сверху).
        assert!(snapshot.mean_difference_in(&phone_shot, CARD_AREA) > 0.0);
    });
}

#[test]
fn white_card_tint() {
    with_rig(|rig| {
        let facts = facts_with(manual_held(3_140.0, 0.0164), measured(3_100.0));
        let kelvin_axis = driver_with_open_card(facts.clone());
        let kelvin_shot = render(rig, &kelvin_axis);
        let mut driver = driver_with_open_card(facts);
        let tint_number = driver
            .screen()
            .white_touch_targets()
            .and_then(|targets| targets.card)
            .expect("карточка открыта")
            .tint_number;
        driver.tap(tint_number);
        driver.run(SETTLE_MS);
        assert_eq!(
            driver.screen().state().white().axis(),
            WhiteAxis::Tint,
            "тап по числу выбрал ось оттенка"
        );
        let snapshot = render(rig, &driver);
        check_golden("white_card_tint", &snapshot);

        // Шкала оттенка отличается от шкалы Кельвинов (другие подписи рисок и выбранное число).
        assert!(
            snapshot.mean_difference_in(&kelvin_shot, CARD_AREA) > 0.001,
            "оси рисуются по-разному"
        );
    });
}

#[test]
fn white_card_landscape_left() {
    with_rig(|rig| {
        let mut driver = ScreenDriver::new();
        driver.act(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
        driver.act(ScreenAction::CameraFactsChanged(facts_with(
            manual_held(3_140.0, 0.0164),
            measured(3_100.0),
        )));
        driver.run(SETTLE_MS);
        open_card(&mut driver);
        let snapshot = render(rig, &driver);
        check_golden("white_card_landscape_left", &snapshot);

        // В альбоме знаки карточки повёрнуты на 90° вместе с телефоном.
        let rotated = driver.list().leaf_items().any(|item| match item {
            ui_draw::draw_list::DrawItem::Text(text) => text.transform.b.abs() > 0.5,
            _ => false,
        });
        assert!(rotated, "тексты карточки повёрнуты в альбоме");
        let strip = button_window(&driver);
        assert!(
            snapshot.count_in(strip, focus_yellow) > 20,
            "ручное число жёлтое и в альбоме"
        );
    });
}
