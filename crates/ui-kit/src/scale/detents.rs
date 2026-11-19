//! Рубежи, на которых шкала сообщает «пройдено» (тик, крупный упор).
//!
//! * **Ступенчатая** модель: рубеж — середина между соседними шагами. Именно там меняется
//!   выбранное значение, поэтому щелчок совпадает со сменой значения, а не запаздывает до
//!   полной остановки на шаге (пружина прилипания подходит к шагу асимптотически).
//! * **Плавная** модель: рубеж — сама риска (значение меняется непрерывно, риски — ориентиры).
//!   Известное ограничение: если пружина прилипания (магнит) подходит к риске снизу без
//!   перелёта, рубеж засчитывается при точной остановке.

use super::events::TravelDirection;
use crate::value::{Emphasis, Mark, Quantization};

/// Рубеж: положение и выделение при входе с каждой стороны.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Detent {
    pub position: f64,
    /// Выделение, если рубеж пройден в сторону больших положений.
    pub entering_greater: Emphasis,
    /// Выделение, если пройден в сторону меньших.
    pub entering_less: Emphasis,
}

/// Построить рубежи по рискам модели (риски отсортированы по возрастанию).
pub(super) fn build_detents(marks: &[Mark], quantization: Quantization) -> Vec<Detent> {
    match quantization {
        Quantization::Continuous => marks
            .iter()
            .map(|mark| Detent {
                position: mark.position,
                entering_greater: mark.emphasis,
                entering_less: mark.emphasis,
            })
            .collect(),
        Quantization::Stepped => marks
            .windows(2)
            .map(|pair| Detent {
                position: (pair[0].position + pair[1].position) * 0.5,
                entering_greater: pair[1].emphasis,
                entering_less: pair[0].emphasis,
            })
            .collect(),
    }
}

/// Рубеж считается пройденным, если стороны «положение >= рубеж» у `from` и `to` различаются.
/// Так стоящая на рубеже шкала не считает его дважды, а возврат через него считается снова.
/// Результат упорядочен по ходу движения.
pub(super) fn crossed(detents: &[Detent], from: f64, to: f64) -> Vec<(Emphasis, TravelDirection)> {
    let before = detents.partition_point(|detent| detent.position <= from);
    let after = detents.partition_point(|detent| detent.position <= to);
    if after > before {
        detents[before..after]
            .iter()
            .map(|detent| (detent.entering_greater, TravelDirection::Increasing))
            .collect()
    } else {
        detents[after..before]
            .iter()
            .rev()
            .map(|detent| (detent.entering_less, TravelDirection::Decreasing))
            .collect()
    }
}
