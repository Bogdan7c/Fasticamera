//! Отметки упоров на строках теплоты и оттенка (решения 288, 293): где на шкале ручка перестаёт менять картинку.
//!
//! Упоры считает рендер (`look_contract::white_stops`), экран хранит последнее сообщение (`LookView::knob_stops`);
//! здесь — перевод упоров в то, что рисует строка: риска-упор на значении `±доля`, тусклые риски за упором и тусклое
//! число, пока ручка стоит за упором. Сжатый ход упором не считается (решение 293): отметок у него нет.
//!
//! Доля упора — доля хода одной стороны, то есть значение ручки в словаре `−1…+1` (упор 0,46 стоит на `+92` при 200
//! единицах на 1,0). Поэтому границы отдаются линейке в значениях ручки, единицы шкалы линейка считает сама.

use look_contract::{KnobSideStops, KnobStop, LookKnob};
use ui_kit::view::DimmedRange;

use super::knob_row::RowTarget;
use crate::calibration::KnobStopsLookConfig;
use crate::widget::Ctx;

/// Пределы значений ручки в словаре: край шкалы с каждой стороны.
const KNOB_EDGE: f64 = 1.0;

/// Упоры одной строки ручки по обе стороны от нуля.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct RowStops {
    sides: KnobSideStops,
}

impl RowStops {
    /// Упоры строки `target` по состоянию экрана; у строк, кроме теплоты и оттенка, отметок нет.
    pub(super) fn of(target: RowTarget, ctx: &Ctx<'_>) -> Self {
        let sides = match target {
            RowTarget::Knob(knob) => ctx.state.look().knob_stops(knob),
            RowTarget::BandAxis(_) | RowTarget::WheelRing => KnobSideStops {
                minus: KnobStop::None,
                plus: KnobStop::None,
            },
        };
        Self { sides }
    }

    /// Значения ручки, на которых стоит риска-упор (с учётом знака стороны). «Мёртвая с нуля» сторона риски не имеет:
    /// упора как точки на шкале у неё нет, тускнеет вся сторона.
    pub(super) fn tick_values(&self) -> impl Iterator<Item = f64> {
        [
            side_tick_value(self.sides.minus, -1.0),
            side_tick_value(self.sides.plus, 1.0),
        ]
        .into_iter()
        .flatten()
    }

    /// Тусклые промежутки хода: от упора до края шкалы стороны (у «мёртвой с нуля» — от нуля до края).
    pub(super) fn dimmed_ranges(&self, look: &KnobStopsLookConfig) -> Vec<DimmedRange<f64>> {
        [(self.sides.minus, -1.0), (self.sides.plus, 1.0)]
            .into_iter()
            .filter_map(|(stop, direction)| {
                let from = match stop {
                    KnobStop::None => return None,
                    KnobStop::At(fraction) => f64::from(fraction.get()) * direction,
                    KnobStop::DeadFromCentre => 0.0,
                };
                let to = KNOB_EDGE * direction;
                // Упор на самом краю шкалы: дальше него ничего нет.
                (from != to).then_some(DimmedRange {
                    from,
                    to,
                    alpha_factor: look.beyond_tick_dim_factor,
                })
            })
            .collect()
    }

    /// Стоит ли ручка за упором: картинка дальше упора не меняется, поэтому число рисуется тусклым.
    pub(super) fn holds_beyond(&self, value: LookKnob) -> bool {
        let value = value.get();
        let beyond_plus = match self.sides.plus {
            KnobStop::None => false,
            KnobStop::At(fraction) => value > fraction.get(),
            KnobStop::DeadFromCentre => value > 0.0,
        };
        let beyond_minus = match self.sides.minus {
            KnobStop::None => false,
            KnobStop::At(fraction) => value < -fraction.get(),
            KnobStop::DeadFromCentre => value < 0.0,
        };
        beyond_plus || beyond_minus
    }
}

/// Значение риски-упора стороны (`direction` — знак стороны).
fn side_tick_value(stop: KnobStop, direction: f64) -> Option<f64> {
    match stop {
        KnobStop::At(fraction) => Some(f64::from(fraction.get()) * direction),
        KnobStop::None | KnobStop::DeadFromCentre => None,
    }
}
