#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! Магнит нуля линейки-ручки (решение 284): под пальцем значение на мгновение замирает на нуле.
//!
//! Шкала как у ручек раздела: 1 pt на шаг значения (1/200), палец влево — значение растёт, значит палец ВПРАВО
//! уменьшает. Зона ±6 pt: палец проходит 12 pt, значение стоит нулём; дальше положение = путь пальца минус 6 pt
//! (без скачка на выходе). Проверяется то, что чувствует палец и слышит рука: стоит ли значение, нет ли
//! разрыва на выходе, что делает бросок, сколько щелчков нуля прозвучало и что у прежних линеек магнита нет.

use std::num::NonZeroU32;

use shell_contract::haptics::HapticPattern;
use ui_core::geometry::Vector;
use ui_core::gesture::PanTranslation;
use ui_core::time::MotionPreference;
use ui_kit::drive::DragOrigin;
use ui_kit::pan::PanUpdate;
use ui_kit::ruler::{GatedEvent, HapticPacing, RulerControl, RulerFeel, RulerSpec};
use ui_kit::scale::{ScaleError, ZeroMagnet};
use ui_kit::tokens::{HapticEvent, PointsPerUnit, ScaleDensity, UiKitTokens};
use ui_kit::value::{ContinuousLinear, LinearLimits, QuantizedLinear, ReferenceValue, TickPitch};

mod common;
use common::{env, ms};

/// Шагов значения на 1,0.
const STEPS_PER_ONE: f64 = 200.0;
/// Половина зоны магнита, pt пути пальца.
const HALF_ZONE_PT: f64 = 6.0;
/// Кадр медленного ведения, мс (ограничитель частоты щелчков не мешает: 100 мс больше его интервала).
const SLOW_FRAME_MS: i64 = 100;

fn knob_model() -> QuantizedLinear {
    QuantizedLinear::new(
        1.0 / STEPS_PER_ONE,
        ReferenceValue::At(0.0),
        LinearLimits { min: -1.0, max: 1.0 },
    )
    .unwrap()
    .with_tick_pitch(TickPitch::EveryNSteps(NonZeroU32::new(20).unwrap()))
}

fn feel_with(zero_magnet: ZeroMagnet) -> RulerFeel {
    RulerFeel {
        drag_origin: DragOrigin::FromTouchDown,
        haptic_pacing: HapticPacing::Throttled,
        zero_magnet,
    }
}

fn magnet() -> ZeroMagnet {
    ZeroMagnet::hold_within(HALF_ZONE_PT).unwrap()
}

fn ruler_at(tokens: &UiKitTokens, feel: RulerFeel, start_steps: f64) -> RulerControl {
    RulerControl::new(
        RulerSpec {
            model: knob_model(),
            density: ScaleDensity::Explicit(PointsPerUnit::new(1.0).unwrap()),
            major_every_steps: NonZeroU32::new(50).unwrap(),
            feel,
        },
        start_steps / STEPS_PER_ONE,
        &env(tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap()
}

fn finger_right_by(pt: f64) -> PanTranslation {
    PanTranslation {
        from_touch_down: Vector::new(pt, 0.0),
        from_recognition: Vector::new(pt, 0.0),
    }
}

fn begin(ruler: &mut RulerControl) {
    ruler
        .apply_pan(
            ms(0),
            PanUpdate::Began {
                translation: finger_right_by(0.0),
            },
        )
        .unwrap();
}

/// Палец сдвинут на `pt` вправо к кадру номер `frame`; возвращает события кадра.
fn move_finger(ruler: &mut RulerControl, frame: i64, pt: f64) -> Vec<GatedEvent<f64>> {
    let at = ms(frame * SLOW_FRAME_MS);
    ruler
        .apply_pan(
            at,
            PanUpdate::Changed {
                translation: finger_right_by(pt),
            },
        )
        .unwrap();
    ruler.advance(at)
}

fn position_after(ruler: &RulerControl, frame: i64) -> f64 {
    ruler.position_steps(ms(frame * SLOW_FRAME_MS))
}

fn reference_clicks(tokens: &UiKitTokens, events: &[GatedEvent<f64>]) -> usize {
    let reference: HapticPattern = tokens.haptics.pattern_for(HapticEvent::ScaleReference).unwrap();
    events
        .iter()
        .filter(|event| matches!(event, GatedEvent::Haptic(pattern) if *pattern == reference))
        .count()
}

// ----- держание и выход без скачка --------------------------------------------------------------------------------

#[test]
fn value_holds_zero_inside_the_zone_and_leaves_it_without_a_jump() {
    let tokens = UiKitTokens::default();
    // Старт на +10 шагах; палец идёт вправо по 1 pt за кадр до 30 pt: 10 pt до нуля, 12 pt держания, 8 pt дальше.
    let mut ruler = ruler_at(&tokens, feel_with(magnet()), 10.0);
    begin(&mut ruler);
    let mut positions = vec![position_after(&ruler, 0)];
    for frame in 1..=30 {
        move_finger(&mut ruler, frame, frame as f64);
        positions.push(position_after(&ruler, frame));
    }
    assert_eq!(positions[0], 10.0, "на старте скачка нет: ручка стоит где стояла");
    assert_eq!(positions[9], 1.0, "до нуля значение идёт за пальцем 1:1");
    for (frame, position) in positions.iter().enumerate().take(23).skip(10) {
        assert_eq!(
            *position, 0.0,
            "кадр {frame}: 12 pt пути после нуля значение держится нулём"
        );
    }
    assert_eq!(
        positions[23], -1.0,
        "на выходе значение сдвинулось на 1 pt пути, а не прыгнуло"
    );
    assert_eq!(positions[30], -8.0, "за зоной положение = старт − путь + ширина зоны");
    let largest_step = positions
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0, f64::max);
    assert!(
        largest_step <= 1.0 + 1e-9,
        "ни одного скачка больше пути пальца за кадр: {positions:?}"
    );
    assert_eq!(ruler.value(ms(30 * SLOW_FRAME_MS)), -8.0 / STEPS_PER_ONE);
}

#[test]
fn starting_exactly_on_zero_gives_a_symmetric_zone_in_both_directions() {
    let tokens = UiKitTokens::default();
    for direction in [1.0_f64, -1.0] {
        let mut ruler = ruler_at(&tokens, feel_with(magnet()), 0.0);
        begin(&mut ruler);
        let mut positions = Vec::new();
        for frame in 1..=10 {
            move_finger(&mut ruler, frame, direction * frame as f64);
            positions.push(position_after(&ruler, frame));
        }
        assert!(
            positions[..6].iter().all(|position| *position == 0.0),
            "{direction}: {positions:?}"
        );
        assert_eq!(
            positions[6], -direction,
            "{direction}: после 6 pt нуль отпускает без скачка"
        );
        assert_eq!(positions[9], -4.0 * direction);
    }
}

#[test]
fn the_zone_is_mirrored_when_approaching_zero_from_below() {
    let tokens = UiKitTokens::default();
    // Старт на −10, палец ВЛЕВО (отрицательный путь вправо): значение растёт к нулю.
    let mut ruler = ruler_at(&tokens, feel_with(magnet()), -10.0);
    begin(&mut ruler);
    let mut positions = Vec::new();
    for frame in 1..=25 {
        move_finger(&mut ruler, frame, -(frame as f64));
        positions.push(position_after(&ruler, frame));
    }
    assert_eq!(positions[8], -1.0, "до нуля идёт 1:1");
    assert!(
        positions[9..21].iter().all(|position| *position == 0.0),
        "{positions:?}"
    );
    assert_eq!(positions[22], 1.0, "выход вверх без скачка");
}

// ----- отпускание: ноль и бросок --------------------------------------------------------------------------------

#[test]
fn slow_release_inside_the_zone_lands_exactly_on_zero() {
    let tokens = UiKitTokens::default();
    let mut ruler = ruler_at(&tokens, feel_with(magnet()), 10.0);
    begin(&mut ruler);
    // Палец остановился на 12 pt пути (положение по пальцу −2 шага): внутри зоны.
    for frame in 1..=12 {
        move_finger(&mut ruler, frame, frame as f64);
    }
    ruler
        .apply_pan(ms(13 * SLOW_FRAME_MS), PanUpdate::Ended { velocity: Vector::ZERO })
        .unwrap();
    for frame in 14..=60 {
        ruler.advance(ms(frame * SLOW_FRAME_MS));
    }
    assert_eq!(
        ruler.value(ms(60 * SLOW_FRAME_MS)),
        0.0,
        "отпускание в зоне оседает на нуле"
    );
}

#[test]
fn a_fast_flick_passes_through_zero_without_being_held() {
    let tokens = UiKitTokens::default();
    let flick = |zero_magnet: ZeroMagnet| {
        let mut ruler = ruler_at(&tokens, feel_with(zero_magnet), 10.0);
        begin(&mut ruler);
        // 2 pt пути до отпускания: значение ещё выше нуля; взмах вправо 4000 pt/с уносит его далеко за ноль.
        move_finger(&mut ruler, 1, 1.0);
        move_finger(&mut ruler, 2, 2.0);
        ruler
            .apply_pan(
                ms(2 * SLOW_FRAME_MS + 16),
                PanUpdate::Ended {
                    velocity: Vector::new(4000.0, 0.0),
                },
            )
            .unwrap();
        for frame in 3..=120 {
            ruler.advance(ms(frame * SLOW_FRAME_MS));
        }
        ruler.value(ms(120 * SLOW_FRAME_MS))
    };
    let without = flick(ZeroMagnet::Off);
    let with = flick(magnet());
    assert!(without < -0.2, "взмах без магнита ушёл далеко за ноль: {without}");
    assert!(
        with < -0.2,
        "взмах с магнитом тоже проходит ноль, не залипая на нём: {with}"
    );
}

// ----- щелчки -----------------------------------------------------------------------------------------------------------

#[test]
fn zero_clicks_exactly_once_per_pass_when_the_value_becomes_zero() {
    let tokens = UiKitTokens::default();
    let mut ruler = ruler_at(&tokens, feel_with(magnet()), 20.0);
    begin(&mut ruler);
    let mut clicks_by_frame = Vec::new();
    let mut positions_by_frame = Vec::new();
    // От +20 до −20 шагов значения: путь пальца 20 + 20 + 12 = 52 pt.
    for frame in 1..=52 {
        let events = move_finger(&mut ruler, frame, frame as f64);
        clicks_by_frame.push(reference_clicks(&tokens, &events));
        positions_by_frame.push(position_after(&ruler, frame));
    }
    let total: usize = clicks_by_frame.iter().sum();
    assert_eq!(
        total, 1,
        "один щелчок нуля за проход, а не на входе и на оседании: {clicks_by_frame:?}"
    );
    let click_frame = clicks_by_frame.iter().position(|clicks| *clicks == 1).unwrap() + 1;
    assert!(
        positions_by_frame[click_frame - 1].abs() <= 1.0,
        "щелчок прозвучал, когда значение стало нулём (кадр {click_frame})"
    );
    // Оседание после отпускания новых щелчков нуля не даёт.
    ruler
        .apply_pan(ms(60 * SLOW_FRAME_MS), PanUpdate::Ended { velocity: Vector::ZERO })
        .unwrap();
    let mut after_release = Vec::new();
    for frame in 61..=120 {
        after_release.extend(ruler.advance(ms(frame * SLOW_FRAME_MS)));
    }
    assert_eq!(reference_clicks(&tokens, &after_release), 0);
}

// ----- прежние линейки и ошибки --------------------------------------------------------------------------------------

#[test]
fn default_feel_has_no_magnet_and_the_value_follows_the_finger_one_to_one() {
    assert_eq!(
        RulerFeel::default().zero_magnet,
        ZeroMagnet::Off,
        "по умолчанию магнита нет"
    );
    let tokens = UiKitTokens::default();
    let mut ruler = ruler_at(&tokens, feel_with(ZeroMagnet::Off), 10.0);
    begin(&mut ruler);
    for frame in 1..=25 {
        move_finger(&mut ruler, frame, frame as f64);
        assert_eq!(
            position_after(&ruler, frame),
            10.0 - frame as f64,
            "кадр {frame}: без магнита положение — ровно путь пальца, ноль не держится"
        );
    }
}

#[test]
fn width_zero_means_off_and_invalid_widths_are_rejected_not_ignored() {
    assert_eq!(ZeroMagnet::hold_within(0.0), Ok(ZeroMagnet::Off));
    assert_eq!(
        ZeroMagnet::hold_within(6.0),
        Ok(ZeroMagnet::Hold { half_width_points: 6.0 })
    );
    for invalid in [-1.0, f64::NAN, f64::INFINITY] {
        assert!(
            matches!(
                ZeroMagnet::hold_within(invalid),
                Err(ScaleError::InvalidZeroMagnetWidth { .. })
            ),
            "{invalid}"
        );
    }
}

#[test]
fn magnet_on_a_continuous_ruler_is_refused_while_off_is_accepted() {
    let tokens = UiKitTokens::default();
    let build = |zero_magnet: ZeroMagnet| {
        RulerControl::new(
            RulerSpec {
                model: ContinuousLinear::new(10.0, LinearLimits { min: -50.0, max: 50.0 }).unwrap(),
                density: ScaleDensity::Regular,
                major_every_steps: NonZeroU32::new(5).unwrap(),
                feel: feel_with(zero_magnet),
            },
            0.0,
            &env(&tokens, MotionPreference::Full),
            ms(0),
        )
    };
    assert!(build(ZeroMagnet::Off).is_ok());
    assert!(matches!(
        build(magnet()),
        Err(ScaleError::ZeroMagnetRequiresSteppedModel)
    ));
}
