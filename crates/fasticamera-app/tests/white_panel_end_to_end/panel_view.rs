//! Что стенд панели белого видит на кадре экрана: тексты (по равенству прогона с раскладкой нужной строки ролью, как
//! `texts_of`/`run_of` в `camera-screen/src/screen/test_rig.rs`), цвета и положения. Смысл строки определяет не
//! догадка, а то, что экран раскладывает именно эту строку именно этой ролью текста.

use camera_contract::control::{ColorTemperature, Tint};
use camera_screen::white_config::WhiteScreenConfig;
use camera_screen::white_display::{kelvin_text, tint_text};
use ui_draw::color::DisplayColor;
use ui_draw::draw_list::{DrawItem, DrawList, TextItem};
use ui_draw::text::TextRun;
use ui_kit::tokens::{TextRoleKind, UiKitTokens};
use ui_kit::view::{LabelPainter, PxPerPt};

/// Пикселей на pt: 1 pt = ширина поверхности (1080 px) / 440 pt эталона.
const PX_PER_PT: f32 = 1080.0 / 440.0;
/// Ниже этой прозрачности текст «не виден» (хвост пружины оставляет у ушедшего ~0,001).
const VISIBLE_OPACITY: f32 = 0.05;

/// Прогон строки `text` ролью `role`, как его раскладывает экран.
pub fn run_of(text: &str, role: TextRoleKind) -> TextRun {
    let tokens = UiKitTokens::default();
    LabelPainter::new(&tokens.typography, PxPerPt::new(PX_PER_PT).expect("масштаб"))
        .layout(text, role)
        .expect("строка раскладывается")
}

/// Все нарисованные экземпляры строки этой ролью, в том числе почти прозрачные (мерцание ждёт и в провале).
pub fn text_items_any_opacity(list: &DrawList, text: &str, role: TextRoleKind) -> Vec<TextItem> {
    let expected = run_of(text, role);
    list.leaf_items()
        .filter_map(|item| match item {
            DrawItem::Text(drawn) if drawn.run == expected => Some(drawn.clone()),
            _ => None,
        })
        .collect()
}

/// То же, но только заметные глазу экземпляры.
pub fn text_items(list: &DrawList, text: &str, role: TextRoleKind) -> Vec<TextItem> {
    text_items_any_opacity(list, text, role)
        .into_iter()
        .filter(|item| item.opacity.get() > VISIBLE_OPACITY)
        .collect()
}

/// Цвета экрана по умолчанию: белый текст и жёлтый «ручного».
pub fn colors() -> (DisplayColor, DisplayColor) {
    let tokens = UiKitTokens::default();
    (tokens.colors.text_primary, tokens.colors.focus_yellow)
}

/// Число Кельвинов на кнопке сверху: единственный экземпляр строки с числом `kelvin` ролью `StripValue`.
pub fn strip_number(list: &DrawList, kelvin: f64) -> Option<TextItem> {
    let text = kelvin_text(
        ColorTemperature::from_kelvin(kelvin).expect("температура"),
        &WhiteScreenConfig::default(),
    );
    let mut found = text_items(list, &text, TextRoleKind::StripValue);
    assert!(found.len() <= 1, "число «{text}» на кнопке нарисовано дважды");
    found.pop()
}

/// Число Кельвинов на кнопке с ЛЮБОЙ видимостью (нужна прозрачность мерцания, а не «виден ли»).
pub fn strip_number_any_opacity(list: &DrawList, kelvin: f64) -> Option<TextItem> {
    let text = kelvin_text(
        ColorTemperature::from_kelvin(kelvin).expect("температура"),
        &WhiteScreenConfig::default(),
    );
    text_items_any_opacity(list, &text, TextRoleKind::StripValue)
        .into_iter()
        .next()
}

/// Число оси Кельвинов на карточке («3140 K»), роль `CardValue`.
pub fn card_kelvin_number(list: &DrawList, kelvin: f64) -> Vec<TextItem> {
    let number = kelvin_text(
        ColorTemperature::from_kelvin(kelvin).expect("температура"),
        &WhiteScreenConfig::default(),
    );
    text_items(list, &format!("{number} K"), TextRoleKind::CardValue)
}

/// Число оси оттенка на карточке («Tint +16»), роль `CardValue`.
pub fn card_tint_number(list: &DrawList, duv: f64) -> Vec<TextItem> {
    let text = tint_text(Tint::from_duv(duv).expect("оттенок"), &WhiteScreenConfig::default());
    text_items(list, &format!("Tint {text}"), TextRoleKind::CardValue)
}

/// Подпись кнопки (роль `Caption`) в видимом виде: «Phone», «Sun», «≈ 3100».
pub fn caption(list: &DrawList, text: &str) -> Vec<TextItem> {
    text_items(list, text, TextRoleKind::Caption)
}
