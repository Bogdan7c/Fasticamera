//! Упоры ручек теплоты и оттенка (решения 288, 293): кэш считает их при смене входов, писатель доски переводит и
//! публикует на изменение. Свет — лампа 2856 К по калибровке образца основной камеры: на ней оттенок «+» упирается.

use std::sync::Arc;

use isp_model::color::chromaticity::Chromaticity;
use isp_model::color::temperature::Kelvin;
use isp_model::color::white_point::{CctDuv, Duv, chromaticity_from_cct_duv};
use isp_model::look::white::WhiteShiftSkipReason;
use isp_model::look::white_stops::TravelStop;
use look_contract::{KnobStop, LookKnob, LookSettings, LookWhiteStops, LookWhiteStopsBoard};

use super::super::look_stops_publisher::{LookStopsPublisher, travel_stops_of};
use super::tests::{calibration, frame, plain, stream};
use super::*;

/// Лампа 2856 К на локусе.
fn lamp() -> Chromaticity {
    chromaticity_from_cct_duv(CctDuv {
        temperature: Kelvin(2856.0),
        duv: Duv(0.0),
    })
    .expect("лампа в таблице")
}

/// Белая вне таблицы Робертсона (синее таблицы): камера её в нейтраль переводит, а сдвиг белой — нет.
const OUTSIDE_TABLE: Chromaticity = Chromaticity { x: 0.2, y: 0.2 };

fn look(warmth: f32, tint: f32) -> Arc<LookSettings> {
    Arc::new(LookSettings {
        warmth: LookKnob::new(warmth).expect("ручка в диапазоне"),
        tint: LookKnob::new(tint).expect("ручка в диапазоне"),
        ..LookSettings::NEUTRAL
    })
}

/// Упор оттенка «+» как число; остальное падает: проверяем именно «упор есть».
fn tint_plus_fraction(stops: &LookWhiteStops) -> f32 {
    match stops {
        LookWhiteStops::Known(known) => match known.tint.plus {
            KnobStop::At(fraction) => fraction.get(),
            other => panic!("ждали упор оттенка «+», получили {other:?}"),
        },
        other => panic!("ждали посчитанные упоры, получили {other:?}"),
    }
}

#[test]
fn warm_lamp_with_warmth_at_minus_one_gets_a_tint_stop_in_the_cache() {
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let mut cache = IspParamsCache::default().with_look(look(-1.0, 0.0));
    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);

    let stops = cache
        .white_knob_stops()
        .expect("упоры посчитаны")
        .as_ref()
        .expect("сдвиг применим");
    match stops.tint.plus {
        TravelStop::At(fraction) => assert!(
            (0.2..0.26).contains(&fraction.get()),
            "для лампы 2856 К при теплоте −1 ждали около 0,23: {}",
            fraction.get()
        ),
        other => panic!("ждали упор оттенка «+», получили {other:?}"),
    }
    assert_eq!(stops.tint.minus, TravelStop::Unbounded);
    assert_eq!(stops.warmth.minus, TravelStop::Unbounded);
    assert_eq!(stops.warmth.plus, TravelStop::Unbounded);
}

#[test]
fn stops_are_recomputed_only_when_the_white_or_the_two_knobs_change() {
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let mut cache = IspParamsCache::default().with_look(look(-1.0, 0.0));
    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    assert_eq!(cache.knob_stops_computations(), 1);

    // Тот же кадр, повтор вида, подтяжка AE, другой контраст: упорам они не нужны.
    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    cache.prepare_with_exposure_gain(
        &frame,
        lamp(),
        ExposureGain::new(1.5).expect("подтяжка"),
        DisplayGamut::Srgb,
    );
    let contrast = Arc::new(LookSettings {
        contrast: LookKnob::new(0.4).expect("ручка в диапазоне"),
        ..(*look(-1.0, 0.0)).clone()
    });
    assert_eq!(cache.set_look(contrast), LookChange::Changed);
    assert!(matches!(
        cache.prepare(&frame, lamp(), DisplayGamut::Srgb).outcome,
        ParamsOutcome::Resolved
    ));
    assert_eq!(
        cache.knob_stops_computations(),
        1,
        "параметры пересчитаны, а упоры нет: ни теплота, ни оттенок, ни белая не менялись"
    );

    // Тёплая ручка — считаем снова, и результат другой.
    let before = tint_plus_fraction(&LookWhiteStops::Known(travel_stops_of(
        cache.white_knob_stops().unwrap().as_ref().unwrap(),
    )));
    cache.set_look(look(0.0, 0.0));
    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    assert_eq!(cache.knob_stops_computations(), 2);
    let after = tint_plus_fraction(&LookWhiteStops::Known(travel_stops_of(
        cache.white_knob_stops().unwrap().as_ref().unwrap(),
    )));
    assert!(
        after > before + 0.3,
        "теплота 0 отодвигает упор оттенка: {before} → {after}"
    );

    // Оттенок и белая тоже входят в ключ.
    cache.set_look(look(0.0, 0.5));
    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    assert_eq!(cache.knob_stops_computations(), 3);
    cache.prepare(&frame, Chromaticity::D65, DisplayGamut::Srgb);
    assert_eq!(cache.knob_stops_computations(), 4);
}

#[test]
fn a_white_outside_the_table_reports_why_the_knobs_do_nothing() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    let decision = cache.prepare(&frame(&stream, plain()), OUTSIDE_TABLE, DisplayGamut::Srgb);
    assert_eq!(decision.outcome, ParamsOutcome::Resolved, "кадр сам по себе рисуется");
    assert!(matches!(
        cache.white_knob_stops(),
        Some(Err(WhiteShiftSkipReason::ShiftOutsideTable(_)))
    ));
}

#[test]
fn a_forgotten_stream_has_no_stops() {
    let stream = stream(calibration());
    let mut cache = IspParamsCache::default();
    assert!(cache.white_knob_stops().is_none(), "до первого кадра упоров нет");
    cache.prepare(&frame(&stream, plain()), lamp(), DisplayGamut::Srgb);
    assert!(cache.white_knob_stops().is_some());
    cache.forget_stream();
    assert!(
        cache.white_knob_stops().is_none(),
        "упоры чужого потока не доживают до нового"
    );
}

#[test]
fn the_publisher_puts_stops_on_the_board_and_republishes_when_the_knob_moves() {
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let board = LookWhiteStopsBoard::new();
    let mut publisher = LookStopsPublisher::new(board.clone());
    let mut cache = IspParamsCache::default().with_look(look(-1.0, 0.0));

    publisher.sync(&cache);
    assert_eq!(board.latest(), LookWhiteStops::Unknown, "кадров не было");

    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    publisher.sync(&cache);
    let first = board.latest();
    let first_fraction = tint_plus_fraction(&first);
    assert!((0.2..0.26).contains(&first_fraction), "{first_fraction}");
    // Перевод точен: доля и виды упоров те же, что посчитала модель.
    let model = cache.white_knob_stops().unwrap().as_ref().unwrap();
    assert_eq!(first, LookWhiteStops::Known(travel_stops_of(model)));
    match (&first, model.tint.plus) {
        (LookWhiteStops::Known(known), TravelStop::At(fraction)) => {
            assert_eq!(
                known.tint.plus,
                KnobStop::At(look_contract::StopFraction::new(fraction.get()).unwrap())
            );
            assert_eq!(known.tint.minus, KnobStop::None, "ход не сжат и не урезан: упора нет");
        }
        other => panic!("{other:?}"),
    }

    cache.set_look(look(0.0, 0.0));
    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    publisher.sync(&cache);
    assert!(
        tint_plus_fraction(&board.latest()) > first_fraction + 0.3,
        "смена теплоты публикует новый упор"
    );
}

#[test]
fn the_publisher_writes_nothing_while_the_stops_stay_the_same() {
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let board = LookWhiteStopsBoard::new();
    let mut publisher = LookStopsPublisher::new(board.clone());
    let mut cache = IspParamsCache::default().with_look(look(-1.0, 0.0));
    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    publisher.sync(&cache);

    // Подмена значения на доске видна, только если писатель её не перезапишет: значит, он не пишет без изменений.
    board.publish(LookWhiteStops::NotApplicable);
    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    publisher.sync(&cache);
    assert_eq!(board.latest(), LookWhiteStops::NotApplicable);
}

#[test]
fn the_publisher_says_not_applicable_for_a_white_outside_the_table_and_unknown_after_the_stream_is_forgotten() {
    let stream = stream(calibration());
    let frame = frame(&stream, plain());
    let board = LookWhiteStopsBoard::new();
    let mut publisher = LookStopsPublisher::new(board.clone());
    let mut cache = IspParamsCache::default();

    cache.prepare(&frame, OUTSIDE_TABLE, DisplayGamut::Srgb);
    publisher.sync(&cache);
    assert_eq!(board.latest(), LookWhiteStops::NotApplicable);

    cache.prepare(&frame, lamp(), DisplayGamut::Srgb);
    publisher.sync(&cache);
    assert!(
        matches!(board.latest(), LookWhiteStops::Known(_)),
        "белая вернулась в таблицу"
    );

    cache.forget_stream();
    publisher.sync(&cache);
    assert_eq!(board.latest(), LookWhiteStops::Unknown);
}
