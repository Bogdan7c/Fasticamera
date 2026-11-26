//! Упоры ручек теплоты и оттенка для экрана калибровки (решения 288, 293).
//!
//! Словарь без типов `isp-model`: считает упоры поток рендера (там живёт профиль цвета камеры), показывает экран.
//! Упор — это отметка на шкале ручки, дальше которой картинка больше не меняется.
//!
//! # Что упором НЕ является
//! Сжатый ход (холодный свет, `TravelCompressed` в `isp-model`) упором не считается (решение 293): ход сжат, но
//! картинка меняется до самого края шкалы, так что упора нет — [`KnobStop::None`].
//!
//! # Почему доска здесь
//! Доска [`LookWhiteStopsBoard`] — «последнее значение» (как `WhiteReadingBoard` для показаний белого): пишет
//! рендер при смене входов, читает приложение для экрана; очереди нет, промежуточные значения не нужны.

use std::sync::{Arc, Mutex, PoisonError};

/// Ошибка проверки доли хода ручки.
#[derive(Debug, Clone, Copy, PartialEq, thiserror::Error)]
pub enum StopFractionError {
    /// Число вне полуинтервала (0, 1] или не число. Нуль не упор (ручка в нейтрали), а «мёртвая с центра» — отдельный
    /// вариант [`KnobStop::DeadFromCentre`].
    #[error("доля хода упора {value} вне (0, 1]")]
    OutOfRange {
        /// Отвергнутое значение.
        value: f32,
    },
}

/// Доля хода ручки на одной стороне от нуля, `(0, 1]`: 1,0 — край шкалы.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct StopFraction(f32);

impl StopFraction {
    /// Проверяет и строит долю: конечное число строго больше нуля и не больше единицы.
    pub fn new(value: f32) -> Result<Self, StopFractionError> {
        // NaN не проходит ни одно из сравнений, поэтому отдельная проверка на число не нужна.
        if value > 0.0 && value <= 1.0 {
            Ok(Self(value))
        } else {
            Err(StopFractionError::OutOfRange { value })
        }
    }

    /// Доля хода, `(0, 1]`.
    pub fn get(self) -> f32 {
        self.0
    }
}

/// Где на одной стороне ручки картинка перестаёт меняться.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KnobStop {
    /// Упора нет: ручка меняет картинку до самого края шкалы (в том числе при сжатом ходе, решение 293).
    None,
    /// Упор на этой доле хода одной стороны: НАИБОЛЬШАЯ отметка, где сдвиг ещё применяется целиком; дальше
    /// картинка не меняется.
    At(StopFraction),
    /// Ручка не меняет картинку с самого нуля (другая ручка уже за своим упором). Бывает только у оттенка.
    DeadFromCentre,
}

/// Упоры одной ручки по обе стороны от нуля.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnobSideStops {
    /// Сторона «−».
    pub minus: KnobStop,
    /// Сторона «+».
    pub plus: KnobStop,
}

/// Упоры теплоты и оттенка при текущих значениях ручек и текущей белой сцены.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KnobTravelStops {
    /// Теплота.
    pub warmth: KnobSideStops,
    /// Оттенок (при текущей теплоте).
    pub tint: KnobSideStops,
}

/// Что известно об упорах сейчас. Три состояния различимы, чтобы экран не путал «ещё не знаем» с «упоров нет».
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum LookWhiteStops {
    /// Неизвестно: потока нет, кадров с метаданными ещё не было или параметры не посчитаны.
    #[default]
    Unknown,
    /// Сдвиг белой в этом кадре не применяется вовсе (белая сцены вне таблицы и т. п.): ручки теплоты и оттенка
    /// картинку не меняют, упоров на шкале нет смысла рисовать.
    NotApplicable,
    /// Упоры посчитаны.
    Known(KnobTravelStops),
}

/// Доска упоров: последнее значение, клоны делят одно состояние. Замок держится на копирование одного значения.
///
/// Отравленный замок безопасен (значение копируемое, частично записанного не бывает), поэтому читатель его
/// не считает отказом: счётчика «доска нечитаема» нет.
#[derive(Debug, Clone, Default)]
pub struct LookWhiteStopsBoard {
    latest: Arc<Mutex<LookWhiteStops>>,
}

impl LookWhiteStopsBoard {
    /// Новая доска: значение [`LookWhiteStops::Unknown`].
    pub fn new() -> Self {
        Self::default()
    }

    /// Заменяет значение (пишет поток рендера).
    pub fn publish(&self, stops: LookWhiteStops) {
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner) = stops;
    }

    /// Последнее опубликованное значение; до первой публикации — [`LookWhiteStops::Unknown`].
    pub fn latest(&self) -> LookWhiteStops {
        *self.latest.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn known() -> LookWhiteStops {
        LookWhiteStops::Known(KnobTravelStops {
            warmth: KnobSideStops {
                minus: KnobStop::None,
                plus: KnobStop::None,
            },
            tint: KnobSideStops {
                minus: KnobStop::None,
                plus: KnobStop::At(StopFraction::new(0.34).unwrap()),
            },
        })
    }

    #[test]
    fn fraction_accepts_only_the_half_open_unit_interval() {
        assert_eq!(StopFraction::new(1.0).unwrap().get(), 1.0);
        assert_eq!(StopFraction::new(0.25).unwrap().get(), 0.25);
        for bad in [0.0, -0.1, 1.0001, f32::NAN, f32::INFINITY] {
            assert!(
                matches!(StopFraction::new(bad), Err(StopFractionError::OutOfRange { .. })),
                "{bad} должно быть отвергнуто"
            );
        }
    }

    #[test]
    fn board_is_unknown_until_first_publish_and_clones_share_the_latest_value() {
        let board = LookWhiteStopsBoard::new();
        let reader = board.clone();
        assert_eq!(reader.latest(), LookWhiteStops::Unknown);

        board.publish(known());
        assert_eq!(reader.latest(), known());

        board.publish(LookWhiteStops::NotApplicable);
        assert_eq!(
            reader.latest(),
            LookWhiteStops::NotApplicable,
            "последнее значение заменяет прежнее"
        );
    }

    #[test]
    fn a_poisoned_board_still_reads_and_writes() {
        let board = LookWhiteStopsBoard::new();
        let poisoner = board.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.latest.lock().unwrap();
            panic!("отравляем замок");
        })
        .join();
        board.publish(known());
        assert_eq!(board.latest(), known());
    }
}
