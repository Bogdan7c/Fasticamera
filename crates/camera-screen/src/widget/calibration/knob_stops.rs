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

/// Значение ручки на доле упора `fraction` стороны `direction` (±1). Единственное место перевода доли в значение:
/// доля зажимается в `[0, KNOB_EDGE]`, чтобы упор никогда не вышел за словарь ручки `−1…+1` и линейка не получила
/// значение, которого не может нарисовать (`ViewError::UnrepresentableOverlayValue`). `StopFraction` уже гарантирует
/// `(0, 1]`; зажим — защита от будущего ослабления проверки, а не обход ошибки.
fn stop_value(fraction: look_contract::StopFraction, direction: f64) -> f64 {
    f64::from(fraction.get()).clamp(0.0, KNOB_EDGE) * direction
}

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
            RowTarget::BandAxis(_) | RowTarget::WheelRing | RowTarget::Primary(_) => KnobSideStops {
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
                    KnobStop::At(fraction) => stop_value(fraction, direction),
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
        KnobStop::At(fraction) => Some(stop_value(fraction, direction)),
        KnobStop::None | KnobStop::DeadFromCentre => None,
    }
}

#[cfg(test)]
mod tests {
    use look_contract::StopFraction;

    use super::*;

    fn edge() -> KnobStop {
        KnobStop::At(StopFraction::new(1.0).expect("доля 1,0 допустима"))
    }

    #[test]
    fn a_stop_on_the_scale_edge_maps_to_the_edge_value_with_no_dimmed_travel_and_never_beyond_it() {
        let stops = RowStops {
            sides: KnobSideStops {
                minus: edge(),
                plus: edge(),
            },
        };
        assert_eq!(
            stops.tick_values().collect::<Vec<_>>(),
            [-1.0, 1.0],
            "риски на краях словаря ручки"
        );
        assert!(
            stops.dimmed_ranges(&KnobStopsLookConfig::default()).is_empty(),
            "за упором на краю ничего нет: затемнять нечего"
        );
        let at_edge = LookKnob::new(1.0).expect("край");
        assert!(!stops.holds_beyond(at_edge), "на самом упоре ручка не «за упором»");
    }

    #[test]
    fn every_valid_fraction_maps_inside_the_knob_dictionary() {
        for fraction in [f32::MIN_POSITIVE, 0.01, 0.46, 0.999_999_9, 1.0] {
            let fraction = StopFraction::new(fraction).expect("доля");
            for direction in [-1.0, 1.0] {
                let value = stop_value(fraction, direction);
                assert!(value.abs() <= KNOB_EDGE && value.abs() > 0.0, "{value}");
                assert_eq!(value.signum(), direction);
            }
        }
    }
}
