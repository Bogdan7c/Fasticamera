//! Публикация упоров ручек теплоты и оттенка на доску для приложения (решения 288, 293).
//!
//! Владелец упоров — [`super::isp_params::IspParamsCache`] (считает при смене входов). Здесь только перевод на
//! язык экрана (`look_contract::white_stops`, без типов `isp-model`) и публикация НА ИЗМЕНЕНИЕ: кадр зовёт
//! [`LookStopsPublisher::sync`], а доска пишется, только если значение отличается от уже опубликованного.
//!
//! # Состояния
//! * кэш упоров не знает (потока нет, поток забыт, параметров ещё нет) → [`LookWhiteStops::Unknown`];
//! * сдвиг белой не применяется вовсе (причина `isp-model`) → [`LookWhiteStops::NotApplicable`];
//! * упоры посчитаны → [`LookWhiteStops::Known`].
//!
//! Сжатый ход (`TravelCompressed`, холодный свет) упором не является (решение 293): `TravelStop::Unbounded` →
//! [`KnobStop::None`].

use isp_model::look::white_stops::{SideStops, TravelStop, WhiteKnobStops};
use look_contract::{KnobSideStops, KnobStop, KnobTravelStops, LookWhiteStops, LookWhiteStopsBoard, StopFraction};

use super::isp_params::IspParamsCache;

/// Упор `isp-model` → упор экрана. Доля 0 (сдвиг пригоден только в самом нуле) — та же «мёртвая ручка», что и
/// «непригоден с нуля»: отметки на шкале у неё нет.
fn stop_of(stop: TravelStop) -> KnobStop {
    match stop {
        TravelStop::Unbounded => KnobStop::None,
        TravelStop::DeadFromCentre => KnobStop::DeadFromCentre,
        TravelStop::At(fraction) => match StopFraction::new(fraction.get()) {
            Ok(fraction) => KnobStop::At(fraction),
            Err(_) => KnobStop::DeadFromCentre,
        },
    }
}

fn side_of(side: SideStops) -> KnobSideStops {
    KnobSideStops {
        minus: stop_of(side.minus),
        plus: stop_of(side.plus),
    }
}

/// Упоры `isp-model` → упоры экрана.
pub(super) fn travel_stops_of(stops: &WhiteKnobStops) -> KnobTravelStops {
    KnobTravelStops {
        warmth: side_of(stops.warmth),
        tint: side_of(stops.tint),
    }
}

/// Состояние доски, которое соответствует кэшу параметров сейчас.
fn desired_state(params: &IspParamsCache) -> LookWhiteStops {
    match params.white_knob_stops() {
        None => LookWhiteStops::Unknown,
        Some(Err(_reason)) => LookWhiteStops::NotApplicable,
        Some(Ok(stops)) => LookWhiteStops::Known(travel_stops_of(stops)),
    }
}

/// Писатель доски упоров: помнит, что опубликовал, и пишет только изменения.
#[derive(Debug)]
pub(super) struct LookStopsPublisher {
    board: LookWhiteStopsBoard,
    /// Что лежит на доске сейчас (доска начинается с `Unknown`).
    published: LookWhiteStops,
}

impl LookStopsPublisher {
    pub(super) fn new(board: LookWhiteStopsBoard) -> Self {
        Self {
            board,
            published: LookWhiteStops::Unknown,
        }
    }

    /// Приводит доску в соответствие с кэшем параметров. Зовётся после подготовки параметров кадра и после
    /// «поток неактивен» (забытый поток даёт `Unknown`). Дешёвая: сравнение двух маленьких копируемых значений.
    pub(super) fn sync(&mut self, params: &IspParamsCache) {
        let desired = desired_state(params);
        if desired != self.published {
            self.board.publish(desired);
            self.published = desired;
        }
    }
}
