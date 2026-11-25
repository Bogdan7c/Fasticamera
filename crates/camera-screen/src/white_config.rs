//! Конфиг панели белого: кнопки света, шаги шкал, правила показа чисел (white-balance 02, решения WB22–WB34).
//!
//! Новая кнопка света — строка конфига, а не код. Значения кнопок обязаны лежать в пределах движка; экран
//! пределов при создании не знает (их присылает движок фактами), поэтому сверка с ними —
//! [`WhiteScreenConfig::check_lights_in_range`], её вызывает приложение, когда пределы стали известны.

use camera_contract::control::{ColorTemperature, InvalidControlValue, Tint, WhiteBalanceSetting};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use ui_draw::icons::IconId;
use ui_draw::text::{Figures, FontFace, TextError, TextStyle, layout_line};

use crate::white_facts::WhiteRange;
use crate::white_row::SOURCE_BUTTON_COUNT;

/// Номер кнопки света в списке конфига (кнопка «Телефон» сюда не входит: она не из списка).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WhiteLightIndex(pub usize);

/// Значок кнопки света: закрытый набор, чтобы конфиг не мог сослаться на несуществующую картинку.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteLightIcon {
    Sun,
    Cloud,
    Tree,
    Lightbulb,
    House,
    DeviceMobile,
}

impl WhiteLightIcon {
    /// Значок в атласе иконок.
    pub const fn icon_id(self) -> IconId {
        match self {
            WhiteLightIcon::Sun => IconId::Sun,
            WhiteLightIcon::Cloud => IconId::Cloud,
            WhiteLightIcon::Tree => IconId::Tree,
            WhiteLightIcon::Lightbulb => IconId::Lightbulb,
            WhiteLightIcon::House => IconId::House,
            WhiteLightIcon::DeviceMobile => IconId::DeviceMobile,
        }
    }
}

/// Одна кнопка света: имя, значок и белое, которое она ставит.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhiteLight {
    /// Подпись под значком (английская, коротко): «Sun», «Lamp».
    pub label: String,
    pub icon: WhiteLightIcon,
    /// Цветовая температура, К.
    pub kelvin: f64,
    /// Оттенок, Duv (плюс — к зелёному свету, картинка уходит в пурпур).
    pub duv: f64,
}

impl WhiteLight {
    /// Белое, которое ставит кнопка. Ошибка — число не конечное или не положительное.
    pub fn setting(&self) -> Result<WhiteBalanceSetting, InvalidControlValue> {
        Ok(WhiteBalanceSetting {
            temperature: ColorTemperature::from_kelvin(self.kelvin)?,
            tint: Tint::from_duv(self.duv)?,
        })
    }

    fn named(label: &str, icon: WhiteLightIcon, kelvin: f64, duv: f64) -> Self {
        Self {
            label: label.to_owned(),
            icon,
            kelvin,
            duv,
        }
    }
}

/// Первый символ строки, которого нет в атласе хотя бы одного шрифта или набора цифр: такую подпись нельзя
/// верстать (кадр падает с `UnsupportedCharacter`), поэтому конфиг с ней не принимается при старте.
pub(crate) fn first_character_outside_atlas(text: &str) -> Option<char> {
    let mut found = None;
    for face in FontFace::ALL {
        for figures in [Figures::Proportional, Figures::Tabular] {
            let style = TextStyle {
                face,
                size_px: 1.0,
                tracking_em: 0.0,
                figures,
            };
            if let Err(TextError::UnsupportedCharacter { ch, .. }) = layout_line(text, &style) {
                found.get_or_insert(ch);
            }
        }
    }
    found
}

/// Ошибка конфига панели белого. Проверка собирает ВСЕ ошибки сразу ([`WhiteScreenConfig::all_errors`]).
#[derive(Debug, Clone, PartialEq, Error)]
pub enum WhiteConfigError {
    #[error("список кнопок света пуст")]
    NoLights,
    #[error(
        "кнопок света {count}, без прокрутки помещается {max_without_scroll} (кнопка «Телефон» занимает одно место)"
    )]
    TooManyLights { count: usize, max_without_scroll: usize },
    #[error("у кнопки света пустое имя (номер {index})")]
    EmptyLabel { index: usize },
    #[error("имя кнопки света «{label}» повторяется")]
    DuplicateLabel { label: String },
    #[error("подпись кнопки света «{label}»: символ «{character}» вне атласа шрифта (кадр с ней не нарисовать)")]
    LabelNotInAtlas { label: String, character: char },
    #[error("кнопка света «{label}»: число не принимается ({reason})")]
    InvalidLight { label: String, reason: InvalidControlValue },
    #[error("кнопка света «{label}» вне пределов движка")]
    LightOutOfRange { label: String },
    #[error("значение {field} должно быть конечным и > 0, получено {value}")]
    NotPositive { field: &'static str, value: f64 },
    #[error("значение {field} должно быть не меньше 1, получено {value}")]
    ZeroPeriod { field: &'static str, value: u32 },
}

/// С какой стороны от затвора стоит кнопка белого (WB35).
///
/// Разбор строгий: неизвестное слово отвергается ошибкой с именем настройки, а не молча берёт умолчание.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WhiteButtonSide {
    /// Справа от затвора, между ним и кнопкой меню.
    #[default]
    Right,
    /// Слева от затвора (зеркально).
    Left,
}

impl<'de> Deserialize<'de> for WhiteButtonSide {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let word = String::deserialize(deserializer)?;
        match word.as_str() {
            "right" => Ok(WhiteButtonSide::Right),
            "left" => Ok(WhiteButtonSide::Left),
            other => Err(serde::de::Error::custom(format!(
                "camera_screen.screen.white.button_side: неизвестное значение «{other}», нужно «right» или «left»"
            ))),
        }
    }
}

/// Конфиг панели белого.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WhiteScreenConfig {
    /// Сторона кнопки белого от затвора (WB35): справа (умолчание) или слева.
    pub button_side: WhiteButtonSide,
    /// Кнопки света слева направо после кнопок «Auto» и «Phone».
    pub lights: Vec<WhiteLight>,
    /// Сколько кнопок видно без прокрутки вместе с кнопками-источниками («Auto» и «Phone»). `[owner]` 7 (WB48) =
    /// «Auto» + «Phone» + 5 света: семь кнопок по 60 pt = 420 pt помещаются в ширину карточки.
    pub max_buttons_without_scroll: usize,
    /// Шаг рисок шкалы Кельвинов в майредах. `[doc]` 5 майред — заметный глазу сдвиг белого, ровно по нему шагает шкала.
    pub mired_step: f64,
    /// Каждая какая риска шкалы Кельвинов крупная. `[estimate]` 5-я: подпись на каждые 25 майред.
    pub mired_major_every: u32,
    /// Шаг рисок шкалы оттенка в единицах оттенка (Duv × `tint_units_per_duv`). `[estimate]` 5.
    pub tint_step_units: f64,
    /// Каждая какая риска шкалы оттенка крупная. `[estimate]` 5-я.
    pub tint_major_every: u32,
    /// Округление показа Кельвинов, К. Само белое не округляется: последняя цифра иначе дрожит без смысла.
    pub display_step_kelvin: f64,
    /// Во сколько раз Duv умножается для показа оттенка (0,005 Duv → 5). `[estimate]` 1000: привычные целые числа.
    pub tint_units_per_duv: f64,
    /// Допуск «кнопка света подсвечена», майреды. `[estimate]` 0,5: ниже шага шкалы (5) и ниже округления показа,
    /// то есть ловит только то же белое с поправкой на округление в движке, а не «примерно похожее».
    pub light_match_tolerance_mired: f64,
    /// Допуск «кнопка света подсвечена» по оттенку, Duv. `[estimate]` 0,0005 = пол-единицы показа оттенка.
    pub light_match_tolerance_duv: f64,
}

impl Default for WhiteScreenConfig {
    fn default() -> Self {
        Self {
            button_side: WhiteButtonSide::default(),
            lights: vec![
                WhiteLight::named("Sun", WhiteLightIcon::Sun, 5_600.0, 0.003),
                WhiteLight::named("Cloud", WhiteLightIcon::Cloud, 6_500.0, 0.003),
                WhiteLight::named("Shade", WhiteLightIcon::Tree, 7_500.0, 0.003),
                WhiteLight::named("Lamp", WhiteLightIcon::Lightbulb, 3_200.0, 0.0),
                WhiteLight::named("Home", WhiteLightIcon::House, 2_700.0, 0.0),
            ],
            max_buttons_without_scroll: 7,
            mired_step: 5.0,
            mired_major_every: 5,
            tint_step_units: 5.0,
            tint_major_every: 5,
            display_step_kelvin: 10.0,
            tint_units_per_duv: 1_000.0,
            light_match_tolerance_mired: 0.5,
            light_match_tolerance_duv: 0.0005,
        }
    }
}

impl WhiteScreenConfig {
    /// Все ошибки конфига (пусто — конфиг годен).
    pub fn all_errors(&self) -> Vec<WhiteConfigError> {
        let mut errors = Vec::new();
        self.collect_light_list_errors(&mut errors);
        self.collect_number_errors(&mut errors);
        errors
    }

    /// Первая ошибка конфига (для общей проверки `ScreenConfig::validate`).
    pub fn validate(&self) -> Result<(), WhiteConfigError> {
        self.all_errors().into_iter().next().map_or(Ok(()), Err)
    }

    /// Сверка кнопок света с пределами движка: приложение зовёт её, когда пределы стали известны. Ошибка называет
    /// первую кнопку вне пределов.
    pub fn check_lights_in_range(&self, range: &WhiteRange) -> Result<(), WhiteConfigError> {
        for light in &self.lights {
            // Число, не годное само по себе, называет `validate`; здесь оно просто «не в пределах».
            let in_range = light.setting().is_ok_and(|white| range.contains(white));
            if !in_range {
                return Err(WhiteConfigError::LightOutOfRange {
                    label: light.label.clone(),
                });
            }
        }
        Ok(())
    }

    /// Максимум кнопок света без прокрутки: [`SOURCE_BUTTON_COUNT`] места занимают «Auto» и «Phone».
    pub fn max_lights_without_scroll(&self) -> usize {
        self.max_buttons_without_scroll.saturating_sub(SOURCE_BUTTON_COUNT)
    }

    fn collect_light_list_errors(&self, errors: &mut Vec<WhiteConfigError>) {
        if self.lights.is_empty() {
            errors.push(WhiteConfigError::NoLights);
        }
        let max_without_scroll = self.max_lights_without_scroll();
        if self.lights.len() > max_without_scroll {
            errors.push(WhiteConfigError::TooManyLights {
                count: self.lights.len(),
                max_without_scroll,
            });
        }
        for (index, light) in self.lights.iter().enumerate() {
            if light.label.trim().is_empty() {
                errors.push(WhiteConfigError::EmptyLabel { index });
            } else if self.lights[..index].iter().any(|earlier| earlier.label == light.label) {
                errors.push(WhiteConfigError::DuplicateLabel {
                    label: light.label.clone(),
                });
            }
            if let Some(character) = first_character_outside_atlas(&light.label) {
                errors.push(WhiteConfigError::LabelNotInAtlas {
                    label: light.label.clone(),
                    character,
                });
            }
            if let Err(reason) = light.setting() {
                errors.push(WhiteConfigError::InvalidLight {
                    label: light.label.clone(),
                    reason,
                });
            }
        }
    }

    fn collect_number_errors(&self, errors: &mut Vec<WhiteConfigError>) {
        let positive_numbers = [
            ("mired_step", self.mired_step),
            ("tint_step_units", self.tint_step_units),
            ("display_step_kelvin", self.display_step_kelvin),
            ("tint_units_per_duv", self.tint_units_per_duv),
            ("light_match_tolerance_mired", self.light_match_tolerance_mired),
            ("light_match_tolerance_duv", self.light_match_tolerance_duv),
        ];
        for (field, value) in positive_numbers {
            if !value.is_finite() || value <= 0.0 {
                errors.push(WhiteConfigError::NotPositive { field, value });
            }
        }
        for (field, value) in [
            ("mired_major_every", self.mired_major_every),
            ("tint_major_every", self.tint_major_every),
        ] {
            if value == 0 {
                errors.push(WhiteConfigError::ZeroPeriod { field, value });
            }
        }
        if self.max_buttons_without_scroll == 0 {
            errors.push(WhiteConfigError::ZeroPeriod {
                field: "max_buttons_without_scroll",
                value: 0,
            });
        }
    }
}
