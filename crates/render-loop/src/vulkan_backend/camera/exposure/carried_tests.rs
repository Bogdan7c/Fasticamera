//! Ожидаемая экспозиция нового потока (13b): первый кадр нового потока открывается без собственного замера.
//!
//! Каждая проверка смотрит на результат: готовность яркости, подтяжку в плане кадра, предложение в ящике и счётчик.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use camera_contract::auto_exposure::{AppliedExposure, CarriedSceneExposure, ProductReach};
use camera_contract::fixtures::camera_id;

use super::test_support::*;
use super::*;

/// Камера потока харнеса (`test_support::stream_of`).
const STREAM_CAMERA: &str = "2";

fn product(seconds_iso: f64) -> ExposureProduct {
    ExposureProduct::from_seconds_iso(seconds_iso).expect("произведение")
}

/// Указание движка перед сменой потока: «на камере `camera` ожидаем `target`».
fn carrying(camera: &str, target_seconds_iso: f64) -> ExposureDirective {
    ExposureDirective {
        stream_start: Some(CarriedSceneExposure::for_camera(
            &camera_id(camera),
            product(target_seconds_iso),
        )),
        ..ExposureDirective::DEFAULT
    }
}

/// Харнес с указанием `directive`; первый кадр потока только спланирован, замера нет.
fn new_stream_under(directive: ExposureDirective) -> (Harness, RawFramePassport) {
    let mut harness = Harness::new();
    harness.directives.publish(directive);
    let first_frame = frame_at(&harness.stream, 0, Some(UNIT_EXPOSURE));
    harness.runner.plan_frame(&first_frame);
    (harness, first_frame)
}

fn readiness(harness: &Harness, frame: &RawFramePassport) -> ExposureReadiness {
    harness
        .runner
        .reveal_readiness(frame, RevealExposureCheck::MeasuredAndPlausible, 1.0)
}

fn carried_count(harness: &Harness) -> u64 {
    harness.runner.diagnostics().counters.frames_ready_by_carried_exposure
}

#[test]
fn first_frame_is_ready_by_the_carried_target_without_an_own_measurement() {
    // Кадр 1 с·ISO, движок ожидает ту же 1 с·ISO: правдоподобно, хотя замера потока нет.
    let (harness, frame) = new_stream_under(carrying(STREAM_CAMERA, 1.0));
    assert_eq!(
        harness.runner.diagnostics().counters.accepted,
        0,
        "собственного замера нет"
    );
    assert_eq!(readiness(&harness, &frame), ExposureReadiness::Ready);
    assert_eq!(carried_count(&harness), 1, "решение принято по ожидаемой экспозиции");
}

#[test]
fn without_a_carried_target_the_first_frame_waits_for_the_measurement() {
    let (harness, frame) = new_stream_under(ExposureDirective::DEFAULT);
    assert_eq!(readiness(&harness, &frame), ExposureReadiness::WaitingForTarget);
    assert_eq!(carried_count(&harness), 0);
}

#[test]
fn a_carried_target_for_another_camera_is_never_used() {
    let (harness, frame) = new_stream_under(carrying("0", 1.0));
    assert_eq!(readiness(&harness, &frame), ExposureReadiness::WaitingForTarget);
    assert_eq!(carried_count(&harness), 0);
}

#[test]
fn an_implausible_frame_level_keeps_waiting_even_with_a_carried_target() {
    // Ожидали 64 с·ISO (6 EV выше кадра): кадр темнее допуска 1 EV, подтяжка ±0,5 EV не спасает.
    let (harness, frame) = new_stream_under(carrying(STREAM_CAMERA, 64.0));
    assert_eq!(readiness(&harness, &frame), ExposureReadiness::WaitingLevelOff);
    assert_eq!(carried_count(&harness), 0);
}

#[test]
fn the_tolerance_applies_to_the_carried_target_as_it_does_to_a_measured_one() {
    // 1,5 с·ISO против кадра 1 с·ISO: 0,58 EV — в допуске 1 EV и вне допуска 0,01 EV (запроса нет, подтяжки нет).
    let (harness, frame) = new_stream_under(carrying(STREAM_CAMERA, 1.5));
    assert_eq!(
        harness
            .runner
            .reveal_readiness(&frame, RevealExposureCheck::MeasuredAndPlausible, 1.0),
        ExposureReadiness::Ready
    );
    assert_eq!(
        harness
            .runner
            .reveal_readiness(&frame, RevealExposureCheck::MeasuredAndPlausible, 0.01),
        ExposureReadiness::WaitingLevelOff
    );
}

#[test]
fn the_own_measurement_supersedes_the_carried_target() {
    // Движок ожидает 64 с·ISO (кадр 1 с·ISO далёк), но собственный замер говорит: сцена на цели для 1 с·ISO.
    let (mut harness, _) = new_stream_under(carrying(STREAM_CAMERA, 64.0));
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(TARGET_GRAY));
    let frame = frame_at(&harness.stream, harness.next_index, Some(UNIT_EXPOSURE));
    let before = carried_count(&harness);
    assert_eq!(
        readiness(&harness, &frame),
        ExposureReadiness::Ready,
        "по собственному замеру"
    );
    assert_eq!(carried_count(&harness), before, "ожидание больше не участвует");
    assert_close_ev(
        harness.latest_target(),
        1.0,
        0.02,
        "предложение — по замеру, не по ожиданию",
    );
}

#[test]
fn the_carried_target_alone_never_produces_a_proposal_to_the_engine() {
    let (harness, _) = new_stream_under(carrying(STREAM_CAMERA, 1.0));
    assert!(harness.proposals.latest().is_none(), "ожидание — не измерение");
    assert_eq!(harness.published(), 0);
}

#[test]
fn a_new_stream_forgets_the_carried_target_of_the_previous_one_when_the_camera_differs() {
    // Поток камеры «2» открылся по ожиданию; затем приходит поток другой камеры — указание ему не принадлежит.
    let (mut harness, _) = new_stream_under(carrying(STREAM_CAMERA, 1.0));
    let mut other_passport = (*harness.stream).clone();
    other_passport.camera = camera_id("0");
    let other_stream = Arc::new(other_passport);
    let other_frame = frame_at(&other_stream, 1, Some(UNIT_EXPOSURE));
    harness.runner.plan_frame(&other_frame);
    assert_eq!(readiness(&harness, &other_frame), ExposureReadiness::WaitingForTarget);
}

#[test]
fn the_residual_gain_is_seeded_from_the_carried_target_until_the_first_measurement() {
    // Движок ожидает 1,3 с·ISO (+0,38 EV к кадру 1 с·ISO) и поставил запрос с широкими пределами: подтяжка
    // тянет кадр к ожиданию, хотя замера нет.
    let mut directive = carrying(STREAM_CAMERA, 1.3);
    directive.applied = Some(AppliedExposure {
        product: product(1.0),
        shortfall_ev: 0.0,
        reach: ProductReach::Between {
            darkest: product(0.01),
            brightest: product(100.0),
        },
    });
    let mut harness = Harness::new();
    harness.directives.publish(directive);
    let frame = frame_at(&harness.stream, 0, Some(UNIT_EXPOSURE));
    let plan = harness.runner.plan_frame(&frame);
    assert!(
        (plan.gain.ev() - 0.38).abs() <= 0.15,
        "подтяжка должна быть около +0,38 EV, а не {} EV",
        plan.gain.ev()
    );
    // Без ожидания подтяжки нет: цели нет.
    let mut plain = Harness::new();
    plain.directives.publish(ExposureDirective {
        stream_start: None,
        ..directive
    });
    let plain_plan = plain
        .runner
        .plan_frame(&frame_at(&plain.stream, 0, Some(UNIT_EXPOSURE)));
    assert_eq!(plain_plan.gain, ExposureGain::UNITY);
}

#[test]
fn proposals_name_the_camera_they_were_measured_on() {
    let mut harness = Harness::new();
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    let (_, proposal) = harness.proposals.latest().expect("предложение опубликовано");
    assert_eq!(proposal.measured_on, CameraTag::of(&camera_id(STREAM_CAMERA)));
}

/// Поток другой камеры `camera` на том же паспорте харнеса.
fn stream_of_camera(harness: &Harness, camera: &str) -> Arc<camera_contract::frame_passport::StreamPassport> {
    let mut passport = (*harness.stream).clone();
    passport.camera = camera_id(camera);
    Arc::new(passport)
}

#[test]
fn a_rapid_swap_back_announced_before_the_first_frame_does_not_hide_the_promoted_camera() {
    // Порядок как на телефоне при дрожании пальца у отметки (решение 130): показывается камера «0»; движок
    // объявил ожидание для «2» и повысил её; кадры «2» ещё в буфере видоискателя, а движок уже объявил
    // ожидание для СЛЕДУЮЩЕГО обмена — обратно на «0». Первый кадр «2» обязан открыться по своему объявлению.
    let mut harness = Harness::new();
    let old_stream = stream_of_camera(&harness, "0");
    harness.directives.publish(carrying(STREAM_CAMERA, 1.0));
    // Рендер успел увидеть объявление для «2», пока показывал старую камеру.
    harness
        .runner
        .plan_frame(&frame_at(&old_stream, 0, Some(UNIT_EXPOSURE)));
    // Следующий обмен объявлен раньше, чем показан первый кадр «2».
    harness.directives.publish(carrying("0", 1.0));
    let first_promoted = frame_at(&harness.stream, 1, Some(UNIT_EXPOSURE));
    harness.runner.plan_frame(&first_promoted);
    assert_eq!(readiness(&harness, &first_promoted), ExposureReadiness::Ready);
    assert_eq!(carried_count(&harness), 1, "открыто по объявлению своей камеры");
}

#[test]
fn a_withdrawal_forgets_the_announcements_of_every_camera() {
    // Движок объявил «2», затем отозвал ожидание (обмен не состоялся): отзыв не называет камеру — забыть всё.
    let mut harness = Harness::new();
    let old_stream = stream_of_camera(&harness, "0");
    harness.directives.publish(carrying(STREAM_CAMERA, 1.0));
    harness
        .runner
        .plan_frame(&frame_at(&old_stream, 0, Some(UNIT_EXPOSURE)));
    harness.directives.publish(ExposureDirective::DEFAULT);
    let first = frame_at(&harness.stream, 1, Some(UNIT_EXPOSURE));
    harness.runner.plan_frame(&first);
    assert_eq!(readiness(&harness, &first), ExposureReadiness::WaitingForTarget);
    assert_eq!(carried_count(&harness), 0);
}

#[test]
fn a_newer_announcement_for_the_same_camera_replaces_the_older_one() {
    // Сначала для «2» ждали 64 с·ISO (неправдоподобно для кадра 1 с·ISO), потом переобъявили 1 с·ISO.
    let mut harness = Harness::new();
    let old_stream = stream_of_camera(&harness, "0");
    harness.directives.publish(carrying(STREAM_CAMERA, 64.0));
    harness
        .runner
        .plan_frame(&frame_at(&old_stream, 0, Some(UNIT_EXPOSURE)));
    harness.directives.publish(carrying(STREAM_CAMERA, 1.0));
    harness
        .runner
        .plan_frame(&frame_at(&old_stream, 1, Some(UNIT_EXPOSURE)));
    // Объявление другой камеры между ними не мешает.
    harness.directives.publish(carrying("0", 8.0));
    let first = frame_at(&harness.stream, 2, Some(UNIT_EXPOSURE));
    harness.runner.plan_frame(&first);
    assert_eq!(
        readiness(&harness, &first),
        ExposureReadiness::Ready,
        "по новому объявлению"
    );
}
