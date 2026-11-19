//! Сценарии потоков касаний (7–12): двойной тап, нажатие, удаление, глубина, детерминизм, затвор.

use super::test_support::*;
use super::*;
use crate::geometry::Point;

const VIEW: u32 = 1;
const BUTTON: u32 = 2;

/// Единичный тап `requires toFail` двойного.
fn single_and_double(rig: &mut Rig, element: crate::id::NodeId) -> (RecognizerId, RecognizerId) {
    let single = rig.add(element, tap(), "single");
    let double = rig.add(element, double_tap(), "double");
    rig.arena.require_to_fail(single, double).expect("отношение верно");
    (single, double)
}

/// 7a. Одиночный срабатывает только после истечения интервала (по кадрам).
#[test]
fn scenario_7_single_tap_waits_for_double_interval_by_frames() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    let (single, _) = single_and_double(&mut rig, node(VIEW));
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.up(0, 100.0, 100.0, 50);
    assert!(rig.trace().is_empty(), "одиночный ждёт: двойной ещё возможен");
    assert_eq!(rig.state(single), GestureState::Possible);
    assert!(!rig.arena.is_idle(), "арена ждёт таймер");
    rig.frame(200);
    rig.frame(350); // ровно 300 мс после отрыва: допустимо
    assert!(rig.trace().is_empty());
    rig.frame(351);
    let events = rig.events();
    assert_eq!(
        events
            .iter()
            .map(|event| (event.intent, event.state, event.time))
            .collect::<Vec<_>>(),
        [
            ("double", GestureState::Failed, at(351)),
            ("single", GestureState::Ended, at(351))
        ]
    );
    assert!(matches!(events[1].data, GestureData::Tap { tap_count: 1, .. }));
    assert!(rig.arena.is_idle(), "после срабатывания арена свободна");
}

/// 7b. Двойной тап: сработает двойной, одиночный не срабатывает никогда.
#[test]
fn scenario_7_double_tap_suppresses_single() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    let (single, _) = single_and_double(&mut rig, node(VIEW));
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.up(0, 100.0, 100.0, 50);
    // Второе касание приходит в уже «удерживаемую» последовательность: путь хит-теста не нужен.
    rig.down(0, 104.0, 102.0, 150, &path);
    assert!(rig.trace().is_empty());
    rig.up(0, 104.0, 102.0, 200);
    let events = rig.events();
    assert_eq!(
        events
            .iter()
            .map(|event| (event.intent, event.state))
            .collect::<Vec<_>>(),
        [("double", GestureState::Ended), ("single", GestureState::Failed)]
    );
    assert!(matches!(events[0].data, GestureData::Tap { tap_count: 2, .. }));
    rig.frame(1000);
    assert!(rig.trace().is_empty(), "одиночный тап так и не сработал");
    assert_eq!(rig.state(single), GestureState::Possible);
    assert!(rig.arena.is_idle());
}

/// 7c. Второе касание слишком далеко или слишком поздно (метка касания, без кадра) — одиночный срабатывает.
#[test]
fn scenario_7_second_touch_too_far_or_too_late_releases_single() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    single_and_double(&mut rig, node(VIEW));
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.up(0, 100.0, 100.0, 50);
    rig.down(0, 300.0, 100.0, 120, &path); // 200 pt от первого тапа > 100
    assert_eq!(rig.trace(), ["double:Failed", "single:Ended"]);
    rig.up(0, 300.0, 100.0, 150);
    assert!(
        rig.trace().is_empty(),
        "второе касание — не новый тап: распознаватели уже завершены"
    );
    assert!(rig.arena.is_idle());

    // Слишком поздно: кадра не было, интервал проверяется по метке самого касания.
    rig.down(0, 100.0, 100.0, 1000, &path);
    rig.up(0, 100.0, 100.0, 1040);
    rig.down(0, 100.0, 100.0, 1400, &path); // 360 мс > 300
    assert_eq!(rig.trace(), ["double:Failed", "single:Ended"]);
}

/// Блокер, которого нет среди кандидатов касания, не блокирует.
#[test]
fn require_to_fail_ignores_blocker_that_is_not_a_candidate() {
    let mut rig = Rig::new();
    let single = rig.add(node(VIEW), tap(), "single");
    let elsewhere_double = rig.add(node(99), double_tap(), "elsewhere_double");
    rig.arena
        .require_to_fail(single, elsewhere_double)
        .expect("отношение верно");
    rig.down(0, 10.0, 10.0, 0, &[node(VIEW)]);
    rig.up(0, 10.0, 10.0, 40);
    assert_eq!(
        rig.trace(),
        ["single:Ended"],
        "двойной тап чужого элемента не задерживает"
    );
}

/// Удаление блокера освобождает ждущего.
#[test]
fn removing_the_blocker_releases_the_waiting_recognizer() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    let (_, double) = single_and_double(&mut rig, node(VIEW));
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.up(0, 100.0, 100.0, 50);
    assert!(rig.trace().is_empty());
    rig.arena.remove_recognizer(double).expect("известен");
    assert_eq!(rig.trace(), ["single:Ended"]);
    assert!(rig.arena.is_idle());
}

/// Отмена системой во время ожидания: ждущий тап теряется (Failed).
#[test]
fn system_cancel_while_waiting_fails_the_pending_single() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    single_and_double(&mut rig, node(VIEW));
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.up(0, 100.0, 100.0, 50);
    rig.system_cancel(0, 100.0, 100.0, 80);
    assert_eq!(rig.trace(), ["single:Failed", "double:Failed"]);
    assert!(rig.arena.is_idle());
}

/// 8. Press: Began сразу, Changed при уходе/возврате, Cancelled при пане родителя, Ended при отрыве.
#[test]
fn scenario_8_press_highlights_immediately_and_is_cancelled_by_parent_pan() {
    let path = [node(BUTTON), node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(BUTTON), press(), "press");
    rig.add(node(VIEW), pan(PanDirection::Vertical), "scroll");

    // Обычное нажатие: подсветка сразу, палец уходит и возвращается, отрыв — Ended.
    rig.down(0, 50.0, 50.0, 0, &path);
    let began = rig.events();
    assert_eq!((began[0].state, began[0].time), (GestureState::Began, at(0)));
    assert!(matches!(began[0].data, GestureData::Press { position } if position == Point::new(50.0, 50.0)));
    rig.moved(0, 53.0, 50.0, 16);
    rig.moved(0, 50.0, 50.0, 32);
    rig.up(0, 50.0, 50.0, 48);
    let rest = rig.events();
    assert_eq!(
        rest.iter().map(|event| (event.intent, event.state)).collect::<Vec<_>>(),
        [
            ("press", GestureState::Changed),
            ("press", GestureState::Changed),
            ("press", GestureState::Ended),
            ("scroll", GestureState::Failed)
        ]
    );
    assert!(matches!(rest[1].data, GestureData::Press { position } if position == Point::new(50.0, 50.0)));

    // Родительский пан распознан: подсветка снимается (Cancelled), кнопка не сработает.
    rig.down(0, 50.0, 50.0, 100, &path);
    rig.moved(0, 50.0, 70.0, 116);
    assert_eq!(
        rig.trace(),
        ["press:Began", "press:Changed", "scroll:Began", "press:Cancelled"]
    );
    rig.up(0, 50.0, 70.0, 132);
    assert_eq!(rig.trace(), ["scroll:Ended"]);
}

/// Press + Tap кнопки внутри видоискателя с тапом: Press не превращается в Cancelled на отрыве.
#[test]
fn button_with_tap_and_press_beats_viewfinder_tap_without_cancelling_press() {
    let path = [node(BUTTON), node(VIEW)];
    let mut rig = Rig::new();
    let button_press = rig.add(node(BUTTON), press(), "button_press");
    let button_tap = rig.add(node(BUTTON), tap(), "button_tap");
    rig.add(node(VIEW), tap(), "focus_tap");
    rig.arena
        .allow_simultaneous(button_press, button_tap)
        .expect("известны");
    rig.down(0, 50.0, 50.0, 0, &path);
    rig.up(0, 50.0, 50.0, 40);
    assert_eq!(
        rig.trace(),
        [
            "button_press:Began",
            "button_press:Ended",
            "button_tap:Ended",
            "focus_tap:Failed"
        ]
    );
}

/// Известное ограничение: кнопка только с Press внутри видоискателя с Tap — срабатывают оба.
/// Рецепт для ui-kit: кнопка = Tap + Press (см. тест выше).
#[test]
fn press_only_button_inside_tap_viewfinder_fires_both() {
    let path = [node(BUTTON), node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(BUTTON), press(), "button_press");
    rig.add(node(VIEW), tap(), "focus_tap");
    rig.down(0, 50.0, 50.0, 0, &path);
    rig.up(0, 50.0, 50.0, 40);
    assert_eq!(
        rig.trace(),
        ["button_press:Began", "button_press:Ended", "focus_tap:Ended"]
    );
}

/// 9. Удаление распознавателей элемента посреди жеста → Cancelled.
#[test]
fn scenario_9_removing_node_recognizers_mid_gesture_cancels_the_active_one() {
    let path = [node(VIEW)];
    let mut rig = Rig::new();
    rig.add(node(VIEW), tap(), "tap");
    let pan_id = rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    let other = rig.add(node(77), tap(), "other");
    rig.down(0, 100.0, 100.0, 0, &path);
    rig.moved(0, 140.0, 100.0, 20);
    rig.trace();
    assert_eq!(rig.arena.remove_node_recognizers(node(VIEW)), Ok(2));
    let events = rig.events();
    assert_eq!(events.len(), 1);
    assert_eq!((events[0].intent, events[0].state), ("pan", GestureState::Cancelled));
    assert!(matches!(events[0].data, GestureData::Pan { position, .. } if position == Point::new(140.0, 100.0)));
    assert_eq!(
        rig.arena.recognizer_state(pan_id),
        Err(GestureError::UnknownRecognizer(pan_id))
    );
    assert_eq!(rig.state(other), GestureState::Possible, "чужие не тронуты");
    rig.moved(0, 160.0, 100.0, 40);
    rig.up(0, 160.0, 100.0, 60);
    assert!(rig.trace().is_empty(), "после удаления палец ничего не запускает");
    assert!(rig.arena.is_idle());
    assert_eq!(
        rig.arena.remove_node_recognizers(node(VIEW)),
        Ok(0),
        "повторно удалять нечего"
    );
}

/// 10. Глубина: тап на кнопке внутри видоискателя — побеждает кнопка.
#[test]
fn scenario_10_deeper_recognizer_wins() {
    let mut rig = Rig::new();
    // Родитель добавлен РАНЬШЕ ребёнка: приоритет даёт глубина, а не порядок добавления.
    rig.add(node(VIEW), tap(), "viewfinder_tap");
    rig.add(node(BUTTON), tap(), "button_tap");
    rig.down(0, 50.0, 50.0, 0, &[node(BUTTON), node(VIEW)]);
    rig.up(0, 50.0, 50.0, 40);
    assert_eq!(rig.trace(), ["button_tap:Ended", "viewfinder_tap:Failed"]);
    // Мимо кнопки — тап видоискателя.
    rig.down(0, 300.0, 300.0, 100, &[node(VIEW)]);
    rig.up(0, 300.0, 300.0, 140);
    assert_eq!(rig.trace(), ["viewfinder_tap:Ended"]);
}

/// Порядок добавления разрешает равную глубину.
#[test]
fn equal_depth_is_resolved_by_insertion_order() {
    let mut rig = Rig::new();
    rig.add(node(VIEW), tap(), "first");
    rig.add(node(VIEW), tap(), "second");
    rig.down(0, 5.0, 5.0, 0, &[node(VIEW)]);
    rig.up(0, 5.0, 5.0, 30);
    assert_eq!(rig.trace(), ["first:Ended", "second:Failed"]);
}

/// Псевдослучайный, но детерминированный поток касаний: дробные координаты и времена.
fn run_mixed_stream() -> Vec<(u32, String, i64, Vec<u64>)> {
    let mut rig = Rig::new();
    let path = [node(BUTTON), node(VIEW)];
    rig.add(node(BUTTON), press(), "press");
    rig.add(node(VIEW), tap(), "tap");
    rig.add(node(VIEW), long_press(), "long");
    rig.add(node(VIEW), pan(PanDirection::Any), "pan");
    rig.add(node(VIEW), pinch(), "pinch");
    let mut all = Vec::new();
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = move || {
        seed = seed
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (seed >> 33) as f64 / f64::from(1u32 << 31)
    };
    let mut clock = 0_i64;
    for round in 0..6 {
        let (mut x, mut y) = (100.0 + next() * 50.0, 200.0 + next() * 50.0);
        rig.down(0, x, y, clock, &path);
        if round % 2 == 1 {
            rig.down(1, x + 90.0 + next() * 20.0, y + next() * 10.0, clock + 5, &path);
        }
        for step in 1..=30 {
            clock += 8 + (next() * 4.0) as i64;
            x += next() * 3.7 - 0.4;
            y += next() * 2.3 - 0.2;
            rig.moved(0, x, y, clock);
            if round % 2 == 1 && step % 3 == 0 {
                rig.moved(1, x + 100.0 + step as f64 * 1.3, y + 3.0, clock);
            }
            if step % 7 == 0 {
                rig.frame(clock + 2);
            }
        }
        if round % 2 == 1 {
            rig.up(1, x + 140.0, y + 3.0, clock + 3);
        }
        clock += 20;
        rig.up(0, x, y, clock);
        rig.frame(clock + 700);
        clock += 800;
        all.extend(bitwise_trace(&rig.events()));
    }
    all
}

/// 11. Детерминизм: один поток → побитово одинаковая трасса на двух свежих аренах.
#[test]
fn scenario_11_same_stream_gives_bitwise_identical_trace() {
    let first = run_mixed_stream();
    let second = run_mixed_stream();
    assert!(
        first.iter().any(|entry| entry.1 == "Began"),
        "в потоке были начавшиеся жесты"
    );
    assert!(
        first.iter().any(|entry| entry.1 == "Changed" && !entry.3.is_empty()),
        "и данные с числами"
    );
    assert_eq!(first, second);
}

/// 12. Затвор (карта функционала М2): tap + long press + горизонтальный пан влево.
#[test]
fn scenario_12_shutter_tap_hold_and_slide_left() {
    let path = [node(BUTTON)];
    let build = || {
        let mut rig = Rig::new();
        rig.add(node(BUTTON), tap(), "shutter_tap");
        rig.add(node(BUTTON), long_press(), "shutter_hold");
        rig.add(node(BUTTON), pan(PanDirection::Left), "shutter_slide_left");
        rig
    };

    // Тап: снимок.
    let mut rig = build();
    rig.down(0, 220.0, 900.0, 0, &path);
    rig.up(0, 220.0, 900.0, 90);
    assert_eq!(
        rig.trace(),
        ["shutter_hold:Failed", "shutter_slide_left:Failed", "shutter_tap:Ended"]
    );

    // Удержание: QuickTake; после Began палец может уйти — жест остаётся удержанием.
    let mut rig = build();
    rig.down(0, 220.0, 900.0, 0, &path);
    rig.frame(520);
    assert_eq!(
        rig.trace(),
        ["shutter_hold:Began", "shutter_tap:Failed", "shutter_slide_left:Failed"]
    );
    rig.moved(0, 150.0, 900.0, 540);
    assert_eq!(rig.trace(), ["shutter_hold:Changed"]);
    rig.up(0, 150.0, 900.0, 600);
    assert_eq!(rig.trace(), ["shutter_hold:Ended"]);

    // Сдвиг влево раньше 0,5 с: серия.
    let mut rig = build();
    rig.down(0, 220.0, 900.0, 0, &path);
    rig.moved(0, 200.0, 902.0, 100);
    assert_eq!(
        rig.trace(),
        ["shutter_tap:Failed", "shutter_hold:Failed", "shutter_slide_left:Began"]
    );

    // Сдвиг вправо: пан влево не принимает, остальные тоже провалены — никто не выигрывает.
    let mut rig = build();
    rig.down(0, 220.0, 900.0, 0, &path);
    rig.moved(0, 240.0, 900.0, 100);
    assert_eq!(
        rig.trace(),
        ["shutter_tap:Failed", "shutter_hold:Failed", "shutter_slide_left:Failed"]
    );
    rig.frame(700);
    rig.up(0, 240.0, 900.0, 800);
    assert!(rig.trace().is_empty());
    assert!(rig.arena.is_idle());
}
