//! Тесты снятия спроса на тёплую камеру исполнителем эффектов (13b, решение 117): при уничтожении исполнителя
//! (снятие экрана на сценарий, гибель экрана в рендере, переустановка) стоящий спрос «держать соседа тёплым»
//! снимается тихим намерением `Continuous`. Стенд (`build_executor`, `lens_intent`) — из
//! `camera_screen_effects_tests`.

use super::*;

fn warm_intent(ratio: f32) -> LensIntent {
    lens_intent(ratio).with_standby(camera_contract::zoom::StandbyDemand::KeepNeighbourWarm)
}

#[test]
fn dropping_the_executor_releases_a_standing_warm_demand_keeping_facing_and_ratio() {
    let (mut executor, rig) = build_executor(Behavior::default());
    executor.execute(vec![Effect::Lens(warm_intent(1.7))]);
    assert_eq!(rig.lens.latest().expect("намерение").1, warm_intent(1.7));

    drop(executor);

    let (_, latest) = rig.lens.latest().expect("намерение");
    assert_eq!(latest, lens_intent(1.7), "то же направление и коэффициент, спрос снят");
}

#[test]
fn releasing_publishes_nothing_when_there_is_no_demand_or_no_intent_at_all() {
    let (mut executor, rig) = build_executor(Behavior::default());
    assert!(!executor.release_standby_demand(), "пустой ящик");
    assert!(rig.lens.latest().is_none());

    executor.execute(vec![Effect::Lens(lens_intent(2.0))]);
    let before = rig.lens.latest().expect("намерение").0;
    assert!(!executor.release_standby_demand(), "спроса нет");
    assert_eq!(executor.counters().lens.published, 1, "лишних публикаций нет");
    drop(executor);
    assert_eq!(rig.lens.latest().expect("намерение").0, before, "ящик не тронут");
}

#[test]
fn an_explicit_release_counts_one_publication_and_a_second_release_is_a_no_op() {
    let (mut executor, rig) = build_executor(Behavior::default());
    executor.execute(vec![Effect::Lens(warm_intent(1.0))]);
    assert!(executor.release_standby_demand());
    assert!(!executor.release_standby_demand(), "уже снято");
    assert_eq!(executor.counters().lens.published, 2);
    assert_eq!(rig.lens.latest().expect("намерение").1, lens_intent(1.0));
}

#[test]
fn releasing_a_demand_published_by_a_discrete_tap_republishes_it_as_continuous() {
    // Повтор `Discrete` снял бы блокировку объектива после отказа (побочный эффект тапа):
    // снятие спроса обязано быть «тихим» намерением `Continuous` с тем же направлением и коэффициентом.
    let (mut executor, rig) = build_executor(Behavior::default());
    let discrete_warm = LensIntent {
        gesture: camera_contract::zoom::ZoomGesture::Discrete,
        ..warm_intent(0.6)
    };
    executor.execute(vec![Effect::Lens(discrete_warm)]);

    assert!(executor.release_standby_demand());

    let (_, latest) = rig.lens.latest().expect("намерение");
    assert_eq!(latest.gesture, camera_contract::zoom::ZoomGesture::Continuous);
    assert_eq!(latest.standby, camera_contract::zoom::StandbyDemand::NotNeeded);
    assert_eq!(latest, lens_intent(0.6), "направление и коэффициент прежние");
}
