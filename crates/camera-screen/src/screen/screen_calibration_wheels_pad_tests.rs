//! Круг толчка, вкладки, кольцо, три «↻» и синхронизация вида колёс (решения 205–207).
//! Общие помощники и тесты переключателя — в `screen_calibration_wheels_tests`.

use look_contract::{ColorWheel, LookControl, LookIntent, LookKnob, WheelPart, WheelPush, WheelZone};
use shell_contract::orientation::DeviceOrientation;
use shell_contract::touch::TouchPhase;
use ui_core::geometry::{Point, Vector};
use ui_draw::draw_list::{DrawItem, DrawList, ImageSource};
use ui_draw::geometry::PxVector;
use ui_kit::tokens::TextRoleKind;

use super::screen_calibration_tests::{center, drag_eye, edited, surface_vector};
use super::screen_calibration_wheels_tests::*;
use super::test_rig::*;
use crate::action::ScreenAction;
use crate::calibration::ControlViewKind;
use crate::look_test_support::{knob, look_facts, look_intents, test_wheel_ring};

// ----- круг толчка: оси ---------------------------------------------------------------------------------------------

#[test]
fn a_finger_drag_along_each_eye_axis_pushes_the_matching_oklab_axis_in_every_orientation() {
    // 0° глазами = +a, 90° = +b, 180° = −a, 270° = −b: так же, как вектороскоп 14h и кольцо оттенков.
    const LENGTH_PT: f64 = 45.0;
    for orientation in ALL_ORIENTATIONS {
        for (degrees, expected_a, expected_b) in [
            (0.0, 1.0, 0.0),
            (90.0, 0.0, 1.0),
            (180.0, -1.0, 0.0),
            (270.0, 0.0, -1.0),
        ] {
            let mut rig = wheels(look_facts(), orientation);
            let placement = pad(&rig);
            drag_eye(&mut rig, placement.circle.center(), eye_direction(degrees, LENGTH_PT));
            let sent = pushes(&rig.effects());
            let (zone, push) = *sent
                .last()
                .unwrap_or_else(|| panic!("{orientation:?} {degrees}°: толчок не ушёл"));
            assert_eq!(zone, WheelZone::Lift);
            let strength = expected_strength(LENGTH_PT, &placement);
            let (a, b) = (f64::from(push.a()), f64::from(push.b()));
            assert!(
                (a - expected_a * strength).abs() < 0.01 && (b - expected_b * strength).abs() < 0.01,
                "{orientation:?} {degrees}°: ждали ({:.3}, {:.3}), пришло ({a:.3}, {b:.3})",
                expected_a * strength,
                expected_b * strength
            );
            assert!(
                (f64::from(push.strength()) - strength).abs() < 0.01,
                "{orientation:?} {degrees}°: сила {} вместо {strength}",
                push.strength()
            );
        }
    }
}

#[test]
fn the_push_direction_matches_the_ring_colour_the_screen_draws_in_that_direction() {
    // Угол толчка → номер стопа кольца; пиксель растра под этим углом — этот же стоп; и через transform картинки
    // (повёрнутый раздел) этот пиксель лежит там, куда повёл палец.
    let ring = test_wheel_ring();
    let stops = ring.stops();
    for orientation in ALL_ORIENTATIONS {
        for degrees in [0.0_f64, 40.0, 90.0, 130.0, 180.0, 220.0, 270.0, 310.0] {
            let mut rig = wheels(look_facts(), orientation);
            let placement = pad(&rig);
            let eye = eye_direction(degrees, 60.0);
            drag_eye(&mut rig, placement.circle.center(), eye);
            let (_, push) = *pushes(&rig.effects())
                .last()
                .unwrap_or_else(|| panic!("{orientation:?} {degrees}°: толчок не ушёл"));
            let angle = f64::from(push.b())
                .atan2(f64::from(push.a()))
                .to_degrees()
                .rem_euclid(360.0);
            assert!(
                (angle - degrees).abs() < 1.0,
                "{orientation:?}: палец повели на {degrees}°, толчок на {angle}°"
            );
            let stop_index = (angle / 10.0).round() as usize % stops.len();

            let list = rig.frame().clone();
            let images = ring_images(&list);
            assert_eq!(images.len(), 1, "{orientation:?}: ровно одно кольцо в кадре");
            let image = images[0];
            let ImageSource::Picture(picture) = &image.source else {
                panic!("кольцо — растр из памяти");
            };
            // Точка картинки под углом толчка (картинка в своих осях: y вниз, угол против часовой).
            let side = f64::from(picture.size().width);
            let thickness_px = f64::from(px(
                crate::calibration::CalibrationWidgetsConfig::default().wheel_ring_thickness_pt
            ));
            let mid_radius = side / 2.0 - thickness_px / 2.0;
            let local = (
                mid_radius * angle.to_radians().cos(),
                -mid_radius * angle.to_radians().sin(),
            );
            // Берём пиксель на градус левее угла толчка: цвет там отличается от стопа на десятую шага, зато не
            // попадает на стык «последний стоп → нулевой» и не зависит от полпикселя округления.
            let sample = (
                mid_radius * (angle + 1.0).to_radians().cos(),
                -mid_radius * (angle + 1.0).to_radians().sin(),
            );
            let texel = picture
                .texel(
                    (side / 2.0 + sample.0).round() as u32,
                    (side / 2.0 + sample.1).round() as u32,
                )
                .expect("точка кольца внутри картинки");
            // Пиксель стоит в полпикселя от точного угла и смешивает два соседних стопа: ближайший стоп к его цвету
            // обязан быть тем, что соответствует углу толчка.
            let distance = |stop: &[u8; 3]| -> i32 {
                (0..3)
                    .map(|channel| (i32::from(texel[channel]) - i32::from(stop[channel])).abs())
                    .sum()
            };
            let nearest = (0..stops.len())
                .min_by_key(|index| distance(&stops[*index].srgb))
                .expect("в кольце есть стопы");
            assert_eq!(
                nearest, stop_index,
                "{orientation:?} {degrees}°: цвет кольца {texel:?} ближе к стопу №{nearest}, чем к №{stop_index}"
            );
            // Поворот раздела: направление этой точки картинки на поверхности = направление пальца на поверхности.
            let on_surface = image
                .transform
                .apply_vector(PxVector::new(local.0 as f32, local.1 as f32));
            let finger = surface_vector(&rig, eye);
            let (sx, sy) = (f64::from(on_surface.dx), f64::from(on_surface.dy));
            let cosine = (sx * finger.dx + sy * finger.dy) / (sx.hypot(sy) * finger.dx.hypot(finger.dy));
            assert!(
                cosine > 0.999,
                "{orientation:?} {degrees}°: кольцо на поверхности смотрит не туда, куда повели палец (cos {cosine})"
            );
        }
    }
}

#[test]
fn dragging_past_the_edge_pins_the_point_to_the_edge_at_the_same_angle_without_repeats() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let placement = pad(&rig);
    // 45°: далеко за край круга.
    drag_eye(&mut rig, placement.circle.center(), eye_direction(45.0, 400.0));
    let sent = pushes(&rig.effects());
    let (_, last) = *sent.last().expect("толчок ушёл");
    assert!(
        last.strength() > 0.999,
        "точка упёрлась в край: сила {}",
        last.strength()
    );
    let angle = f64::from(last.b()).atan2(f64::from(last.a())).to_degrees();
    assert!((angle - 45.0).abs() < 0.5, "угол сохранился: {angle}");
    assert!(
        sent.windows(2).all(|pair| pair[0].1 != pair[1].1),
        "на краю действие не повторяется: {sent:?}"
    );
}

#[test]
fn a_drag_on_the_circle_moves_only_the_point_and_never_scrolls_or_touches_other_controls() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let placement = pad(&rig);
    let mut offsets = Vec::new();
    drag_eye_with(&mut rig, placement.circle.center(), Vector::new(30.0, 80.0), |rig| {
        offsets.push(rig.screen.calibration_scroll_offset())
    });
    assert_eq!(offsets, [Some(0.0)], "середина жеста: список не тянется");
    assert_eq!(rig.screen.calibration_scroll_offset(), Some(0.0));
    let sent = intents(&rig.effects());
    assert!(!sent.is_empty(), "точка сдвинулась");
    assert!(
        sent.iter().all(|intent| matches!(
            intent,
            LookIntent::SetWheelPush {
                zone: WheelZone::Lift,
                ..
            }
        )),
        "ушли только толчки выбранного колеса: {sent:?}"
    );
}

#[test]
fn a_drag_outside_the_circle_sends_no_wheel_action_and_scrolls_nothing() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let placement = pad(&rig);
    // Левее круга, но внутри колонки: поле между краем содержимого и кругом.
    let beside = Point::new(placement.circle.min_x() - 8.0, placement.circle.center().y);
    drag_eye(&mut rig, beside, Vector::new(0.0, 70.0));
    assert!(intents(&rig.effects()).is_empty(), "вне круга действий колеса нет");
    assert_eq!(rig.screen.calibration_scroll_offset(), Some(0.0));
}

// ----- вкладки, кольцо, зона ----------------------------------------------------------------------------------------

#[test]
fn after_switching_the_tab_the_next_push_and_ring_drag_go_to_the_new_wheel() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let gamma_tab = center(&rig, "calib_wheel_tab", 1);
    rig.tap(gamma_tab);
    rig.run(200);
    let placement = pad(&rig);
    drag_eye(&mut rig, placement.circle.center(), eye_direction(0.0, 40.0));
    let sent = pushes(&rig.effects());
    assert!(
        !sent.is_empty() && sent.iter().all(|(zone, _)| *zone == WheelZone::Gamma),
        "{sent:?}"
    );

    let ruler = center(&rig, "calib_ruler", 0);
    drag_eye(&mut rig, ruler, Vector::new(-knob_finger_pt_from_zero(60.0), 0.0));
    let ring: Vec<_> = intents(&rig.effects())
        .into_iter()
        .filter(|intent| matches!(intent, LookIntent::SetWheelRing { .. }))
        .collect();
    assert_eq!(
        ring.last(),
        Some(&LookIntent::SetWheelRing {
            zone: WheelZone::Gamma,
            ring: knob(0.3)
        }),
        "шкала кольца ведёт кольцо выбранного колеса на точное значение: {ring:?}"
    );
}

#[test]
fn the_wheel_chosen_at_touch_down_keeps_the_gesture_even_if_the_tab_changes_midway() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let placement = pad(&rig);
    drag_eye_with(&mut rig, placement.circle.center(), eye_direction(0.0, 60.0), |rig| {
        rig.screen.handle_action(ScreenAction::WheelSelected(WheelZone::Offset));
    });
    assert_eq!(rig.screen.state().calibration().selected_wheel(), WheelZone::Offset);
    let sent = pushes(&rig.effects());
    assert!(!sent.is_empty());
    assert!(
        sent.iter().all(|(zone, _)| *zone == WheelZone::Lift),
        "жест начат на Lift и им же заканчивается: {sent:?}"
    );
}

// ----- три «↻» ------------------------------------------------------------------------------------------------------

#[test]
fn each_reset_arrow_resets_only_its_part_and_only_when_that_part_differs() {
    // Правлен только толчок Lift: «колесо целиком» и «толчок» яркие, «кольцо» тусклое.
    let wheel = ColorWheel {
        push: push_of(0.4, 0.2),
        ring: LookKnob::NEUTRAL,
    };
    let mut rig = wheels(facts_with_wheel(WheelZone::Lift, wheel), DeviceOrientation::Portrait);
    let whole = center(&rig, "calib_wheel_reset_whole", 0);
    let push = center(&rig, "calib_wheel_reset_push", 0);
    let ring = center(&rig, "calib_reset", 0);

    let list = rig.frame().clone();
    let whole_opacity = reset_opacity_at(&list, whole).expect("«↻» колеса нарисована");
    let push_opacity = reset_opacity_at(&list, push).expect("«↻» толчка нарисована");
    let ring_opacity = reset_opacity_at(&list, ring).expect("«↻» кольца нарисована");
    assert!(
        near(whole_opacity, 1.0, 0.02),
        "колесо правлено — яркая: {whole_opacity}"
    );
    assert!(near(push_opacity, 1.0, 0.02), "толчок правлен — яркая: {push_opacity}");
    assert!(
        near(ring_opacity, 0.44, 0.02),
        "кольцо как в пресете — 44 %: {ring_opacity}"
    );

    for (point, part) in [(whole, WheelPart::Whole), (push, WheelPart::Push)] {
        rig.tap(point);
        rig.run(100);
        assert_eq!(
            intents(&rig.effects()),
            vec![LookIntent::ResetControl(LookControl::Wheel {
                zone: WheelZone::Lift,
                part
            })],
            "{part:?}"
        );
    }
    rig.tap(ring);
    rig.run(100);
    assert!(intents(&rig.effects()).is_empty(), "тусклая «↻» кольца молчит");
}

#[test]
fn the_ring_reset_arrow_resets_the_ring_of_the_selected_wheel() {
    let wheel = ColorWheel {
        push: WheelPush::NEUTRAL,
        ring: knob(0.5),
    };
    let mut rig = wheels(facts_with_wheel(WheelZone::Gain, wheel), DeviceOrientation::Portrait);
    let ring_reset = center(&rig, "calib_reset", 0);
    rig.tap(ring_reset);
    rig.run(100);
    assert!(
        intents(&rig.effects()).is_empty(),
        "у выбранного Lift кольцо не правлено — «↻» молчит"
    );

    let gain_tab = center(&rig, "calib_wheel_tab", 2);
    rig.tap(gain_tab);
    rig.run(200);
    rig.tap(ring_reset);
    rig.run(100);
    assert_eq!(
        intents(&rig.effects()),
        vec![LookIntent::ResetControl(LookControl::Wheel {
            zone: WheelZone::Gain,
            part: WheelPart::Ring
        })]
    );
}

#[test]
fn only_the_edited_wheel_tab_carries_the_ring_mark() {
    let wheel = ColorWheel {
        push: WheelPush::NEUTRAL,
        ring: knob(-0.3),
    };
    let rig_with = |facts| wheels(facts, DeviceOrientation::Portrait).frame().clone();
    let marked_rings = |list: &DrawList| {
        list.leaf_items()
            .filter(|item| {
                matches!(item, DrawItem::Shape(shape)
                    if matches!(shape.geometry, ui_draw::shape::ShapeGeometry::Circle { diameter }
                        if near(diameter, px(7.0), 0.6))
                        && shape.stroke.is_some()
                        && shape.fill.a == 0.0)
            })
            .count()
    };
    let untouched = rig_with(look_facts());
    let edited_gamma = rig_with(facts_with_wheel(WheelZone::Gamma, wheel));
    assert_eq!(marked_rings(&untouched), 0, "ни одно колесо не правлено — меток нет");
    assert_eq!(
        marked_rings(&edited_gamma),
        1,
        "правлено одно колесо — метка у одной вкладки"
    );
}

// ----- без фактов и синхронизация -----------------------------------------------------------------------------------

#[test]
fn without_look_facts_the_wheel_is_dim_and_inert() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::ModeSelected(crate::modes::CaptureMode::Calibrate));
    rig.run(400);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.run(300);
    rig.effects();
    let placement = pad(&rig);
    drag_eye(&mut rig, placement.circle.center(), eye_direction(0.0, 60.0));
    assert!(
        look_intents(&rig.effects()).is_empty(),
        "без библиотеки пресетов круг молчит"
    );
    assert!(rig.frame().items.len() > 5, "вид всё равно рисуется");
}

#[test]
fn facts_arriving_mid_drag_do_not_pull_the_point_and_programmatic_sync_sends_no_actions() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let placement = pad(&rig);
    let moved = facts_with_wheel(
        WheelZone::Lift,
        ColorWheel {
            push: push_of(-0.6, 0.0),
            ring: LookKnob::NEUTRAL,
        },
    );
    drag_eye_with(&mut rig, placement.circle.center(), eye_direction(0.0, 60.0), |rig| {
        rig.screen
            .handle_action(ScreenAction::LookFactsChanged(Box::new(moved.clone())));
    });
    let sent = pushes(&rig.effects());
    assert!(!sent.is_empty());
    assert!(
        sent.iter().all(|(_, push)| push.a() >= 0.0),
        "точка пальца не прыгала к значению из фактов, пришедших посреди жеста: {sent:?}"
    );

    // Факты, пришедшие без пальца, двигают точку молча: ни одного действия.
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(moved)));
    rig.run(500);
    assert!(
        look_intents(&rig.effects()).is_empty(),
        "синхронизация из фактов не рождает действий"
    );
    let drawn = center(&rig, "calib_wheel_pad", 0);
    assert!(
        (drawn.x - placement.circle.center().x).abs() < 0.5,
        "круг стоит на месте"
    );
}

#[test]
fn rebuilding_the_section_mid_gesture_leaves_no_stuck_drag() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let placement = pad(&rig);
    let from = placement.circle.center();
    let mut at_ms = rig.now_ms;
    let mut touch_at = |rig: &mut Rig, phase: TouchPhase, point: Point| {
        rig.touch(phase, point, at_ms);
        rig.now_ms = at_ms;
        rig.frame();
        at_ms += FRAME;
    };
    touch_at(&mut rig, TouchPhase::Began, from);
    for step in 1..=6 {
        touch_at(
            &mut rig,
            TouchPhase::Moved,
            Point::new(from.x + 6.0 * f64::from(step), from.y),
        );
    }
    assert!(!pushes(&rig.effects()).is_empty(), "жест идёт и двигает точку");
    // Поворот телефона посреди жеста пересобирает раздел: палец прежнего раздела потерян.
    rig.screen
        .handle_action(ScreenAction::DeviceOrientationChanged(DeviceOrientation::LandscapeLeft));
    rig.frame();
    // Тот же физический палец продолжает идти и отрывается: новому разделу он не принадлежит.
    for step in 7..=12 {
        touch_at(
            &mut rig,
            TouchPhase::Moved,
            Point::new(from.x + 6.0 * f64::from(step), from.y),
        );
    }
    touch_at(&mut rig, TouchPhase::Ended, from);
    rig.run(600);
    assert!(
        intents(&rig.effects()).is_empty(),
        "после пересборки старый палец не ведёт ни точку нового круга, ни что-либо ещё"
    );
    // Новый палец ведёт точку нового круга, как обычно.
    let placement = pad(&rig);
    drag_eye(&mut rig, placement.circle.center(), eye_direction(90.0, 45.0));
    let sent = pushes(&rig.effects());
    let (_, last) = *sent.last().expect("новый жест работает после пересборки");
    assert!(last.b() > 0.1 && last.a().abs() < 0.05, "{last:?}");
}

// ----- арбитраж «↻» толчка внутри квадрата круга ---------------------------------------------------------------------

fn edited_push_facts() -> look_contract::LookFacts {
    facts_with_wheel(
        WheelZone::Lift,
        ColorWheel {
            push: push_of(0.4, 0.2),
            ring: LookKnob::NEUTRAL,
        },
    )
}

#[test]
fn a_pan_that_starts_on_the_push_reset_arrow_drives_the_point_and_sends_no_reset() {
    let mut rig = wheels(edited_push_facts(), DeviceOrientation::Portrait);
    let on_arrow = center(&rig, "calib_wheel_reset_push", 0);
    drag_eye(&mut rig, on_arrow, eye_direction(135.0, 45.0));
    let sent = intents(&rig.effects());
    assert!(
        sent.iter()
            .any(|intent| matches!(intent, LookIntent::SetWheelPush { .. })),
        "палец, начатый на «↻», ведёт точку: {sent:?}"
    );
    assert!(
        !sent.iter().any(|intent| matches!(intent, LookIntent::ResetControl(_))),
        "пан не превращается в сброс: {sent:?}"
    );
}

#[test]
fn a_short_tap_on_the_push_reset_arrow_resets_and_leaves_the_point_alone() {
    let mut rig = wheels(edited_push_facts(), DeviceOrientation::Portrait);
    let on_arrow = center(&rig, "calib_wheel_reset_push", 0);
    rig.tap(on_arrow);
    rig.run(100);
    assert_eq!(
        intents(&rig.effects()),
        vec![LookIntent::ResetControl(LookControl::Wheel {
            zone: WheelZone::Lift,
            part: WheelPart::Push
        })],
        "тап — только сброс, без SetWheelPush"
    );
}

// ----- точка равна фактам после отпускания ---------------------------------------------------------------------------

/// Смещение шайбы от центра круга, pt поверхности (x вправо, y ВНИЗ); портретная ориентация.
fn puck_offset_pt(list: &DrawList, placement: &crate::calibration::WheelPadPlacement) -> (f64, f64) {
    let puck_diameter = px(ui_kit::tokens::UiKitTokens::default().disc_pad.puck_diameter_pt);
    let puck = list
        .leaf_items()
        .find_map(|item| match item {
            DrawItem::Shape(shape)
                if shape.stroke.is_some()
                    && matches!(shape.geometry, ui_draw::shape::ShapeGeometry::Circle { diameter }
                        if near(diameter, puck_diameter, 0.6)) =>
            {
                Some(shape.transform)
            }
            _ => None,
        })
        .expect("шайба нарисована");
    let center = placement.circle.center();
    (
        (f64::from(puck.tx) - f64::from(px(center.x))) / f64::from(PX_PER_PT),
        (f64::from(puck.ty) - f64::from(px(center.y))) / f64::from(PX_PER_PT),
    )
}

#[test]
fn after_the_finger_lifts_the_point_returns_to_the_facts_even_when_the_facts_did_not_change() {
    // Библиотека «отвергла» значение: факты после жеста те же, что до него (нейтраль).
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let placement = pad(&rig);
    let mut midway_offset = (0.0, 0.0);
    drag_eye_with(&mut rig, placement.circle.center(), eye_direction(0.0, 60.0), |rig| {
        let list = rig.frame().clone();
        midway_offset = puck_offset_pt(&list, &placement);
    });
    assert!(
        midway_offset.0 > 5.0,
        "посреди жеста шайба идёт за пальцем: {midway_offset:?}"
    );
    let list = rig.frame().clone();
    let (dx, dy) = puck_offset_pt(&list, &placement);
    assert!(
        dx.abs() < 0.5 && dy.abs() < 0.5,
        "после отпускания шайба в центре, как в фактах: ({dx}, {dy})"
    );
}

#[test]
fn facts_that_arrive_mid_drag_are_applied_after_the_finger_lifts() {
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let placement = pad(&rig);
    let arrived = facts_with_wheel(
        WheelZone::Lift,
        ColorWheel {
            push: push_of(-0.6, 0.0),
            ring: LookKnob::NEUTRAL,
        },
    );
    drag_eye_with(&mut rig, placement.circle.center(), eye_direction(0.0, 60.0), |rig| {
        rig.screen
            .handle_action(ScreenAction::LookFactsChanged(Box::new(arrived.clone())));
    });
    let list = rig.frame().clone();
    let (dx, dy) = puck_offset_pt(&list, &placement);
    let expected = -0.6 * placement.pad_radius_pt;
    assert!(
        (dx - expected).abs() < 0.6 && dy.abs() < 0.5,
        "шайба стоит по фактам: ({dx}, {dy}) вместо ({expected}, 0)"
    );
}

// ----- кэш картинки кольца -------------------------------------------------------------------------------------------

#[test]
fn the_ring_picture_is_built_once_and_rebuilt_only_when_the_colours_change() {
    let version_of = |rig: &mut Rig| {
        let list = rig.frame().clone();
        let images = ring_images(&list);
        let ImageSource::Picture(picture) = &images[0].source else {
            panic!("кольцо — растр из памяти");
        };
        picture.version()
    };
    let mut rig = wheels(look_facts(), DeviceOrientation::Portrait);
    let first = version_of(&mut rig);
    rig.run(100);
    assert_eq!(
        version_of(&mut rig),
        first,
        "кадры подряд — та же картинка, растр не строится заново"
    );

    let mut recoloured = look_facts();
    recoloured.wheel_ring = look_contract::WheelHueRing::solid(look_contract::BandSwatch { srgb: [10, 20, 30] });
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(recoloured)));
    rig.run(100);
    let second = version_of(&mut rig);
    assert_ne!(second, first, "другие цвета — новая картинка");
    rig.run(100);
    assert_eq!(version_of(&mut rig), second);
}

// ----- деградация на низкой поверхности ------------------------------------------------------------------------------

#[test]
fn when_the_circle_does_not_fit_the_column_lives_without_wheels_counts_once_and_scales_come_back() {
    let mut config = crate::ScreenConfig::default();
    // Блок вкладок во всю колонку: круг заведомо не помещается.
    config.calibration.widgets.wheel_tabs_height_pt = 900.0;
    let mut rig = Rig::with(config, ui_core::time::MotionPreference::Full);
    rig.screen
        .handle_action(ScreenAction::LookFactsChanged(Box::new(look_facts())));
    rig.act(ScreenAction::ModeSelected(crate::modes::CaptureMode::Calibrate));
    rig.run(400);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.run(1000);
    rig.effects();

    assert!(rig.screen.wheel_pad_placement().is_none(), "круга нет");
    assert!(rig.screen.node_centers("calib_wheel_pad").is_empty());
    assert_eq!(
        rig.screen.node_centers("calib_view_segment").len(),
        2,
        "переключатель на месте"
    );
    assert_eq!(
        rig.screen
            .state()
            .rejections()
            .get(crate::outcome::Rejection::InvalidSurface),
        1,
        "причина посчитана один раз, а не на каждом кадре"
    );

    let scales = center(&rig, "calib_view_segment", 0);
    rig.tap(scales);
    rig.run(300);
    assert_eq!(rig.screen.state().calibration().control_view(), ControlViewKind::Scales);
    assert!(rig.screen.node_centers("calib_row").len() >= 11, "шкалы вернулись");
}

// ----- мелочи ---------------------------------------------------------------------------------------------------------

#[test]
fn a_reset_arrow_resets_the_wheel_that_was_selected_when_it_was_pressed() {
    let both = edited(|mut settings| {
        for zone in [WheelZone::Lift, WheelZone::Gain] {
            settings.wheels = settings.wheels.with_wheel(
                zone,
                ColorWheel {
                    push: push_of(0.4, 0.2),
                    ring: LookKnob::NEUTRAL,
                },
            );
        }
        settings
    });
    let mut rig = wheels(both, DeviceOrientation::Portrait);
    let whole = center(&rig, "calib_wheel_reset_whole", 0);
    let start = rig.now_ms;
    rig.touch(TouchPhase::Began, whole, start);
    rig.now_ms = start + 20;
    rig.frame();
    // Вкладка сменилась другим пальцем, пока «↻» нажата.
    rig.screen.handle_action(ScreenAction::WheelSelected(WheelZone::Gain));
    rig.touch(TouchPhase::Ended, whole, start + 60);
    rig.now_ms = start + 80;
    rig.frame();
    rig.run(100);
    assert_eq!(
        intents(&rig.effects()),
        vec![LookIntent::ResetControl(LookControl::Wheel {
            zone: WheelZone::Lift,
            part: WheelPart::Whole
        })],
        "сброс ушёл колесу, выбранному при нажатии"
    );
}

#[test]
fn without_facts_the_ring_scale_is_dimmed_like_the_other_wheel_parts() {
    let mut rig = Rig::new();
    rig.act(ScreenAction::ModeSelected(crate::modes::CaptureMode::Calibrate));
    rig.run(400);
    rig.act(ScreenAction::ControlViewSelected(ControlViewKind::Wheels));
    rig.run(300);
    let list = rig.frame().clone();
    let title = texts_of(&list, "RING", TextRoleKind::CardTitle);
    assert_eq!(title.len(), 1);
    assert!(
        title[0].opacity.get() < 0.5,
        "шкала кольца тусклая без фактов: {}",
        title[0].opacity.get()
    );
}
