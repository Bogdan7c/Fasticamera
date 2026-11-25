//! Шкалы карточки белого следуют за фактами (WB21) и ведутся пальцем без скачков: настоящие касания, кадры и факты.
//! Смотрим результат — положение шкал и намерения, которые ушли камере.

use camera_contract::white::{WhiteAdjustment, WhiteOrigin};
use shell_contract::haptics::HapticPattern;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::Point;

use super::test_rig::*;
use super::white_test_support::*;
use crate::action::ScreenAction;
use crate::effect::{Effect, ScreenCameraIntent};
use crate::white::WhiteAxis;
use crate::white_facts::{WhiteFacts, WhiteRange};

/// Положение шкалы Кельвинов для `kelvin` (отрицательные майреды).
fn mired_position(kelvin: f64) -> f64 {
    -1_000_000.0 / kelvin
}

fn position(rig: &Rig, axis: WhiteAxis) -> f64 {
    rig.screen.white_scale_position(axis).expect("шкалы есть")
}

fn select_axis(rig: &mut Rig, axis: WhiteAxis) {
    let targets = card_targets(rig);
    let number = match axis {
        WhiteAxis::Temperature => targets.temperature_number,
        WhiteAxis::Tint => targets.tint_number,
    };
    rig.tap(number);
    rig.effects();
}

/// Ждать, пока шкала Кельвинов перестанет быть «ведомой пальцем» (инерция после отпускания кончилась), и
/// собрать все эффекты за это время.
fn settle(rig: &mut Rig) -> Vec<Effect> {
    let mut collected = Vec::new();
    for _ in 0..1_200 {
        rig.run(FRAME_MS);
        collected.extend(rig.effects());
        if !rig.screen.white_driven() {
            break;
        }
    }
    assert!(!rig.screen.white_driven(), "шкала так и не успокоилась");
    collected
}

/// Палец ведёт по шкале вбок на `step_pt` за кадр `steps` раз, потом отрывается. Возвращает значения намерений
/// после КАЖДОГО кадра (пусто — на кадре ничего не ушло).
fn drag_by_steps(rig: &mut Rig, from: Point, step_pt: f64, steps: usize) -> Vec<Vec<Effect>> {
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, from, start);
    let mut per_frame = Vec::new();
    for step in 1..=steps {
        let point = Point::new(from.x + step_pt * step as f64, from.y);
        rig.touch(TouchPhase::Moved, point, start + step as i64 * FRAME_MS);
        rig.now_ms = start + step as i64 * FRAME_MS;
        rig.frame();
        per_frame.push(rig.effects());
    }
    let end = Point::new(from.x + step_pt * steps as f64, from.y);
    rig.touch(TouchPhase::Ended, end, rig.now_ms + 1);
    per_frame
}

fn tint_values(frames: &[Vec<Effect>]) -> Vec<f64> {
    camera_intents(&frames.concat())
        .iter()
        .filter_map(|intent| match intent {
            ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Tint(tint)) => Some(tint.duv()),
            _ => None,
        })
        .collect()
}

fn kelvin_values(frames: &[Vec<Effect>]) -> Vec<f64> {
    camera_intents(&frames.concat())
        .iter()
        .filter_map(|intent| match intent {
            ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Temperature(temperature)) => Some(temperature.kelvin()),
            _ => None,
        })
        .collect()
}

// ---- следование за фактами ----

#[test]
fn both_scales_stand_exactly_on_3140_k_and_0_0164_whatever_the_card_is_open_or_not() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(3_140.0, 0.0164, WhiteOrigin::Manual));
    rig.run(900);
    // Шкалы следуют и при закрытой карточке: открытие застаёт их на месте.
    assert!((position(&rig, WhiteAxis::Temperature) - mired_position(3_140.0)).abs() < 0.05);
    assert!((position(&rig, WhiteAxis::Tint) - 16.4).abs() < 0.05);
    open_card(&mut rig);
    assert!((position(&rig, WhiteAxis::Temperature) - mired_position(3_140.0)).abs() < 0.05);
    assert!(
        (position(&rig, WhiteAxis::Tint) - 16.4).abs() < 0.05,
        "ровно 16,4 единицы, а не округлённые 16"
    );
    assert!(rig.effects().iter().all(|effect| !matches!(effect, Effect::Camera(_))));
}

#[test]
fn opening_closing_switching_axes_and_new_facts_never_send_an_intent_or_buzz() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(3_140.0, 0.0164, WhiteOrigin::Manual));
    open_card(&mut rig);
    select_axis(&mut rig, WhiteAxis::Tint);
    // Белое сменилось «снаружи» на другое далеко: обе шкалы едут к нему сотни тиков.
    set_facts(&mut rig, facts_held(7_500.0, -0.02, WhiteOrigin::Manual));
    rig.run(1_500);
    select_axis(&mut rig, WhiteAxis::Temperature);
    rig.act(ScreenAction::WhiteButtonTapped);
    rig.run(700);
    open_card(&mut rig);
    rig.run(300);
    let effects = rig.effects();
    assert!(effects.is_empty(), "намерений и вибрации нет: {effects:?}");
    assert!((position(&rig, WhiteAxis::Temperature) - mired_position(7_500.0)).abs() < 0.05);
    assert!((position(&rig, WhiteAxis::Tint) + 20.0).abs() < 0.05);
}

// ---- ведение пальцем ----

#[test]
fn the_first_pixel_costs_exactly_one_pixel_of_the_scale_and_there_is_no_jump_on_the_tint_axis() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(3_140.0, 0.0164, WhiteOrigin::Manual));
    open_card(&mut rig);
    select_axis(&mut rig, WhiteAxis::Tint);
    let zone = card_targets(&rig).ruler_zone;
    let frames = drag_by_steps(&mut rig, zone.center(), -1.0, 40);
    let values = tint_values(&frames);
    assert!(values.len() > 10, "шкала отдаёт значения: {}", values.len());
    // 1 pt = 1/2 единицы оттенка (плотность 2 pt на единицу) = 0,0005 Duv.
    let price = 0.0005;
    // Без скачка: первое значение от точного старта (0,0164), а не от округлённого и не от положения пальца.
    assert!(
        values[0] > 0.0164 && values[0] - 0.0164 < price * 14.0,
        "первое значение {}",
        values[0]
    );
    for pair in values.windows(2) {
        assert!(
            ((pair[1] - pair[0]) - price).abs() < 1e-4,
            "шаг {} вместо {price}",
            pair[1] - pair[0]
        );
    }
    // Второй оси в намерении нет: ни температуры, ни «белое целиком».
    let all = camera_intents(&frames.concat());
    assert!(
        all.iter()
            .all(|intent| matches!(intent, ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Tint(_)))),
        "{all:?}"
    );
}

#[test]
fn the_first_pixel_costs_one_pixel_of_mired_on_the_kelvin_axis_and_only_the_temperature_goes_out() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(3_140.0, 0.0164, WhiteOrigin::Manual));
    open_card(&mut rig);
    let zone = card_targets(&rig).ruler_zone;
    let frames = drag_by_steps(&mut rig, zone.center(), -1.0, 40);
    let values = kelvin_values(&frames);
    assert!(values.len() > 10);
    // Палец влево — Кельвины растут; 1 pt = 0,5 майреда (2 pt на майред).
    for pair in values.windows(2) {
        let mired_step = 1_000_000.0 / pair[0] - 1_000_000.0 / pair[1];
        assert!((mired_step - 0.5).abs() < 0.01, "шаг {mired_step} майред");
    }
    assert!(values[0] > 3_140.0);
    let first_mired_jump = 1_000_000.0 / 3_140.0 - 1_000_000.0 / values[0];
    assert!(
        first_mired_jump < 7.0,
        "первое значение не прыгает: {first_mired_jump} майред"
    );
    assert!(
        camera_intents(&frames.concat())
            .iter()
            .all(|intent| matches!(intent, ScreenCameraIntent::AdjustWhite(WhiteAdjustment::Temperature(_))))
    );
}

#[test]
fn finger_left_raises_the_tint_to_purple_and_finger_right_lowers_it() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    select_axis(&mut rig, WhiteAxis::Tint);
    let zone = card_targets(&rig).ruler_zone;
    let left = tint_values(&drag_by_steps(&mut rig, zone.center(), -2.0, 20));
    assert!(
        left.last().copied().unwrap_or(0.0) > 0.01,
        "влево — плюс (к пурпурному): {left:?}"
    );
    settle(&mut rig);
    // Движок принял белое, шкала стоит на нём; затем кто-то другой вернул оттенок в ноль.
    set_facts(&mut rig, facts_held(5_600.0, 0.045, WhiteOrigin::Manual));
    set_facts(&mut rig, facts_held(5_600.0, 0.0, WhiteOrigin::Manual));
    rig.run(1_500);
    rig.effects();
    let right = tint_values(&drag_by_steps(&mut rig, zone.center(), 2.0, 20));
    assert!(
        right.last().copied().unwrap_or(0.0) < -0.01,
        "вправо — минус: {right:?}"
    );
}

#[test]
fn the_scale_limits_come_from_the_facts_and_change_on_the_fly() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    let narrow = WhiteFacts {
        range: WhiteRange {
            min_kelvin: 3_000.0,
            max_kelvin: 6_000.0,
            max_abs_duv: 0.02,
        },
        ..WhiteFacts::STARTING
    };
    set_facts(&mut rig, facts_with(narrow));
    rig.run(600);
    let zone = card_targets(&rig).ruler_zone;
    // Долго тянем влево: Кельвины растут, но выше нового предела не уходят.
    let frames = drag_by_steps(&mut rig, zone.center(), -6.0, 60);
    let kelvin = kelvin_values(&frames);
    assert!(!kelvin.is_empty());
    assert!(
        kelvin.iter().all(|value| *value <= 6_000.0 + 1e-6),
        "выше предела: {:?}",
        kelvin.last()
    );
    assert!(kelvin.iter().any(|value| *value > 5_900.0), "дошли до нового предела");
}

// ---- гонка: устаревшие факты ----

/// Палец ведёт шкалу влево `steps` кадров, замирает на 200 мс и отрывается: инерции нет, шкала успокаивается сразу.
/// Возвращает все отправленные значения Кельвинов по порядку.
fn drag_hold_and_release(rig: &mut Rig, from: Point, steps: usize) -> Vec<f64> {
    let mut sent = Vec::new();
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, from, start);
    let mut frames = Vec::new();
    for step in 1..=steps {
        rig.touch(
            TouchPhase::Moved,
            Point::new(from.x - 2.0 * step as f64, from.y),
            start + step as i64 * FRAME_MS,
        );
        rig.now_ms = start + step as i64 * FRAME_MS;
        rig.frame();
        frames.push(rig.effects());
    }
    // Стоп: палец лежит неподвижно 200 мс.
    for _ in 0..13 {
        rig.now_ms += FRAME_MS;
        rig.frame();
        frames.push(rig.effects());
    }
    rig.touch(
        TouchPhase::Ended,
        Point::new(from.x - 2.0 * steps as f64, from.y),
        rig.now_ms,
    );
    frames.push(settle(rig));
    sent.extend(kelvin_values(&frames));
    sent
}

#[test]
fn a_stale_fact_after_release_never_jerks_the_scale_back_and_a_later_foreign_fact_is_followed() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(5_000.0, 0.0, WhiteOrigin::Manual));
    open_card(&mut rig);
    let zone = card_targets(&rig).ruler_zone;
    // Палец отпущен на K2 (инерции нет); движок отстаёт: сначала приходит устаревшее K1, затем K2.
    let sent = drag_hold_and_release(&mut rig, zone.center(), 30);
    let k2 = *sent.last().expect("значение");
    let k1 = sent[sent.len() - 2];
    assert!(
        (mired_position(k1) - mired_position(k2)).abs() > 0.3,
        "K1 и K2 различимы: {k1} / {k2}"
    );
    let rest = position(&rig, WhiteAxis::Temperature);
    assert!((rest - mired_position(k2)).abs() < 0.05, "шкала стоит на K2");
    set_facts(&mut rig, facts_held(k1, 0.0, WhiteOrigin::Manual));
    assert!(
        (position(&rig, WhiteAxis::Temperature) - rest).abs() < 0.01,
        "шкала дёрнулась назад на устаревший факт K1"
    );
    set_facts(&mut rig, facts_held(k2, 0.0, WhiteOrigin::Manual));
    rig.run(500);
    assert!(
        (position(&rig, WhiteAxis::Temperature) - rest).abs() < 0.01,
        "и на K2 она не двигается"
    );
    // Чужое значение (кнопка света): шкала догоняет и намерений не шлёт.
    set_facts(&mut rig, facts_held(2_700.0, 0.0, WhiteOrigin::Manual));
    rig.run(1_500);
    assert!((position(&rig, WhiteAxis::Temperature) - mired_position(2_700.0)).abs() < 0.05);
    assert!(camera_intents(&rig.effects()).is_empty(), "догонять — не намерение");
}

#[test]
fn an_old_sent_value_that_the_facts_never_confirmed_expires_and_is_then_a_foreign_edit() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(5_000.0, 0.0, WhiteOrigin::Manual));
    open_card(&mut rig);
    let zone = card_targets(&rig).ruler_zone;
    let sent = drag_hold_and_release(&mut rig, zone.center(), 30);
    let k_old = sent[sent.len() - 2];
    // Факты молчали дольше срока памяти; потом кнопка света ставит ровно то же число — это чужая правка.
    rig.run(1_500);
    set_facts(&mut rig, facts_held(k_old, 0.0, WhiteOrigin::Manual));
    rig.run(1_500);
    assert!((position(&rig, WhiteAxis::Temperature) - mired_position(k_old)).abs() < 0.05);
}

#[test]
fn an_engine_that_lags_a_frame_behind_the_finger_never_pulls_the_scale_back_during_the_drag() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(5_000.0, 0.0, WhiteOrigin::Manual));
    open_card(&mut rig);
    let zone = card_targets(&rig).ruler_zone;
    let mut previous = position(&rig, WhiteAxis::Temperature);
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, zone.center(), start);
    let mut lagged: Option<f64> = None;
    for step in 1..=40_i64 {
        rig.touch(
            TouchPhase::Moved,
            Point::new(zone.center().x - 2.0 * step as f64, zone.center().y),
            start + step * FRAME_MS,
        );
        rig.now_ms = start + step * FRAME_MS;
        rig.frame();
        let now_position = position(&rig, WhiteAxis::Temperature);
        assert!(
            now_position >= previous - 1e-9,
            "шаг {step}: шкала откатилась {previous} → {now_position}"
        );
        previous = now_position;
        let values = kelvin_values(&[rig.effects()]);
        if let Some(stale) = lagged {
            rig.screen.handle_action(ScreenAction::CameraFactsChanged(facts_held(
                stale,
                0.0,
                WhiteOrigin::Manual,
            )));
        }
        lagged = values.last().copied().or(lagged);
    }
    assert!(previous > mired_position(5_000.0) + 20.0, "палец сдвинул шкалу");
}

#[test]
fn facts_are_ignored_while_the_finger_leads_and_a_foreign_one_is_followed_after_the_scale_settles() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(5_000.0, 0.0, WhiteOrigin::Manual));
    open_card(&mut rig);
    let zone = card_targets(&rig).ruler_zone;
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, zone.center(), start);
    for step in 1..=20 {
        rig.touch(
            TouchPhase::Moved,
            Point::new(zone.center().x - 2.0 * f64::from(step), zone.center().y),
            start + i64::from(step) * FRAME_MS,
        );
        rig.now_ms = start + i64::from(step) * FRAME_MS;
        rig.frame();
    }
    rig.effects();
    let under_finger = position(&rig, WhiteAxis::Temperature);
    // Палец ещё лежит; приходит чужое белое.
    set_facts(&mut rig, facts_held(2_700.0, 0.0, WhiteOrigin::Manual));
    assert!(
        (position(&rig, WhiteAxis::Temperature) - under_finger).abs() < 1.0,
        "пока палец ведёт, факты игнорируются"
    );
    rig.touch(
        TouchPhase::Ended,
        Point::new(zone.center().x - 40.0, zone.center().y),
        rig.now_ms + 1,
    );
    settle(&mut rig);
    rig.run(2_000);
    assert!(
        (position(&rig, WhiteAxis::Temperature) - mired_position(2_700.0)).abs() < 0.2,
        "после того как шкала успокоилась, она догнала факт, сменившийся на другое"
    );
}

// ---- итоговая сверка и закрытие посреди ведения ----

#[test]
fn closing_the_card_in_the_middle_of_a_drag_stops_the_intents_and_the_reopened_scale_stands_on_the_fact() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(5_000.0, 0.0, WhiteOrigin::Manual));
    open_card(&mut rig);
    let zone = card_targets(&rig).ruler_zone;
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, zone.center(), start);
    for step in 1..=40_i64 {
        rig.touch(
            TouchPhase::Moved,
            Point::new(zone.center().x - 2.0 * step as f64, zone.center().y),
            start + step * FRAME_MS,
        );
        rig.now_ms = start + step * FRAME_MS;
        rig.frame();
        if step == 20 {
            // Карточку закрыли (кнопка «×» второй рукой): дальше намерения редьюсер отверг бы.
            rig.screen.handle_action(ScreenAction::SubmenuBackRequested);
            rig.effects();
        } else if step > 20 {
            let after_close = camera_intents(&rig.effects());
            assert!(
                after_close.is_empty(),
                "шаг {step}: после закрытия ушло намерение {after_close:?}"
            );
        }
    }
    rig.touch(
        TouchPhase::Ended,
        Point::new(zone.center().x - 80.0, zone.center().y),
        rig.now_ms + 1,
    );
    settle(&mut rig);
    assert_eq!(rig.screen.state().panel(), crate::state::PanelState::Closed);
    // Движок ничего не получил после закрытия; факт прежний. Через память отправленного шкала встаёт на факт.
    rig.run(1_500);
    open_card(&mut rig);
    rig.run(1_500);
    assert!(
        (position(&rig, WhiteAxis::Temperature) - mired_position(5_000.0)).abs() < 0.1,
        "шкала стоит на {} вместо факта {}",
        position(&rig, WhiteAxis::Temperature),
        mired_position(5_000.0)
    );
    assert!(camera_intents(&rig.effects()).is_empty());
}

#[test]
fn a_light_button_that_equals_a_value_the_finger_sent_still_wins_once_the_memory_runs_out() {
    let mut rig = Rig::new();
    set_facts(&mut rig, facts_held(5_000.0, 0.0, WhiteOrigin::Manual));
    open_card(&mut rig);
    let zone = card_targets(&rig).ruler_zone;
    let sent = drag_hold_and_release(&mut rig, zone.center(), 30);
    // Кнопка света ставит ровно то число, через которое прошёл палец (оно ещё в памяти): сперва похоже на эхо.
    let button_kelvin = sent[sent.len() / 2];
    let rest = position(&rig, WhiteAxis::Temperature);
    assert!(
        (mired_position(button_kelvin) - rest).abs() > 3.0,
        "кнопка заметно далеко от места шкалы"
    );
    set_facts(&mut rig, facts_held(button_kelvin, 0.0, WhiteOrigin::Manual));
    // Память (600 мс) ещё держит значение: шкалу не дёргаем.
    assert!((position(&rig, WhiteAxis::Temperature) - rest).abs() < 0.01);
    rig.run(1_500);
    assert!(
        (position(&rig, WhiteAxis::Temperature) - mired_position(button_kelvin)).abs() < 0.1,
        "после срока памяти шкала стоит на кнопке"
    );
    assert!(camera_intents(&rig.effects()).is_empty(), "сверка — не намерение");
}

// ---- программное движение не вибрирует ----

#[test]
fn a_programmatic_move_of_the_scale_plays_no_tick_haptics_but_the_finger_does() {
    let mut rig = Rig::new();
    open_card(&mut rig);
    set_facts(&mut rig, facts_held(11_000.0, 0.04, WhiteOrigin::Manual));
    rig.run(1_500);
    let haptics = rig
        .effects()
        .into_iter()
        .filter(|e| matches!(e, Effect::Haptic(_)))
        .count();
    assert_eq!(haptics, 0, "шкала сама поехала — вибрации нет");
    let zone = card_targets(&rig).ruler_zone;
    // Вправо: Кельвины падают от 11 000 К, до предела 12 000 К шкала не упирается.
    let mut during = drag_by_steps(&mut rig, zone.center(), 3.0, 30).concat();
    during.extend(settle(&mut rig));
    let ticks = during
        .into_iter()
        .filter(|e| matches!(e, Effect::Haptic(HapticPattern::SelectionTick)))
        .count();
    assert!(
        ticks > 3,
        "палец ведёт шкалу через риски — тики вибрации как у EV: {ticks}"
    );
}
