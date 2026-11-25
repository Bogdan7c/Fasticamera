//! Проверки белого для шагов сценария `screen` (white-balance 02): рука поставила число кнопки, экран показывает то же,
//! что держит движок, долгое нажатие кончилось честным исходом.
//!
//! Что видит проверка — [`WhiteDelta`]: белое движка до и после шага, исход последней команды «взять» и то, что об
//! этом думает экран ([`WhiteView`]). Правила — чистые функции без ввода-вывода, поэтому проверяются без маршрутизатора.
//!
//! # Долгое нажатие и «не знаю»
//! Источник «телефон» вправе отказать: в темноте он честно не знает света. Это не провал шага — провал, если
//! исход **не различим**: команда не закрылась, при «взято» белое не стало источником, при отказе белое изменилось
//! или экран не отметил отказ (тряска и вибрация идут от счёта отказов).

use camera_contract::white::{HeldWhite, WhiteOrigin, WhiteSourceId};
use camera_screen::{WhiteTakeFacts, WhiteTakeState};

use crate::camera_screen_white_view::{WhiteView, kelvin_rounded};

/// Допуск сравнения Кельвинов руки с кнопкой, К: движок хранит то, что ему сказали, допуск — на преобразования
/// температуры туда и обратно.
const KELVIN_TOLERANCE: f64 = 1.0;
/// Допуск сравнения оттенка руки с кнопкой, Duv.
const DUV_TOLERANCE: f64 = 1e-4;

/// Белое за шаг.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteDelta {
    /// Белое движка до шага.
    pub held_before: HeldWhite,
    /// Белое движка сейчас.
    pub held_now: HeldWhite,
    /// Исход последней команды «взять» сейчас.
    pub take_now: Option<WhiteTakeFacts>,
    /// Номер команды «взять» до шага (нового исхода нет, пока номер тот же).
    pub take_serial_before: Option<u64>,
    /// Белое глазами экрана до шага.
    pub view_before: WhiteView,
    /// Белое глазами экрана сейчас.
    pub view_now: WhiteView,
}

impl WhiteDelta {
    /// Команда «взять» этого шага: исход, номер которого новее виденного до шага.
    fn new_take(&self) -> Option<WhiteTakeFacts> {
        self.take_now
            .filter(|take| self.take_serial_before.is_none_or(|before| take.serial > before))
    }
}

#[cfg(test)]
impl WhiteDelta {
    /// Шаг, на котором белое не менялось (дневной свет запасным происхождением, команд «взять» не было): для тестов
    /// правил, которые белого не касаются.
    pub(crate) fn quiet() -> Self {
        use camera_contract::control::{ColorTemperature, Tint, WhiteBalanceSetting};
        let held = HeldWhite {
            setting: WhiteBalanceSetting {
                temperature: ColorTemperature::DAYLIGHT,
                tint: Tint::NEUTRAL,
            },
            origin: WhiteOrigin::Fallback,
        };
        Self {
            held_before: held,
            held_now: held,
            take_now: None,
            take_serial_before: None,
            view_before: WhiteView::starting(),
            view_now: WhiteView::starting(),
        }
    }
}

/// Что ожидается от белого после шага.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WhiteCheck {
    /// Движок держит белое руки ровно с этими числами (кнопка света).
    SetByHand { kelvin: f64, duv: f64 },
    /// Экран показывает то же число и происхождение, что движок.
    ScreenShowsEngineWhite,
    /// Команда «взять» этого шага закрыта честным исходом: взято (белое источника «телефон») либо отказ (белое прежнее,
    /// экран отметил отказ).
    PhoneTakeSettled,
}

impl WhiteCheck {
    pub fn holds(self, delta: &WhiteDelta) -> bool {
        self.problem(delta).is_none()
    }

    /// Описание нарушения «ожидали X, увидели Y»; `None` — проверка выполнена.
    pub fn problem(self, delta: &WhiteDelta) -> Option<String> {
        match self {
            WhiteCheck::SetByHand { kelvin, duv } => set_by_hand_problem(kelvin, duv, delta),
            WhiteCheck::ScreenShowsEngineWhite => screen_shows_engine_problem(delta),
            WhiteCheck::PhoneTakeSettled => take_problem(delta),
        }
    }
}

fn set_by_hand_problem(kelvin: f64, duv: f64, delta: &WhiteDelta) -> Option<String> {
    let held = delta.held_now;
    let same_numbers = (held.setting.temperature.kelvin() - kelvin).abs() <= KELVIN_TOLERANCE
        && (held.setting.tint.duv() - duv).abs() <= DUV_TOLERANCE;
    if held.origin == WhiteOrigin::Manual && same_numbers {
        return None;
    }
    Some(format!(
        "белое руки {kelvin:.0} К / {duv:+.4} Duv в движке, увидели {:.0} К / {:+.4} Duv, происхождение {:?}",
        held.setting.temperature.kelvin(),
        held.setting.tint.duv(),
        held.origin
    ))
}

fn screen_shows_engine_problem(delta: &WhiteDelta) -> Option<String> {
    let held = delta.held_now;
    let engine_kelvin = kelvin_rounded(held.setting.temperature.kelvin());
    if delta.view_now.kelvin == engine_kelvin && delta.view_now.origin == held.origin {
        return None;
    }
    Some(format!(
        "экран показывает то же белое, что движок ({engine_kelvin} К, {:?}), увидели на экране {} К, {:?}",
        held.origin, delta.view_now.kelvin, delta.view_now.origin
    ))
}

fn take_problem(delta: &WhiteDelta) -> Option<String> {
    let Some(take) = delta.new_take() else {
        return Some("у команды «взять» этого шага должен быть исход в движке, новой команды нет".to_owned());
    };
    match take.state {
        WhiteTakeState::Waiting => {
            Some("исход «взять» должен закрыться (взято или отказ), движок всё ещё ждёт".to_owned())
        }
        WhiteTakeState::Cancelled => Some("исход «взять» должен быть взято или отказ, увидели «отменено»".to_owned()),
        WhiteTakeState::Taken => taken_problem(delta),
        WhiteTakeState::Refused => refused_problem(delta),
    }
}

/// «Взято» обязано сделать белое ответом источника «телефон», а экран не должен был трястись.
fn taken_problem(delta: &WhiteDelta) -> Option<String> {
    if delta.held_now.origin != WhiteOrigin::Source(WhiteSourceId::PHONE) {
        return Some(format!(
            "исход «взято» требует происхождения Source(phone), увидели {:?}",
            delta.held_now.origin
        ));
    }
    if delta.view_now.refusals != delta.view_before.refusals {
        return Some("взято, но экран отметил отказ (тряска и вибрация «ошибка»)".to_owned());
    }
    None
}

/// Отказ — честный итог: белое прежнее, экран отметил отказ ровно один раз.
fn refused_problem(delta: &WhiteDelta) -> Option<String> {
    if delta.held_now != delta.held_before {
        return Some(format!(
            "отказ источника не должен менять белое, было {:?}, стало {:?}",
            delta.held_before, delta.held_now
        ));
    }
    let shaken = delta.view_now.refusals.saturating_sub(delta.view_before.refusals);
    if shaken != 1 {
        return Some(format!(
            "отказ обязан один раз встряхнуть экран (refusals), встряхнул {shaken} раз"
        ));
    }
    None
}

/// Как кончилась команда «взять» этого шага — одним словом для отчёта (`observed`): честный отказ и «взято» видны
/// глазами, а не только как «зелёный» шаг.
pub fn take_verdict_word(delta: &WhiteDelta) -> &'static str {
    match delta.new_take().map(|take| take.state) {
        None => "нет",
        Some(WhiteTakeState::Waiting) => "ждёт",
        Some(WhiteTakeState::Taken) => "взято",
        Some(WhiteTakeState::Refused) => "отказ",
        Some(WhiteTakeState::Cancelled) => "отменено",
    }
}

#[cfg(test)]
#[path = "screen_white_check_tests.rs"]
mod tests;
