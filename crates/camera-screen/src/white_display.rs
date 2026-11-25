//! Модель показа белого: чистые функции «значение → что написать и что подсветить». Рисования здесь нет
//! (виджеты берут готовые строки и флаги), конфиг приходит параметром.
//!
//! Показ белое не меняет: округление до 10 К и целые единицы оттенка касаются только текста.

use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
use camera_contract::white::{HeldWhite, WhiteOrigin, WhiteSourceId};
use camera_contract::white_reading::WhiteReading;

use crate::white_config::{WhiteLight, WhiteScreenConfig};
use crate::white_facts::{WhiteFollowFacts, WhiteTakeFacts, WhiteTakeState};

/// Подпись единицы рядом с числом Кельвинов.
pub const KELVIN_UNIT: &str = "K";

/// Подпись единицы под числом у затвора при слежении (WB49): вместо «K».
pub const AUTO_UNIT: &str = "AUTO";

/// Подпись кнопки «Auto» на карточке (английская, как все подписи UI).
pub const AUTO_LABEL: &str = "Auto";

/// Подпись кнопки источника «Телефон» на карточке (английская, как все подписи UI).
pub const PHONE_LABEL: &str = "Phone";

/// Подпись числа оттенка на карточке («Tint +16»): слово и пробел перед числом.
pub const TINT_LABEL_PREFIX: &str = "Tint ";

/// Знак «минус» из Unicode (U+2212): обычный дефис в числе читается хуже и короче.
const MINUS_SIGN: char = '\u{2212}';

/// Майреды (микрообратные кельвины): шкала шагает по ним, потому что глаз видит сдвиг белого ровно по майредам.
pub fn mired_of(kelvin: f64) -> f64 {
    ui_kit::value::mired_of_kelvin(kelvin)
}

/// Кельвины, округлённые для показа до `display_step_kelvin` (само белое не округляется).
pub fn displayed_kelvin(temperature: ColorTemperature, config: &WhiteScreenConfig) -> f64 {
    (temperature.kelvin() / config.display_step_kelvin).round() * config.display_step_kelvin
}

/// Число Кельвинов для кнопки сверху и карточки: «5600» (единицу «K» рисуют отдельно, [`KELVIN_UNIT`]).
pub fn kelvin_text(temperature: ColorTemperature, config: &WhiteScreenConfig) -> String {
    // Белое ограничено пределами движка (тысячи К), поэтому приведение к целому не обрезает значение.
    format!("{:.0}", displayed_kelvin(temperature, config))
}

/// Оттенок в целых единицах показа: Duv × `tint_units_per_duv`, плюс — к пурпурному.
pub fn tint_units(tint: Tint, config: &WhiteScreenConfig) -> i32 {
    (tint.duv() * config.tint_units_per_duv).round() as i32
}

/// Текст оттенка: целое со знаком («+5», «−12»), ноль — «0» без знака.
pub fn tint_text(tint: Tint, config: &WhiteScreenConfig) -> String {
    let units = tint_units(tint, config);
    match units {
        0 => "0".to_owned(),
        positive if positive > 0 => format!("+{positive}"),
        negative => format!("{MINUS_SIGN}{}", negative.unsigned_abs()),
    }
}

/// Знак «примерно» перед оценкой (решение владельца: «≈ 3100»). Знак лежит в запечённом атласе шрифта
/// (`ui-draw/assets/bake.json`, `charset.extra`); тест `every_character_of_the_white_panel_is_in_the_atlas` следит,
/// что он там остаётся: символ вне атласа роняет кадр целиком (`UnsupportedCharacter`).
pub const APPROX_SIGN: &str = "\u{2248}";

/// Текст оценки на кнопке «Телефон»: «≈ 3100» только когда источник измерил свет; иначе ничего (число, которое
/// не измерено на этом потоке, показанием не является).
pub fn phone_estimate_text(reading: Option<WhiteReading>, config: &WhiteScreenConfig) -> Option<String> {
    match reading? {
        WhiteReading::Measured { white } => Some(format!("{APPROX_SIGN} {}", kelvin_text(white.temperature, config))),
        WhiteReading::NotMeasuredYet | WhiteReading::Unavailable { .. } | WhiteReading::Failed { .. } => None,
    }
}

/// Подсвечена ли кнопка света: действующее белое равно её числу с малым допуском (майреды и Duv, константы в
/// конфиге). Происхождение не важно: рука, поставившая те же числа, тоже «выбрала» эту кнопку. Пока слежение
/// включено, не горит никакая: белое проезжает мимо чисел кнопок, и совпадение на миг было бы мельканием.
pub fn light_is_active(
    light: &WhiteLight,
    held: WhiteBalanceSetting,
    follow: WhiteFollowFacts,
    config: &WhiteScreenConfig,
) -> bool {
    if follow.is_on() {
        return false;
    }
    let mired_gap = (mired_of(light.kelvin) - mired_of(held.temperature.kelvin())).abs();
    let duv_gap = (light.duv - held.tint.duv()).abs();
    mired_gap <= config.light_match_tolerance_mired && duv_gap <= config.light_match_tolerance_duv
}

/// Подсвечена ли кнопка «Auto»: слежение включено (в том числе заморожено блокировкой, WB46).
pub fn auto_is_active(follow: WhiteFollowFacts) -> bool {
    follow.is_on()
}

/// Подсвечена ли кнопка «Телефон»: белое сейчас взято у телефона РАЗОВО. При слежении горит «Auto», а не она:
/// происхождение «телефон» у белого и так, но режим другой.
pub fn phone_is_active(held: HeldWhite, follow: WhiteFollowFacts) -> bool {
    !follow.is_on() && held.origin == WhiteOrigin::Source(WhiteSourceId::PHONE)
}

/// Цвет числа на кнопке сверху: «ручное» жёлтое, остальное белое (WB16). Пока слежение включено, число у затвора
/// белое и живое, даже если держимое происхождение ещё «ручное» (ответа источника на включение не было).
pub fn number_is_manual(held: HeldWhite, follow: WhiteFollowFacts) -> bool {
    !follow.is_on() && held.origin == WhiteOrigin::Manual
}

/// Подпись единицы под числом у затвора: «K» обычно, «AUTO» при слежении (WB49).
pub fn white_unit_label(follow: WhiteFollowFacts) -> &'static str {
    if follow.is_on() { AUTO_UNIT } else { KELVIN_UNIT }
}

/// Манера кнопки «Телефон» и кнопки белого у затвора.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteButtonManner {
    /// Обычный вид.
    Normal,
    /// Движок ждёт ответа источника: кнопка показывает «ожидание».
    Waiting,
}

/// Манера кнопки: мерцает ТОЛЬКО пока ждёт ответа на СВОЮ команду «взять». Заявки, которых человек не делал
/// (открытие камеры), не мерцают: без кадров камеры такая заявка висела бы, и число мигало бы бесконечно.
pub fn button_manner(take: Option<WhiteTakeFacts>) -> WhiteButtonManner {
    match take {
        Some(WhiteTakeFacts {
            state: WhiteTakeState::Waiting,
            ..
        }) => WhiteButtonManner::Waiting,
        _ => WhiteButtonManner::Normal,
    }
}
