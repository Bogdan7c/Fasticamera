//! Упоры ручек теплоты и оттенка: на какой отметке шкалы картинка перестаёт меняться (решения 288, 293).
//!
//! # Что такое упор
//! Если запрошенный сдвиг белой непригоден для камеры (канал нейтрали у нуля), [`super::white::shifted_neutral`]
//! урезает его: сначала оттенок (доля запрошенного), а если и без оттенка никак — теплота (тоже доля запрошенного).
//! Урезанный сдвиг от дальнейшего хода ручки не зависит, поэтому именно с этой отметки картинка стоит на месте:
//!
//! * **Оттенок** (теплота стоит где стоит). Дальше упора применяется один и тот же оттенок — картинка постоянна.
//!   Если непригодна уже теплота сама по себе (оттенок нулевой), оттенок мёртв с центра: [`TravelStop::DeadFromCentre`].
//! * **Теплота** (оттенок стоит где стоит). Между первой непригодной точкой и упором картинка ещё ЕДЕТ: урезается
//!   оттенок, и он тянет за собой белую. Постоянной она становится лишь там, где непригодна теплота при нулевом
//!   оттенке, — это и есть упор теплоты. Он не зависит от оттенка (отличается лишь точка, где оттенок обнулится).
//! * **Сжатый ход** ([`super::white::WhiteShiftLimit::TravelCompressed`], холодный свет) — НЕ упор: весь ход сжат так,
//!   что картинка меняется до самого края шкалы. Критерий смотрит только на пригодность нейтрали, не на сжатие.
//!
//! # Стоимость
//! Один расчёт — не более `1 + 4 · (2 + 14)` = 65 вычислений сдвига (`shift_white` + перевод
//! в нейтраль): проверка нуля, затем по четыре упора (теплота и оттенок × минус и плюс): проверка края, проверка
//! нуля и деление пополам. Считать нужно при смене ручки или белой сцены, а не на каждый кадр.

use look_contract::{LookKnob, LookSettings};

use super::model::LookModelSettings;
use super::white::{FullShiftAttempt, WhiteShift, WhiteShiftSkipReason, largest_usable};
use super::white_shift;
use crate::color::chromaticity::Chromaticity;
use crate::color::dng::{CameraNeutral, DngColorProfile};

/// Доля хода ручки на одной стороне от нуля: 1,0 — край шкалы. Точность — 2⁻¹⁴ (14 делений пополам, как в урезании).
///
/// Это НАИБОЛЬШАЯ отметка, где сдвиг ещё применяется целиком: чуть дальше — уже урезан.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct TravelFraction(f32);

impl TravelFraction {
    /// Доля хода, `0.0..=1.0`.
    pub fn get(self) -> f32 {
        self.0
    }
}

/// Где на одной стороне ручки картинка перестаёт меняться.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TravelStop {
    /// Упора нет: сдвиг применяется целиком до самого края шкалы (или ход сжат, но картинка меняется до края).
    Unbounded,
    /// Упор на этой доле хода: дальше картинка не меняется (для теплоты — перестаёт меняться оттенок-сопровождение,
    /// см. описание модуля).
    At(TravelFraction),
    /// Ручка не меняет картинку с самого нуля: другая ручка уже за своим упором. Только у оттенка.
    DeadFromCentre,
}

/// Упоры одной ручки по обе стороны от нуля.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SideStops {
    /// Сторона «−» ручки.
    pub minus: TravelStop,
    /// Сторона «+» ручки.
    pub plus: TravelStop,
}

/// Упоры теплоты и оттенка для данной белой сцены и ТЕКУЩИХ значений ручек.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WhiteKnobStops {
    /// Теплота (оттенок стоит на текущем месте; упор от него не зависит, см. модуль).
    pub warmth: SideStops,
    /// Оттенок при текущей теплоте.
    pub tint: SideStops,
}

/// Сторона шкалы ручки относительно нуля.
#[derive(Debug, Clone, Copy)]
enum KnobSide {
    Minus,
    Plus,
}

impl KnobSide {
    /// Значение ручки на этой стороне при доле хода `fraction`.
    fn knob_at(self, fraction: f32) -> LookKnob {
        let signed = match self {
            Self::Minus => -fraction,
            Self::Plus => fraction,
        };
        let knob = LookKnob::new(signed);
        // Доля всегда в [0, 1] (границы делит пополам `largest_usable`), так что отказ — ошибка программиста.
        debug_assert!(knob.is_ok(), "доля хода вне [0, 1]: {fraction}");
        knob.unwrap_or(LookKnob::NEUTRAL)
    }
}

/// Упоры теплоты и оттенка по обе стороны шкалы.
///
/// Вход тот же, что у [`super::white::shifted_neutral`] (профиль, белая и нейтраль сцены), плюс модель вида (она знает
/// силу края ручки) и текущие значения ручек. Остальные ручки `settings` на результат не влияют.
///
/// # Ошибка
/// Если не применяется даже нулевой сдвиг (белая сцены вне таблицы Робертсона и т. п.), ручки не делают ничего:
/// отдаётся та же причина, что у [`super::white::WhiteShiftLimit::NotApplied`].
pub fn white_knob_stops(
    profile: &DngColorProfile,
    white: Chromaticity,
    scene: CameraNeutral,
    model: &LookModelSettings,
    settings: &LookSettings,
) -> Result<WhiteKnobStops, WhiteShiftSkipReason> {
    let usability = FullShiftAttempt::new(profile, white, scene);
    // Без этого проверки «нуль непригоден» ниже перепутали бы «мёртвую ручку» с «сломанной сценой».
    usability.apply(WhiteShift::NONE)?;
    let applies = |warmth: LookKnob, tint: LookKnob| {
        let shift = white_shift(
            model,
            &LookSettings {
                warmth,
                tint,
                ..LookSettings::NEUTRAL
            },
        );
        usability.apply(shift).is_ok()
    };
    let warmth_stop = |side: KnobSide| {
        // Оттенок нулевой: так считает и урезание, когда оно переходит к теплоте (см. описание модуля).
        stop_along(|fraction| applies(side.knob_at(fraction), LookKnob::NEUTRAL))
    };
    let tint_stop = |side: KnobSide| stop_along(|fraction| applies(settings.warmth, side.knob_at(fraction)));
    Ok(WhiteKnobStops {
        warmth: SideStops {
            minus: warmth_stop(KnobSide::Minus),
            plus: warmth_stop(KnobSide::Plus),
        },
        tint: SideStops {
            minus: tint_stop(KnobSide::Minus),
            plus: tint_stop(KnobSide::Plus),
        },
    })
}

/// Упор вдоль одной стороны: `applies(доля)` — применяется ли сдвиг целиком (пригодные доли — отрезок от нуля).
fn stop_along(applies: impl Fn(f32) -> bool) -> TravelStop {
    if applies(1.0) {
        return TravelStop::Unbounded;
    }
    // Тот же поиск границы, что в урезании, — упор совпадает с началом урезания с точностью шага.
    match largest_usable(|fraction| applies(fraction as f32).then_some(())) {
        Some((fraction, ())) => TravelStop::At(TravelFraction(fraction as f32)),
        None => TravelStop::DeadFromCentre,
    }
}
