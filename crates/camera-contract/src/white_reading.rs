//! Показания источников белого: что источник ответил бы прямо сейчас (white-balance 02, решение WB33).
//!
//! Показание — **не ответ**: заявки у него нет, белую оно не меняет и владелец белого его не видит. Его показывает
//! экран на кнопке источника («≈ 3100»), чтобы человек знал, что получит, до тапа.
//!
//! Пишет исполнитель источников (поток рендера), читает экран. Инвариант свежести держит сам источник: значение,
//! которое не измерено на текущем потоке (белая прошлого сеанса), показанием не становится — это
//! [`WhiteReading::NotMeasuredYet`]. На новый поток прежнее показание забывается.

use std::sync::{Arc, Mutex, PoisonError};

use serde::Serialize;

use crate::control::WhiteBalanceSetting;
use crate::white::{WhiteSourceFailure, WhiteSourceId, WhiteUnavailableReason};

/// Что источник видит сейчас.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum WhiteReading {
    /// Источник ещё не мерил на этом потоке (или света не хватает): числа нет.
    NotMeasuredYet,
    /// Свет сцены, измеренный на этом потоке.
    Measured { white: WhiteBalanceSetting },
    /// Источник на этом потоке не ответит вовсе.
    Unavailable { reason: WhiteUnavailableReason },
    /// Источник сломался.
    Failed { failure: WhiteSourceFailure },
}

/// Показание одного источника.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct WhiteSourceReading {
    pub source: WhiteSourceId,
    pub reading: WhiteReading,
}

/// Доска показаний: последнее показание каждого источника. Клоны делят одно состояние. Замок держится только на
/// копирование одного значения; очереди нет — промежуточные показания не нужны.
#[derive(Debug, Clone, Default)]
pub struct WhiteReadingBoard {
    readings: Arc<Mutex<Vec<WhiteSourceReading>>>,
}

impl WhiteReadingBoard {
    pub fn new() -> Self {
        Self::default()
    }

    /// Заменить показание источника.
    pub fn publish(&self, report: WhiteSourceReading) {
        // Отравленный замок безопасен: записи — копируемые значения, частично записанной не бывает.
        let mut readings = self.readings.lock().unwrap_or_else(PoisonError::into_inner);
        match readings.iter_mut().find(|known| known.source == report.source) {
            Some(known) => known.reading = report.reading,
            None => readings.push(report),
        }
    }

    /// Последнее показание источника; `None` — источник ещё ничего не показывал (или его нет).
    pub fn latest_of(&self, source: WhiteSourceId) -> Option<WhiteReading> {
        let readings = self.readings.lock().unwrap_or_else(PoisonError::into_inner);
        readings
            .iter()
            .find(|known| known.source == source)
            .map(|known| known.reading)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{ColorTemperature, Tint};

    const NETWORK: WhiteSourceId = WhiteSourceId::new("network");

    fn measured(kelvin: f64) -> WhiteReading {
        WhiteReading::Measured {
            white: WhiteBalanceSetting {
                temperature: ColorTemperature::from_kelvin(kelvin).unwrap(),
                tint: Tint::NEUTRAL,
            },
        }
    }

    #[test]
    fn a_source_that_never_published_has_no_reading() {
        let board = WhiteReadingBoard::new();
        assert_eq!(board.latest_of(WhiteSourceId::PHONE), None);
        board.publish(WhiteSourceReading {
            source: NETWORK,
            reading: measured(3_100.0),
        });
        assert_eq!(
            board.latest_of(WhiteSourceId::PHONE),
            None,
            "чужое показание не подставляется"
        );
    }

    #[test]
    fn a_clone_sees_the_latest_reading_and_sources_do_not_mix() {
        let writer = WhiteReadingBoard::new();
        let reader = writer.clone();
        for (source, kelvin) in [
            (WhiteSourceId::PHONE, 3_100.0),
            (NETWORK, 5_000.0),
            (WhiteSourceId::PHONE, 4_050.0),
        ] {
            writer.publish(WhiteSourceReading {
                source,
                reading: measured(kelvin),
            });
        }
        assert_eq!(reader.latest_of(WhiteSourceId::PHONE), Some(measured(4_050.0)));
        assert_eq!(reader.latest_of(NETWORK), Some(measured(5_000.0)));
    }

    #[test]
    fn a_lost_measurement_replaces_the_old_number() {
        let board = WhiteReadingBoard::new();
        for reading in [measured(3_100.0), WhiteReading::NotMeasuredYet] {
            board.publish(WhiteSourceReading {
                source: WhiteSourceId::PHONE,
                reading,
            });
        }
        assert_eq!(
            board.latest_of(WhiteSourceId::PHONE),
            Some(WhiteReading::NotMeasuredYet)
        );
    }
}
