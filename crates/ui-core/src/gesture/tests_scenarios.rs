//! Сценарии потоков касаний (1–6): точная последовательность состояний и победитель.

use super::test_support::*;
use super::*;
use crate::geometry::Vector;

const VIEW: u32 = 1;

fn expect_pan(event: &GestureEvent<&'static str>) -> (crate::geometry::Point, PanTranslation, Vector) {
    match event.data {
        GestureData::Pan {
            position,
            translation,
            velocity,
        } => (position, translation, velocity),
        other => panic!("ожидали данные пана, получили {other:?}"),
    }
}

/// 1. Тап против пана по порогу сдвига: 9 pt — тап, 11 pt — пан.
#[test]
fn scenario_1_tap_vs_pan_by_movement_threshold() {
    let path = [node(VIEW)];
    // 9 pt и отрыв: тап побеждает, пан проваливается.
    let mut rig = Rig::new();
    let tap_id = rig.add(node(VIEW), tap(), "tap");
    let pan_id = rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.moved(0, 109.0, 100.0, 20);
    assert!(rig.trace().is_empty(), "до порога ничего не решено");
    rig.up(0, 109.0, 100.0, 60);
    assert_eq!(rig.trace(), ["pan:Failed", "tap:Ended"]);
    assert!(rig.arena.is_idle());
    assert_eq!(
        rig.state(tap_id),
        GestureState::Possible,
        "после отрыва всех пальцев — снова Possible"
    );
    assert_eq!(rig.state(pan_id), GestureState::Possible);

    // Ровно 10 pt: порог строгий («больше 10»), это ещё тап.
    rig.down(0, 100.0, 100.0, 100, &path);
    rig.moved(0, 110.0, 100.0, 120);
    rig.up(0, 110.0, 100.0, 160);
    assert_eq!(rig.trace(), ["pan:Failed", "tap:Ended"]);

    // 11 pt: пан начинается, тап проваливается; дальше Changed и Ended.
    rig.down(0, 100.0, 100.0, 200, &path);
    rig.moved(0, 111.0, 100.0, 220);
    assert_eq!(rig.trace(), ["tap:Failed", "pan:Began"]);
    rig.moved(0, 150.0, 100.0, 240);
    rig.up(0, 150.0, 100.0, 260);
    assert_eq!(rig.trace(), ["pan:Changed", "pan:Ended"]);
    assert!(rig.arena.is_idle());
}

/// Ведение 1:1: на `Began` смещение от распознавания 0, от касания = гистерезис; дальше оба растут одинаково.
#[test]
fn pan_reports_both_translations_without_hysteresis_jump() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.moved(0, 112.0, 100.0, 16);
    rig.moved(0, 130.0, 105.0, 32);
    rig.moved(0, 160.0, 120.0, 48);
    let events = rig.events();
    assert_eq!(events.len(), 3);
    let (_, began, _) = expect_pan(&events[0]);
    assert_eq!(events[0].state, GestureState::Began);
    assert_eq!(began.from_recognition, Vector::ZERO);
    assert!(
        (began.from_touch_down.dx - 12.0).abs() < 1e-12,
        "гистерезис пройден: {:?}",
        began.from_touch_down
    );
    let (_, second, _) = expect_pan(&events[1]);
    let (_, third, _) = expect_pan(&events[2]);
    assert!((second.from_recognition.dx - 18.0).abs() < 1e-12 && (second.from_recognition.dy - 5.0).abs() < 1e-12);
    assert!((third.from_recognition.dx - 48.0).abs() < 1e-12 && (third.from_recognition.dy - 20.0).abs() < 1e-12);
    // Разность двух смещений постоянна: они растут одинаково 1:1 с пальцем.
    for translation in [second, third] {
        let gap_dx = translation.from_touch_down.dx - translation.from_recognition.dx;
        let gap_dy = translation.from_touch_down.dy - translation.from_recognition.dy;
        assert!(
            (gap_dx - 12.0).abs() < 1e-12 && gap_dy.abs() < 1e-12,
            "{gap_dx} {gap_dy}"
        );
    }
}

/// Скорость пана идёт из оценщика; пауза перед отрывом даёт ноль.
#[test]
fn pan_velocity_follows_finger_and_is_zero_after_pause_before_lift() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    rig.down(0, 0.0, 0.0, 0, &path);
    for step in 1..=10 {
        rig.moved(0, 8.0 * f64::from(step), 0.0, 8 * i64::from(step)); // 1000 pt/с
    }
    let moving = rig.events();
    let (_, _, velocity) = expect_pan(moving.last().expect("события"));
    assert!((velocity.dx - 1000.0).abs() < 1.0, "{velocity:?}");

    // Палец замер на 200 мс (идут только кадры), потом отрыв: Ended со скоростью 0.
    rig.frame(150);
    rig.frame(280);
    rig.up(0, 80.0, 0.0, 280);
    let ended = rig.events();
    assert_eq!(ended.last().map(|event| event.state), Some(GestureState::Ended));
    let (_, _, velocity) = expect_pan(ended.last().expect("Ended"));
    assert_eq!(velocity, Vector::ZERO);
}

/// 2. Долгий тап по времени: палец стоит, идут только кадры.
#[test]
fn scenario_2_long_press_fires_on_first_frame_at_or_after_half_second() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), long_press(), "long");
    rig.down(0, 50.0, 50.0, 0, &path);
    for frame_ms in (16..500).step_by(16) {
        rig.frame(frame_ms);
    }
    assert!(rig.trace().is_empty(), "до 0,5 с ничего");
    rig.frame(496);
    assert!(rig.trace().is_empty());
    rig.frame(512);
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].state, events[0].time), (GestureState::Began, at(512)));
    // Дальше: кадры молчат, отрыв — Ended.
    rig.frame(528);
    assert!(rig.trace().is_empty());
    rig.up(0, 50.0, 50.0, 540);
    assert_eq!(rig.trace(), ["long:Ended"]);

    // Кадр ровно на 500 мс тоже запускает («не меньше»).
    rig.down(0, 50.0, 50.0, 1000, &path);
    rig.frame(1499);
    assert!(rig.trace().is_empty());
    rig.frame(1500);
    assert_eq!(rig.trace(), ["long:Began"]);
    rig.up(0, 50.0, 50.0, 1600);
    rig.trace();

    // Сдвиг 11 pt до 0,5 с — провал, кадры дальше ничего не запускают.
    rig.down(0, 50.0, 50.0, 2000, &path);
    rig.moved(0, 61.0, 50.0, 2300);
    assert_eq!(rig.trace(), ["long:Failed"]);
    rig.frame(2600);
    assert!(rig.trace().is_empty());

    // Сдвиг 9 pt — допустим: жест всё равно начнётся.
    rig.up(0, 61.0, 50.0, 2650);
    rig.down(0, 50.0, 50.0, 3000, &path);
    rig.moved(0, 59.0, 50.0, 3100);
    rig.frame(3500);
    assert_eq!(rig.trace(), ["long:Began"]);
}

/// 3. Щипок с отрывом одного пальца; оставшийся палец ничего не начинает; новое касание работает.
#[test]
fn scenario_3_pinch_then_one_finger_lift_leaves_no_phantom_gestures() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), tap(), "tap");
    rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    rig.add(node(VIEW), pinch(), "pinch");
    rig.down(0, 100.0, 200.0, 0, &path);
    rig.down(1, 200.0, 200.0, 10, &path);
    assert_eq!(
        rig.trace(),
        ["tap:Failed", "pan:Failed"],
        "2 пальца: однопальцевые сразу провалены"
    );
    rig.moved(1, 220.0, 200.0, 20); // расстояние 120: изменение 20 ≥ 10
    let began = rig.events();
    assert_eq!(began.len(), 1);
    assert_eq!(began[0].state, GestureState::Began);
    match began[0].data {
        GestureData::Pinch { scale, focus, .. } => {
            assert!((scale - 1.2).abs() < 1e-12);
            assert!((focus.x - 160.0).abs() < 1e-12 && (focus.y - 200.0).abs() < 1e-12);
        }
        other => panic!("ожидали щипок: {other:?}"),
    }
    rig.moved(1, 230.0, 200.0, 30);
    let changed = rig.events();
    match changed[0].data {
        // Масштаб растёт (1,0 → 1,2 → 1,3), значит скорость масштаба положительна.
        GestureData::Pinch {
            scale, scale_velocity, ..
        } => {
            assert!((scale - 1.3).abs() < 1e-12);
            assert!(scale_velocity > 0.0, "масштаб растёт: {scale_velocity}");
        }
        other => panic!("ожидали щипок: {other:?}"),
    }
    rig.up(0, 100.0, 200.0, 40);
    assert_eq!(rig.trace(), ["pinch:Ended"]);
    // Оставшийся палец уходит на 100 pt: никакого пана.
    rig.moved(1, 330.0, 200.0, 60);
    rig.moved(1, 330.0, 300.0, 80);
    assert!(rig.trace().is_empty(), "фантомного пана нет");
    rig.up(1, 330.0, 300.0, 100);
    assert!(rig.trace().is_empty(), "и фантомного тапа тоже");
    assert!(rig.arena.is_idle());

    // Новое касание после полного отрыва работает как обычно.
    rig.down(0, 100.0, 100.0, 200, &path);
    rig.moved(0, 120.0, 100.0, 220);
    assert_eq!(rig.trace(), ["tap:Failed", "pan:Began", "pinch:Failed"]);
}

/// 4. Свайп режимов (горизонтальный пан) против вертикального пана экспозиции: решает доминирующая ось.
#[test]
fn scenario_4_horizontal_vs_vertical_pan_by_dominant_axis() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), pan(PanDirection::Horizontal), "modes");
    rig.add(node(VIEW), pan(PanDirection::Vertical), "exposure");
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.moved(0, 115.0, 104.0, 16);
    assert_eq!(rig.trace(), ["exposure:Failed", "modes:Began"]);
    rig.up(0, 115.0, 104.0, 32);
    assert_eq!(rig.trace(), ["modes:Ended"]);

    rig.down(0, 100.0, 100.0, 100, &path);
    rig.moved(0, 104.0, 85.0, 116);
    assert_eq!(rig.trace(), ["modes:Failed", "exposure:Began"]);
    rig.moved(0, 104.0, 60.0, 132);
    assert_eq!(rig.trace(), ["exposure:Changed"]);
    rig.up(0, 104.0, 60.0, 148);
    assert_eq!(rig.trace(), ["exposure:Ended"]);

    // Ничья по модулям: горизонталь (документированное правило).
    rig.down(0, 100.0, 100.0, 200, &path);
    rig.moved(0, 110.0, 110.0, 216);
    assert_eq!(rig.trace(), ["exposure:Failed", "modes:Began"]);
}

/// 5. Системная отмена посреди пана и посреди Possible; дыра в потоке.
#[test]
fn scenario_5_system_cancel_and_stream_gap() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), tap(), "tap");
    rig.add(node(VIEW), pan(PanDirection::Any), "pan");

    // Отмена посреди Possible: всё проваливается.
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.system_cancel(0, 100.0, 100.0, 30);
    assert_eq!(rig.trace(), ["tap:Failed", "pan:Failed"]);
    assert!(rig.arena.is_idle());

    // Отмена посреди пана: Cancelled с последними данными.
    rig.down(0, 100.0, 100.0, 100, &path);
    rig.moved(0, 140.0, 100.0, 120);
    rig.trace();
    rig.system_cancel(0, 140.0, 100.0, 140);
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].state, GestureState::Cancelled);
    let (position, _, _) = expect_pan(&events[0]);
    assert_eq!(position, crate::geometry::Point::new(140.0, 100.0));
    assert!(rig.arena.is_idle());

    // Дыра: то же самое, а запоздавшие касания того же пальца игнорируются.
    rig.down(0, 100.0, 100.0, 200, &path);
    rig.moved(0, 140.0, 100.0, 220);
    rig.trace();
    rig.arena.handle_stream_gap();
    assert_eq!(rig.trace(), ["pan:Cancelled"]);
    assert_eq!(
        rig.touch(0, shell_contract::touch::TouchPhase::Moved, 150.0, 100.0, 240, &[]),
        PointerDisposition::IgnoredUnknownPointer
    );
    assert!(rig.arena.is_idle());
    // После дыры жесты работают заново.
    rig.down(0, 10.0, 10.0, 300, &path);
    rig.up(0, 10.0, 10.0, 330);
    assert_eq!(rig.trace(), ["pan:Failed", "tap:Ended"]);
}

/// 6. Кнопка зума: тап против (долгий тап → перетаскивание тем же касанием) против пана.
#[test]
fn scenario_6_zoom_button_tap_vs_long_press_drag_vs_pan() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), tap(), "tap");
    rig.add(node(VIEW), long_press(), "wheel_hold");
    rig.add(node(VIEW), pan(PanDirection::Any), "wheel_pan");

    // Быстрый тап.
    rig.down(0, 100.0, 500.0, 0, &path);
    rig.up(0, 100.0, 500.0, 80);
    assert_eq!(rig.trace(), ["wheel_hold:Failed", "wheel_pan:Failed", "tap:Ended"]);

    // Удержание 0,5 с, затем ведение пальца: тот же жест идёт Changed, пан и тап провалены.
    rig.down(0, 100.0, 500.0, 1000, &path);
    rig.frame(1512);
    assert_eq!(rig.trace(), ["wheel_hold:Began", "tap:Failed", "wheel_pan:Failed"]);
    rig.moved(0, 140.0, 500.0, 1530);
    rig.moved(0, 90.0, 500.0, 1546);
    let drag = rig.events();
    assert_eq!(
        drag.iter().map(|event| event.state).collect::<Vec<_>>(),
        [GestureState::Changed; 2]
    );
    match (drag[0].data, drag[1].data) {
        (
            GestureData::LongPress { translation: first, .. },
            GestureData::LongPress {
                translation: second, ..
            },
        ) => {
            assert_eq!(first.from_touch_down, Vector::new(40.0, 0.0));
            assert_eq!(
                first.from_recognition,
                Vector::new(40.0, 0.0),
                "палец стоял на месте до Began"
            );
            assert_eq!(second.from_touch_down, Vector::new(-10.0, 0.0));
        }
        other => panic!("ожидали долгий тап: {other:?}"),
    }
    rig.up(0, 90.0, 500.0, 1600);
    assert_eq!(rig.trace(), ["wheel_hold:Ended"]);

    // Быстрое перетаскивание (> 10 pt до 0,5 с) — пан; долгий тап проваливается.
    rig.down(0, 100.0, 500.0, 2000, &path);
    rig.moved(0, 125.0, 500.0, 2100);
    assert_eq!(rig.trace(), ["tap:Failed", "wheel_hold:Failed", "wheel_pan:Began"]);
}
