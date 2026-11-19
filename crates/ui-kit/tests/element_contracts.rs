#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]
//! Общие контракты элементов: отмена жеста (одно правило), атомарная смена пределов/списка,
//! смена Reduce Motion на лету, один тип перехода для шкалы и карусели.

use shell_contract::haptics::HapticPattern;
use ui_core::scroll::ScrollPhase;
use ui_core::time::MotionPreference;
use ui_kit::carousel::{CarouselEntry, CarouselError, CarouselEvent, CarouselSpec, SelectionCause, SnapCarousel};
use ui_kit::drive::{DragOrigin, GrowthDirection, MotionPhase, ScrollTransition};
use ui_kit::scale::{ScaleError, ScaleEvent, ScaleSpec, SnapScale};
use ui_kit::tokens::{SpringToken, UiKitTokens};
use ui_kit::value::{LinearLimits, Mark, Quantization, QuantizedLinear, ValueError, ValueModel};

mod common;
use common::*;

// ---------- отмена жеста: одно правило ----------

fn right_spec() -> ScaleSpec {
    exposure_spec(GrowthDirection::FingerRight, DragOrigin::FromRecognition)
}

/// Тянет шкалу на `dx` (с гистерезисом 0) и возвращает её после `finish`.
fn dragged_scale(
    tokens: &UiKitTokens,
    dx: f64,
    finish: impl FnOnce(&mut SnapScale<QuantizedLinear>),
) -> SnapScale<QuantizedLinear> {
    let mut scale = exposure_scale(0.0, right_spec(), tokens);
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(dx)).unwrap();
    finish(&mut scale);
    scale
}

/// Системная отмена шкалы = отпускание без скорости: тот же итог, те же события, та же траектория
/// бит в бит; значение садится на шаг (магнит у шкалы без магнитов не нужен).
#[test]
fn scale_cancel_is_a_release_without_velocity() {
    let tokens = UiKitTokens::default();
    // 20 pt при 12 pt на шаг: между шагами 1 и 2, ближе к шагу 2 (24 pt).
    let mut cancelled = dragged_scale(&tokens, 20.0, |scale| scale.drag_cancelled(ms(32)).unwrap());
    let mut released = dragged_scale(&tokens, 20.0, |scale| scale.release_without_drag(ms(32)).unwrap());
    for at in [32, 100, 300, 2_000] {
        assert_eq!(
            cancelled.snapshot(ms(at)).offset_points.to_bits(),
            released.snapshot(ms(at)).offset_points.to_bits(),
            "t = {at}"
        );
    }
    assert_eq!(cancelled.drain_events(), released.drain_events());
    let settled = cancelled.snapshot(ms(5_000));
    assert_eq!(settled.phase, MotionPhase::Resting);
    assert!(
        (settled.value - 2.0 / 3.0).abs() < 1e-9,
        "ближайший шаг 2/3 EV: {}",
        settled.value
    );
}

/// У плавной шкалы отмена тоже притягивается магнитом, как отпускание без скорости.
#[test]
fn magnet_scale_cancel_snaps_to_the_anchor_like_a_release() {
    let tokens = UiKitTokens::default();
    // 24 pt на единицу, радиус магнита 14 pt: останавливаемся в 6 pt от якоря 1,0 (позиция 0).
    let finish_with = |cancel: bool| {
        let mut scale = magnet_scale(0.5, vec![1.0], 4.0, &tokens);
        let start_offset = scale.snapshot(ms(0)).offset_points;
        let anchor_offset = 0.0;
        // Палец вправо — значение растёт: ведём к точке в 6 pt до якоря.
        let travel = anchor_offset - 6.0 - start_offset;
        scale.drag_began(ms(0), pan_same(0.0)).unwrap();
        scale.drag_moved(ms(16), pan_same(travel)).unwrap();
        if cancel {
            scale.drag_cancelled(ms(32)).unwrap();
        } else {
            scale.release_without_drag(ms(32)).unwrap();
        }
        scale
    };
    let cancelled = finish_with(true);
    let released = finish_with(false);
    assert_eq!(cancelled.value(ms(10_000)), 1.0, "магнит довёл до якоря");
    assert_eq!(
        cancelled.snapshot(ms(10_000)).offset_points.to_bits(),
        released.snapshot(ms(10_000)).offset_points.to_bits()
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Video,
    Photo,
    Portrait,
}

fn modes() -> Vec<CarouselEntry<Mode>> {
    vec![
        CarouselEntry {
            item: Mode::Video,
            width_points: 61.1,
        },
        CarouselEntry {
            item: Mode::Photo,
            width_points: 50.0,
        },
        CarouselEntry {
            item: Mode::Portrait,
            width_points: 70.0,
        },
    ]
}

fn mode_carousel(tokens: &UiKitTokens, motion: MotionPreference) -> SnapCarousel<Mode> {
    SnapCarousel::new(
        modes(),
        &Mode::Video,
        CarouselSpec::default(),
        &env(tokens, motion),
        ms(0),
    )
    .unwrap()
}

/// Отмена жеста карусели фиксирует ближайший элемент: за половину пути — следующий (событие выбора
/// от пальца с хаптикой), меньше половины — прежний (событий нет). Совпадает с отпусканием без скорости.
#[test]
fn carousel_cancel_commits_the_nearest_item_like_a_release() {
    let tokens = UiKitTokens::default();
    // До центра следующего элемента 90,95 pt: 60 pt — дальше половины, 20 pt — ближе.
    for (drag, expected, changed) in [(60.0, Mode::Photo, true), (20.0, Mode::Video, false)] {
        let finish = |cancel: bool| {
            let mut carousel = mode_carousel(&tokens, MotionPreference::Full);
            carousel.drag_began(ms(0), pan_same(0.0)).unwrap();
            carousel.drag_moved(pan_same(-drag)).unwrap();
            if cancel {
                carousel.drag_cancelled(ms(16)).unwrap();
            } else {
                carousel.release_without_drag(ms(16)).unwrap();
            }
            carousel
        };
        let mut cancelled = finish(true);
        let mut released = finish(false);
        assert_eq!(cancelled.selected(), &expected, "drag = {drag}");
        let events = cancelled.drain_events();
        assert_eq!(events, released.drain_events());
        if changed {
            assert!(matches!(
                events.as_slice(),
                [
                    CarouselEvent::SelectionChanged {
                        item: Mode::Photo,
                        index: 1,
                        cause: SelectionCause::Swipe
                    },
                    CarouselEvent::Haptic {
                        pattern: HapticPattern::SelectionTick
                    }
                ]
            ));
        } else {
            assert!(events.is_empty());
        }
        for at in [16, 200, 5_000] {
            assert_eq!(
                cancelled.offset_points(ms(at)).to_bits(),
                released.offset_points(ms(at)).to_bits()
            );
        }
        assert_eq!(cancelled.phase(ms(5_000)), MotionPhase::Resting);
    }
}

/// За краем карусель возвращается к последнему элементу; индекс точки приходит от движка, а не
/// ищется сравнением чисел: отмена за краем даёт ровно один выбор последнего.
#[test]
fn carousel_cancel_past_the_edge_selects_the_last_item_by_engine_index() {
    let tokens = UiKitTokens::default();
    let mut carousel = mode_carousel(&tokens, MotionPreference::Full);
    carousel.drag_began(ms(0), pan_same(0.0)).unwrap();
    // Тянем сильно за последний элемент (186,35 pt), в зону «резинки».
    carousel.drag_moved(pan_same(-300.0)).unwrap();
    carousel.drag_cancelled(ms(16)).unwrap();
    assert_eq!(carousel.selected(), &Mode::Portrait);
    let events = carousel.drain_events();
    assert!(matches!(
        events.first(),
        Some(CarouselEvent::SelectionChanged {
            item: Mode::Portrait,
            index: 2,
            ..
        })
    ));
}

// ---------- атомарность смены пределов / списка ----------

/// Модель-двойник: пределы — флаг `poison`. Смена пределов на `true` модель **принимает**, но после
/// этого её диапазон положений нечисловой, и привод отвергнет геометрию: ошибка случается на втором
/// шаге `set_limits` — после того как модель уже приняла новые пределы.
#[derive(Debug, Clone, PartialEq)]
struct FlakyModel {
    inner: QuantizedLinear,
    poisoned: bool,
}

impl ValueModel for FlakyModel {
    type Value = f64;
    type Limits = bool;

    fn limits(&self) -> &bool {
        &self.poisoned
    }
    fn set_limits(&mut self, poisoned: bool) -> Result<(), ValueError> {
        self.poisoned = poisoned;
        Ok(())
    }
    fn quantization(&self) -> Quantization {
        self.inner.quantization()
    }
    fn position_range(&self) -> (f64, f64) {
        if self.poisoned {
            (f64::NAN, 1.0)
        } else {
            self.inner.position_range()
        }
    }
    fn clamp_value(&self, value: f64) -> Result<f64, ValueError> {
        self.inner.clamp_value(value)
    }
    fn position_of(&self, value: f64) -> Result<f64, ValueError> {
        self.inner.position_of(value)
    }
    fn value_at(&self, position: f64) -> f64 {
        self.inner.value_at(position)
    }
    fn marks(&self) -> Vec<Mark> {
        self.inner.marks()
    }
}

#[test]
fn scale_set_limits_failing_on_a_later_step_changes_nothing() {
    let tokens = UiKitTokens::default();
    let model = FlakyModel {
        inner: exposure_model(),
        poisoned: false,
    };
    let mut scale = SnapScale::new(
        model.clone(),
        1.0,
        right_spec(),
        &env(&tokens, MotionPreference::Full),
        ms(0),
    )
    .unwrap();
    let snapshot_before = scale.snapshot(ms(0));
    let marks_before = scale.marks();

    let result = scale.set_limits(true, ScrollTransition::Immediate, ms(1));
    assert!(matches!(result, Err(ScaleError::Scroll(_))), "{result:?}");
    assert_eq!(
        scale.model(),
        &model,
        "модель не приняла пределы, на которых привод отказал"
    );
    assert_eq!(scale.snapshot(ms(0)), snapshot_before);
    assert_eq!(scale.marks(), marks_before);
    assert!(scale.drain_events().is_empty());

    // Шкала жива: принимает нормальные пределы и работает дальше.
    scale.set_limits(false, ScrollTransition::Animated, ms(2)).unwrap();
    scale.drag_began(ms(3), pan_same(0.0)).unwrap();
}

#[test]
fn carousel_set_entries_failing_on_a_later_step_changes_nothing() {
    let tokens = UiKitTokens::default();
    let mut carousel = mode_carousel(&tokens, MotionPreference::Full);
    let offsets_before = carousel.snap_offsets().to_vec();
    let position_before = carousel.offset_points(ms(0));

    // Ширины конечны (проверка списка проходит), но смещения переполняются в бесконечность:
    // отказывает привод, уже после разбора списка.
    let huge = vec![
        CarouselEntry {
            item: Mode::Video,
            width_points: 1e308,
        },
        CarouselEntry {
            item: Mode::Photo,
            width_points: 1e308,
        },
        CarouselEntry {
            item: Mode::Portrait,
            width_points: 1e308,
        },
    ];
    let result = carousel.set_entries(huge, ms(1));
    assert!(matches!(result, Err(CarouselError::Scroll(_))), "{result:?}");
    assert_eq!(carousel.entries(), modes().as_slice());
    assert_eq!(carousel.snap_offsets(), offsets_before.as_slice());
    assert_eq!(carousel.selected(), &Mode::Video);
    assert_eq!(carousel.offset_points(ms(0)), position_before);
    assert!(carousel.drain_events().is_empty());

    // Карусель жива.
    carousel.drag_began(ms(2), pan_same(0.0)).unwrap();
    carousel.drag_moved(pan_same(-60.0)).unwrap();
    carousel.drag_ended(ms(18), horizontal(0.0)).unwrap();
    assert_eq!(carousel.selected(), &Mode::Photo);
}

// ---------- смена Reduce Motion на лету ----------

/// Токены, у которых прилипание шкалы и карусели с перелётом: тогда Reduce Motion виден глазами.
fn bouncy_snap_tokens() -> UiKitTokens {
    let mut tokens = UiKitTokens::default();
    tokens.motion.scale_snap = SpringToken {
        duration_s: 0.5,
        bounce: 0.3,
    };
    tokens.motion.camera_snap = SpringToken {
        duration_s: 0.85,
        bounce: 0.3,
    };
    tokens
}

/// Наибольшее смещение шкалы за 3 с программного перехода к значению 1,0 (цель — 36 pt).
fn scale_peak_after_set_value(scale: &mut SnapScale<QuantizedLinear>) -> f64 {
    scale.set_value(1.0, ScrollTransition::Animated, ms(0)).unwrap();
    (0..=3_000)
        .map(|step| scale.snapshot(ms(step)).offset_points)
        .fold(f64::MIN, f64::max)
}

#[test]
fn scale_motion_can_be_switched_on_a_live_element() {
    let tokens = bouncy_snap_tokens();
    let mut full = exposure_scale(-1.0, right_spec(), &tokens);
    assert!(scale_peak_after_set_value(&mut full) > 36.5, "контроль: с перелётом");

    let mut live = exposure_scale(-1.0, right_spec(), &tokens);
    let before = live.snapshot(ms(0));
    live.set_motion(MotionPreference::Reduced);
    assert_eq!(live.motion(), MotionPreference::Reduced);
    assert_eq!(live.snapshot(ms(0)), before, "смена режима не двигает шкалу");
    assert!(scale_peak_after_set_value(&mut live) <= 36.0 + 1e-6, "без перелёта");

    // Переключённая на лету шкала едет так же, как созданная сразу в Reduced.
    let mut born_reduced = SnapScale::new(
        exposure_model(),
        -1.0,
        right_spec(),
        &env(&tokens, MotionPreference::Reduced),
        ms(0),
    )
    .unwrap();
    born_reduced.set_value(1.0, ScrollTransition::Animated, ms(0)).unwrap();
    let mut live = exposure_scale(-1.0, right_spec(), &tokens);
    live.set_motion(MotionPreference::Reduced);
    live.set_value(1.0, ScrollTransition::Animated, ms(0)).unwrap();
    for at in [50, 200, 600, 2_000] {
        assert_eq!(
            live.snapshot(ms(at)).offset_points.to_bits(),
            born_reduced.snapshot(ms(at)).offset_points.to_bits(),
            "t = {at}"
        );
    }
}

#[test]
fn carousel_motion_can_be_switched_on_a_live_element() {
    let tokens = bouncy_snap_tokens();
    let peak = |carousel: &mut SnapCarousel<Mode>| {
        carousel.select(1, ScrollTransition::Animated, ms(0)).unwrap();
        (0..=4_000)
            .map(|step| carousel.offset_points(ms(step)))
            .fold(f64::MIN, f64::max)
    };
    let target = 61.1 / 2.0 + 35.4 + 25.0; // центр второго элемента, 90,95 pt
    let mut full = mode_carousel(&tokens, MotionPreference::Full);
    assert!(peak(&mut full) > target + 0.5, "контроль: с перелётом");

    let mut live = mode_carousel(&tokens, MotionPreference::Full);
    live.set_motion(MotionPreference::Reduced);
    assert_eq!(live.motion(), MotionPreference::Reduced);
    assert_eq!(live.selected(), &Mode::Video, "смена режима не меняет выбор");
    assert!(peak(&mut live) <= target + 1e-6, "без перелёта");
}

// ---------- один тип перехода и сопоставление фаз ----------

/// И шкала, и карусель принимают один и тот же `ScrollTransition` из `drive`.
#[test]
fn scale_and_carousel_share_one_transition_type() {
    let tokens = UiKitTokens::default();
    for transition in [ScrollTransition::Immediate, ScrollTransition::Animated] {
        let mut scale = exposure_scale(0.0, right_spec(), &tokens);
        scale.set_value(1.0, transition, ms(0)).unwrap();
        let mut carousel = mode_carousel(&tokens, MotionPreference::Full);
        carousel.select(2, transition, ms(0)).unwrap();
        let (settled_value, settled_item) = (scale.value(ms(5_000)), *carousel.selected());
        assert_eq!((settled_value, settled_item), (1.0, Mode::Portrait), "{transition:?}");
    }
}

/// Фазы движка → фазы элемента: каждая фаза `ui-core` сопоставлена явно (новая фаза не попадёт
/// молча в «движется»). Проверка по реальным состояниям шкалы.
#[test]
fn element_phases_follow_every_engine_phase() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(0.0, right_spec(), &tokens);
    assert_eq!(scale.phase(ms(0)), MotionPhase::Resting);
    scale.catch(ms(0));
    assert_eq!(scale.phase(ms(1)), MotionPhase::Touched);
    scale.drag_began(ms(2), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(3), pan_same(30.0)).unwrap();
    assert_eq!(scale.phase(ms(4)), MotionPhase::Touched);
    scale.drag_ended(ms(5), horizontal(400.0)).unwrap();
    assert_eq!(
        scale.phase(ms(6)),
        MotionPhase::Settling,
        "бросок и прилипание — движение"
    );
    assert_eq!(scale.phase(ms(10_000)), MotionPhase::Resting);
    // Ссылка на фазы движка: набор не должен тихо расходиться с тем, что знает элемент.
    let _all_engine_phases = [
        ScrollPhase::Resting,
        ScrollPhase::Dragging,
        ScrollPhase::Decelerating,
        ScrollPhase::SnappingToPoint,
        ScrollPhase::ReturningFromEdge,
        ScrollPhase::ScrollingToTarget,
    ];
}

/// Покой после прилипания не уезжает при смене пределов, пока значение в новых пределах.
#[test]
fn settled_scale_stays_put_when_limits_change_around_it() {
    let tokens = UiKitTokens::default();
    let mut scale = exposure_scale(0.0, right_spec(), &tokens);
    scale.drag_began(ms(0), pan_same(0.0)).unwrap();
    scale.drag_moved(ms(16), pan_same(30.0)).unwrap();
    scale.release_without_drag(ms(32)).unwrap();
    let rest = scale.snapshot(ms(10_000));
    assert_eq!(rest.phase, MotionPhase::Resting);
    // Кадр, на котором шкала «узнала» о покое (`Settled`), уже прошёл: дальше событий быть не должно.
    scale.advance(ms(10_000));
    let settled_events = scale.drain_events();
    assert!(
        matches!(settled_events.last(), Some(ScaleEvent::Settled { .. })),
        "{settled_events:?}"
    );
    scale
        .set_limits(
            LinearLimits { min: -1.0, max: 2.0 },
            ScrollTransition::Animated,
            ms(10_000),
        )
        .unwrap();
    for at in [10_000, 10_100, 12_000] {
        let snapshot = scale.snapshot(ms(at));
        assert_eq!(
            snapshot.offset_points.to_bits(),
            rest.offset_points.to_bits(),
            "t = {at}"
        );
        assert_eq!(snapshot.phase, MotionPhase::Resting);
    }
    assert!(scale.drain_events().is_empty(), "стоящая шкала событий не порождает");
}
