//! Модель значений колеса зума: логарифмическая шкала с упорами каталога и рисками по правилу iOS.
//!
//! Правило рисок (`[measured: R00 t=88.5–100]`): через 0,1× до ≈ 4,4×, дальше через 1×; яркие — на 0,5 / 1 / 1,2 /
//! 1,5 / 2 и целых от 3. Упоры каталога (кнопки ленты) — `Emphasis::Reference`: на них магнит и сильный щелчок.
//! Остальное, в том числе яркость целых, — только вид. Все числа — `zoom.wheel` конфига.

use camera_contract::zoom::LensGroup;
use ui_kit::value::{LogLimits, Logarithmic, LogarithmicSpec};

use super::error::{WidgetError, element};
use crate::zoom_config::ZoomWheelLayout;

/// Единиц положения на октаву: одна, так что единица шкалы = октава, а токен плотности `Dial` задаёт её длину.
const UNITS_PER_OCTAVE: f64 = 1.0;

/// Отпечаток шкалы группы: если он изменился (другое направление, новый каталог), модель колеса строится заново.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WheelSignature {
    min: f32,
    max: f32,
    stops: Vec<f32>,
}

impl WheelSignature {
    pub(crate) fn of(group: &LensGroup) -> Self {
        Self {
            min: group.range.min.value(),
            max: group.range.max.value(),
            stops: group.stops.iter().map(|stop| stop.ratio.value()).collect(),
        }
    }
}

/// Значения обычных рисок для отрезка `[min, max]`; риски вплотную к упору (в пределах `tick_merge_octaves`)
/// опускаются — упор уже стоит на этом месте.
pub(crate) fn tick_values(layout: &ZoomWheelLayout, min: f64, max: f64, anchors: &[f64]) -> Vec<f64> {
    let mut values = Vec::new();
    let fine_count = (layout.fine_tick_until / layout.fine_tick_step).round() as i64;
    for index in 1..=fine_count {
        values.push(round_micro(index as f64 * layout.fine_tick_step));
    }
    let mut coarse = layout.fine_tick_until.floor() + layout.coarse_tick_step;
    while coarse <= max + 1e-9 {
        values.push(round_micro(coarse));
        coarse += layout.coarse_tick_step;
    }
    values
        .into_iter()
        .filter(|value| *value >= min && *value <= max)
        .filter(|value| {
            anchors
                .iter()
                .all(|anchor| (value.log2() - anchor.log2()).abs() > layout.tick_merge_octaves)
        })
        .collect()
}

/// Убирает хвост двоичной арифметики (0,30000000000000004 → 0,3), чтобы яркие риски сравнивались точно.
fn round_micro(value: f64) -> f64 {
    (value * 1.0e6).round() / 1.0e6
}

/// Модель колеса для группы: пределы — диапазон группы, опорные точки — её упоры.
pub(crate) fn build_model(layout: &ZoomWheelLayout, group: &LensGroup) -> Result<Logarithmic, WidgetError> {
    let min = f64::from(group.range.min.value());
    let max = f64::from(group.range.max.value());
    let anchors: Vec<f64> = group.stops.iter().map(|stop| f64::from(stop.ratio.value())).collect();
    let ticks = tick_values(layout, min, max, &anchors);
    Logarithmic::new(
        LogarithmicSpec {
            units_per_octave: UNITS_PER_OCTAVE,
            // Не используется: риски заданы значениями (`with_tick_values`), но модель требует > 0.
            tick_spacing_units: 0.1,
            anchors,
        },
        LogLimits { min, max },
    )
    .and_then(|model| model.with_tick_values(ticks))
    .map_err(element("модель колеса зума"))
}

/// Яркая ли риска со значением `value`: перечисленные значения и целые от порога.
pub(crate) fn is_bright(layout: &ZoomWheelLayout, value: f64) -> bool {
    const EPS: f64 = 1.0e-4;
    layout
        .bright_tick_values
        .iter()
        .any(|bright| (value - bright).abs() < EPS)
        || (value >= layout.bright_whole_numbers_from - EPS && (value - value.round()).abs() < EPS)
}
