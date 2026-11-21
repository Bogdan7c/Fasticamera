//! AE потока рендера без GPU: замер и предложение, смена потока, область замера.
//!
//! Статистика строится так же, как её выдал бы ISP (плитки сумм). Каждая проверка смотрит на результат:
//! какое предложение ушло движку, какая подтяжка встала в план кадра, какие счётчики выросли.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "в тестах падение — сигнал провала"
)]

use std::time::Duration;

use camera_contract::auto_exposure::{AppliedExposure, ProductReach};
use camera_contract::capture_result::SensorTimestamp;
use camera_contract::stream::ReadoutAspect;
use camera_contract::three_a::{MeteringRegion, PointRequestId, SensorPoint};
use scene_estimation::executor::SubmitError;
use scene_estimation::exposure::{CompletedExposureMeasurement, CpuExposureMeter};

use super::test_support::*;
use super::*;

// ---- замер и предложение ----

#[test]
fn a_dark_scene_makes_the_runner_ask_the_engine_for_more_light() {
    let mut harness = Harness::new();
    // Кадр 1 с·ISO показывает сцену на 2 EV темнее цели: нужно ~4 с·ISO.
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    let (_, proposal) = harness.proposals.latest().expect("предложение опубликовано");
    assert_close_ev(
        proposal.target.seconds_iso(),
        required_product(DARK_VALUE),
        0.02,
        "требуемое произведение",
    );
    assert_eq!(proposal.metered_area, MeteredArea::WholeFrame);
    // Метка кадра предложения — кадр, по статистике которого оно сделано (первый), а не последний показанный.
    assert_eq!(proposal.measured_frame, SensorTimestamp::from_nanos(1_000_000_000));
}

#[test]
fn a_bright_scene_asks_for_less_light() {
    let mut harness = Harness::new();
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(BRIGHT_VALUE));
    assert_close_ev(
        harness.latest_target(),
        required_product(BRIGHT_VALUE),
        0.02,
        "цель яркой сцены",
    );
    assert!(harness.latest_target() < 1.0, "яркой сцене нужно меньше 1 с·ISO");
}

#[test]
fn the_measurement_uses_the_exposure_of_the_frame_that_was_measured() {
    // Та же яркость сцены на кадре с вдвое большей экспозицией означает, что сцене нужна вдвое меньшая.
    let mut harness = Harness::new();
    harness.steps(3, Some(exposure_product(2.0)), &uniform_scene(DARK_VALUE));
    assert_close_ev(
        harness.latest_target(),
        2.0 * required_product(DARK_VALUE),
        0.02,
        "цель при кадре 2 с·ISO",
    );
}

#[test]
fn proposals_are_rate_limited() {
    let mut steady = Harness::new();
    steady.steps(60, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    assert!(
        (1..=3).contains(&steady.published()),
        "постоянная сцена за секунду даёт единицы предложений: {}",
        steady.published()
    );
    // Сцена скачет каждый кадр: предложения не чаще раза в 50 мс (60 кадров ≈ 1 с → не больше 21).
    let mut jumpy = Harness::new();
    let (dark, bright) = (uniform_scene(DARK_VALUE), uniform_scene(BRIGHT_VALUE));
    for index in 0..60 {
        jumpy.step(Some(UNIT_EXPOSURE), if index % 2 == 0 { &dark } else { &bright });
    }
    assert!(
        (2..=21).contains(&jumpy.published()),
        "частота ограничена, но предложения идут: {}",
        jumpy.published()
    );
}

#[test]
fn frames_without_paired_exposure_are_counted_and_never_filled_in() {
    let mut harness = Harness::new();
    let scene = uniform_scene(DARK_VALUE);
    // 1. Пары нет вовсе.
    let stream = Arc::clone(&harness.stream);
    let lost_pair = missing_frame(&stream, 0);
    let plan = harness.runner.plan_frame(&lost_pair);
    harness
        .runner
        .ingest_statistics(plan.stream, identity_of(&lost_pair), &facts_of(&lost_pair), &scene);
    // 2. Пара есть, но выдержки нет / ISO нет / ISO нулевой.
    for (index, (time, iso)) in [
        (None, Some(100)),
        (Some(Duration::from_millis(10)), None),
        (Some(Duration::from_millis(10)), Some(0)),
    ]
    .into_iter()
    .enumerate()
    {
        let passport = frame_with_keys(&stream, i64::try_from(index).unwrap() + 1, time, iso);
        let plan = harness.runner.plan_frame(&passport);
        harness
            .runner
            .ingest_statistics(plan.stream, identity_of(&passport), &facts_of(&passport), &scene);
    }
    let diagnostics = harness.runner.diagnostics();
    assert_eq!(diagnostics.counters.skipped_no_paired_exposure, 4);
    assert_eq!(diagnostics.counters.accepted, 0, "замера нет: подставлять нечего");
    assert_eq!(diagnostics.target_log2_seconds_iso, None);
    assert!(
        harness.proposals.latest().is_none(),
        "предложение без замера не публикуется"
    );
}

#[test]
fn a_busy_meter_skips_the_frame_with_a_counter() {
    struct AlwaysBusy;
    impl ExposureMeter for AlwaysBusy {
        fn submit(&mut self, _request: ExposureMeteringRequest<'_>) -> Result<(), SubmitError> {
            Err(SubmitError::Busy)
        }
        fn poll(&mut self) -> Option<CompletedExposureMeasurement> {
            None
        }
    }
    let mut harness = Harness::new();
    harness.runner.meter = Box::new(AlwaysBusy);
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    let counters = harness.runner.diagnostics().counters;
    assert_eq!((counters.skipped_meter_busy, counters.accepted), (3, 0));
}

// ---- потоки ----

#[test]
fn a_new_stream_starts_over_and_the_old_stream_cannot_publish_into_it() {
    let mut harness = Harness::new();
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    let published_in_a = harness.published();
    assert!(published_in_a >= 1);
    let generation_in_a = harness.proposals.latest().expect("предложение потока A").0;
    // Статистика кадра потока A, собранная уже после смены потока.
    let old_stream = Arc::clone(&harness.stream);
    let late_frame = frame_at(&old_stream, 100, Some(UNIT_EXPOSURE));
    let old_plan_stream = harness.runner.plan_frame(&late_frame).stream;

    // Новый поток B: контроллер сброшен, цели нет, новых предложений нет, пока B не измерен.
    harness.stream = new_stream();
    harness.next_index = 1_000;
    let plan_b = harness
        .runner
        .plan_frame(&frame_at(&harness.stream, 1_000, Some(UNIT_EXPOSURE)));
    assert_ne!(plan_b.stream, old_plan_stream);
    assert_eq!(
        plan_b.gain,
        ExposureGain::UNITY,
        "в новом потоке подтяжки нет, пока нет цели"
    );
    harness.runner.ingest_statistics(
        old_plan_stream,
        identity_of(&late_frame),
        &facts_of(&late_frame),
        &uniform_scene(BRIGHT_VALUE),
    );
    let after_stale = harness.runner.diagnostics();
    assert_eq!(
        after_stale.counters.statistics_stale, 1,
        "статистика чужого потока отброшена"
    );
    assert_eq!(
        after_stale.target_log2_seconds_iso, None,
        "цель потока A в поток B не перешла"
    );
    assert_eq!(harness.proposals.latest().expect("A").0, generation_in_a);
    assert_eq!(
        after_stale.last_proposal, None,
        "диагностика нового потока не показывает предложение старого"
    );

    // Поток B измерен: предложение уходит, и оно про кадры потока B.
    harness.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(BRIGHT_VALUE));
    let (generation_in_b, proposal_b) = harness.proposals.latest().expect("предложение потока B");
    assert!(generation_in_b.0 > generation_in_a.0);
    assert!(proposal_b.measured_frame >= SensorTimestamp::from_nanos(1_000_000_000 + 1_000 * FRAME_NANOS));
    assert_close_ev(
        proposal_b.target.seconds_iso(),
        required_product(BRIGHT_VALUE),
        0.02,
        "цель потока B",
    );
}

// ---- область замера ----

#[test]
fn a_tap_meters_only_the_region_and_a_bright_window_elsewhere_does_not_count() {
    let scene = bright_window_left_scene();
    // Тап по тёмной правой половине: цель — по тёмной области.
    let mut spot = Harness::new();
    spot.directives.publish(point_directive(0.75, 0.5, 1));
    spot.steps(3, Some(UNIT_EXPOSURE), &scene);
    let (_, proposal) = spot.proposals.latest().expect("предложение по точке");
    assert_eq!(proposal.metered_area, MeteredArea::Point(PointRequestId(1)));
    assert_close_ev(
        proposal.target.seconds_iso(),
        required_product(DARK_VALUE),
        0.4,
        "тап по тёмной области",
    );
    // Матричный замер той же сцены видит и яркое окно: цель заметно меньше.
    let mut matrix = Harness::new();
    matrix.steps(3, Some(UNIT_EXPOSURE), &scene);
    let whole_frame = matrix.latest_target();
    assert!(
        (spot.latest_target() / whole_frame).log2() > 1.5,
        "тап по тёмной области просит заметно больше света, чем весь кадр: {} против {whole_frame}",
        spot.latest_target()
    );
}

#[test]
fn a_new_tap_rebuilds_the_weights_and_the_estimate_follows_the_new_region() {
    let scene = bright_window_left_scene();
    let mut harness = Harness::new();
    harness.directives.publish(point_directive(0.75, 0.5, 1));
    harness.steps(3, Some(UNIT_EXPOSURE), &scene);
    // Тап по яркому окну: через секунду сглаженная цель приходит к яркой области.
    harness.directives.publish(point_directive(0.25, 0.5, 2));
    harness.steps(90, Some(UNIT_EXPOSURE), &scene);
    let diagnostics = harness.runner.diagnostics();
    assert_eq!(diagnostics.metered_area, MeteredArea::Point(PointRequestId(2)));
    let target = diagnostics.target_log2_seconds_iso.expect("цель есть").exp2();
    assert_close_ev(target, required_product(BRIGHT_VALUE), 0.4, "цель после нового тапа");
    assert_eq!(diagnostics.counters.rejected_foreign_area, 0);
}

#[test]
fn a_region_outside_the_sixteen_by_nine_band_is_skipped_with_a_counter_and_stops_the_cold_start_demand() {
    let mut harness = Harness::new();
    harness.stream = stream_of(ReadoutAspect::SixteenByNine, 36);
    // Верхние 12,5 % активной области в режиме 16:9 не читаются: тап по y = 0,02 вне сетки.
    let region = MeteringRegion::new(SensorPoint::new(0.5, 0.02).expect("точка"), 0.02).expect("регион");
    harness.directives.publish(ExposureDirective {
        metering: PointOfInterest::Point {
            region,
            request: PointRequestId(7),
        },
        ..ExposureDirective::DEFAULT
    });
    let first = harness.step(Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    assert_eq!(first.statistics, StatisticsDemand::EveryFrame, "оценки ещё нет");
    let second = harness.step(Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    let counters = harness.runner.diagnostics().counters;
    assert_eq!((counters.skipped_area_unavailable, counters.accepted), (2, 0));
    assert_eq!(
        second.statistics,
        StatisticsDemand::WhenScheduled,
        "оценки не будет: статистику каждый кадр не просим"
    );
}

#[test]
fn ready_measurements_are_collected_even_when_the_current_frame_is_skipped() {
    /// Исполнитель с задержкой: ответ отдаёт на втором опросе после постановки.
    struct Delayed {
        inner: CpuExposureMeter,
        held: Vec<CompletedExposureMeasurement>,
        polls_since_submit: u32,
    }
    impl ExposureMeter for Delayed {
        fn submit(&mut self, request: ExposureMeteringRequest<'_>) -> Result<(), SubmitError> {
            self.inner.submit(request)?;
            self.held.extend(self.inner.poll());
            self.polls_since_submit = 0;
            Ok(())
        }
        fn poll(&mut self) -> Option<CompletedExposureMeasurement> {
            if self.polls_since_submit == 0 {
                self.polls_since_submit = 1;
                return None;
            }
            self.held.pop()
        }
    }
    let mut harness = Harness::new();
    harness.runner.meter = Box::new(Delayed {
        inner: CpuExposureMeter::new(AutoExposureConfig::default().metering).expect("замер"),
        held: Vec::new(),
        polls_since_submit: 0,
    });
    let scene = uniform_scene(DARK_VALUE);
    harness.step(Some(UNIT_EXPOSURE), &scene);
    assert_eq!(harness.runner.diagnostics().counters.accepted, 0, "ответ ещё в пути");
    // Следующий кадр без пары: сам не меряется, но готовый замер прошлого кадра обязан дойти до контроллера.
    let stream = Arc::clone(&harness.stream);
    let unpaired = missing_frame(&stream, 1);
    let plan = harness.runner.plan_frame(&unpaired);
    harness
        .runner
        .ingest_statistics(plan.stream, identity_of(&unpaired), &facts_of(&unpaired), &scene);
    let counters = harness.runner.diagnostics().counters;
    assert_eq!((counters.accepted, counters.skipped_no_paired_exposure), (1, 1));
}

// ---- готовность яркости для показа нового потока вместо удержанного кадра (сессия 13) ----

/// Готовность яркости последнего кадра харнеса при проверке «измерено и правдоподобно» с допуском 1 EV.
fn readiness_of(harness: &Harness, exposure: Option<Exposure>) -> ExposureReadiness {
    let passport = frame_at(&harness.stream, harness.next_index, exposure);
    harness
        .runner
        .reveal_readiness(&passport, RevealExposureCheck::MeasuredAndPlausible, 1.0)
}

#[test]
fn reveal_waits_for_the_first_measurement_of_the_new_stream() {
    let mut harness = Harness::new();
    // Поток привязан (кадр спланирован), но замера его статистики ещё не было.
    let passport = frame_at(&harness.stream, 0, Some(UNIT_EXPOSURE));
    harness.runner.plan_frame(&passport);
    assert_eq!(
        harness
            .runner
            .reveal_readiness(&passport, RevealExposureCheck::MeasuredAndPlausible, 1.0),
        ExposureReadiness::WaitingForTarget,
        "замера нет — яркость не известна"
    );
    assert_eq!(
        harness
            .runner
            .reveal_readiness(&passport, RevealExposureCheck::Ignore, 1.0),
        ExposureReadiness::Ready,
        "проверка выключена конфигом"
    );
}

#[test]
fn reveal_is_ready_only_when_the_shown_level_is_close_to_the_target() {
    // Кадр снят с экспозицией, на 2 EV меньшей нужной (тёмная сцена, камера ещё не ответила): замер есть, но
    // показанный уровень далёк от цели даже с подтяжкой ±0,5 EV — ждём.
    let mut dark_frame = Harness::new();
    dark_frame.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(DARK_VALUE));
    assert_eq!(
        readiness_of(&dark_frame, Some(UNIT_EXPOSURE)),
        ReadinessProbe::WAITING,
        "замер принят, но уровень на экране далёк от цели"
    );
    // Кадр уже на цели: его замер даёт ровно средний серый при его экспозиции, показанный уровень = цель.
    let mut on_target = Harness::new();
    on_target.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(TARGET_GRAY));
    assert_eq!(readiness_of(&on_target, Some(UNIT_EXPOSURE)), ReadinessProbe::READY);
    // И допуск работает как параметр: те же числа с допуском 0,1 EV и сдвигом сцены на 0,3 EV — ждём.
    let mut slightly_off = Harness::new();
    slightly_off.steps(3, Some(UNIT_EXPOSURE), &uniform_scene(TARGET_GRAY * 1.23));
    let passport = frame_at(&slightly_off.stream, slightly_off.next_index, Some(UNIT_EXPOSURE));
    assert_eq!(
        slightly_off
            .runner
            .reveal_readiness(&passport, RevealExposureCheck::MeasuredAndPlausible, 1.0),
        ExposureReadiness::Ready
    );
    assert_eq!(
        slightly_off
            .runner
            .reveal_readiness(&passport, RevealExposureCheck::MeasuredAndPlausible, 0.1),
        ExposureReadiness::WaitingLevelOff
    );
}

#[test]
fn reveal_does_not_wait_when_the_ae_has_nothing_to_measure() {
    // Кадр без парных выдержки и ISO: измерять нечего, ждать весь срок удержания зря.
    let harness = Harness::new();
    let unpaired = missing_frame(&harness.stream, 0);
    assert_eq!(
        harness
            .runner
            .reveal_readiness(&unpaired, RevealExposureCheck::MeasuredAndPlausible, 1.0),
        ExposureReadiness::Ready
    );
    // Ручная экспозиция: AE ничего не решает.
    let mut manual = Harness::new();
    let mut directive = ExposureDirective::DEFAULT;
    directive.authority = ExposureAuthority::Manual;
    manual.directives.publish(directive);
    let passport = frame_at(&manual.stream, 0, Some(UNIT_EXPOSURE));
    manual.runner.plan_frame(&passport);
    assert_eq!(
        manual
            .runner
            .reveal_readiness(&passport, RevealExposureCheck::MeasuredAndPlausible, 1.0),
        ExposureReadiness::Ready
    );
}

/// Короткие имена результатов для читаемых `assert_eq!`.
struct ReadinessProbe;

impl ReadinessProbe {
    const READY: ExposureReadiness = ExposureReadiness::Ready;
    const WAITING: ExposureReadiness = ExposureReadiness::WaitingLevelOff;
}

// ---- готовность в тёмной сцене: цель недостижима, кадр на краю пределов потока ----

/// Произведение потока на его самом светлом краю в «тёмной» комнате телефона (6,7 с·ISO в логе: 106,67).
const BRIGHTEST_SECONDS_ISO: f64 = 100.0;
const DARKEST_SECONDS_ISO: f64 = 0.01;

/// Указание: в запросе стоит `applied_seconds_iso`, достижим отрезок `[darkest, brightest]`.
fn directive_with_reach(applied_seconds_iso: f64, darkest: f64, brightest: f64) -> ExposureDirective {
    let product = |seconds_iso| ExposureProduct::from_seconds_iso(seconds_iso).expect("произведение");
    ExposureDirective {
        applied: Some(AppliedExposure {
            product: product(applied_seconds_iso),
            shortfall_ev: -4.0,
            reach: ProductReach::Between {
                darkest: product(darkest),
                brightest: product(brightest),
            },
        }),
        ..ExposureDirective::DEFAULT
    }
}

/// Харнес, где движок сообщил указание `directive`, а кадры идут с произведением `frame_seconds_iso`.
fn harness_under(directive: ExposureDirective, frame_seconds_iso: f64, scene: &AwbStatistics) -> Harness {
    let mut harness = Harness::new();
    harness.directives.publish(directive);
    harness.steps(3, Some(exposure_product(frame_seconds_iso)), scene);
    harness
}

#[test]
fn reveal_is_ready_in_a_dark_room_when_the_frame_sits_at_the_brightest_edge() {
    // Сцена на 4 EV темнее цели при самом светлом кадре потока: цель недостижима, AE сделал всё возможное.
    let scene = uniform_scene(TARGET_GRAY / 16.0);
    let harness = harness_under(
        directive_with_reach(BRIGHTEST_SECONDS_ISO, DARKEST_SECONDS_ISO, BRIGHTEST_SECONDS_ISO),
        BRIGHTEST_SECONDS_ISO,
        &scene,
    );
    let target_above_edge_ev =
        harness.runner.controller.target_log2().expect("цель есть") - BRIGHTEST_SECONDS_ISO.log2();
    assert!(
        target_above_edge_ev > 3.0,
        "цель должна быть за краем: {target_above_edge_ev} EV"
    );
    assert_eq!(
        readiness_of(&harness, Some(exposure_product(BRIGHTEST_SECONDS_ISO))),
        ReadinessProbe::READY
    );
}

#[test]
fn reveal_is_ready_when_the_frame_sits_at_the_darkest_edge_of_a_bright_scene() {
    // Симметрия: сцена светлее цели на ~2,7 EV, кадр уже на самом тёмном краю потока (1 с·ISO).
    let harness = harness_under(
        directive_with_reach(1.0, 1.0, BRIGHTEST_SECONDS_ISO),
        1.0,
        &uniform_scene(BRIGHT_VALUE),
    );
    assert_eq!(readiness_of(&harness, Some(UNIT_EXPOSURE)), ReadinessProbe::READY);
}

#[test]
fn reveal_still_waits_while_ae_has_headroom_to_converge() {
    // Цель в пределах потока (≈4 с·ISO), кадр на 2 EV темнее и подтяжка ±0,5 EV не дотягивает: AE сходится — ждём.
    let harness = harness_under(
        directive_with_reach(1.0, DARKEST_SECONDS_ISO, BRIGHTEST_SECONDS_ISO),
        1.0,
        &uniform_scene(DARK_VALUE),
    );
    assert_eq!(readiness_of(&harness, Some(UNIT_EXPOSURE)), ReadinessProbe::WAITING);
}

#[test]
fn reveal_is_ready_within_a_reachable_target() {
    // Сцена на цели, пределы потока широкие: готово и без срезки.
    let harness = harness_under(
        directive_with_reach(1.0, DARKEST_SECONDS_ISO, BRIGHTEST_SECONDS_ISO),
        1.0,
        &uniform_scene(TARGET_GRAY),
    );
    assert_eq!(readiness_of(&harness, Some(UNIT_EXPOSURE)), ReadinessProbe::READY);
}

#[test]
fn reveal_does_not_trust_the_previous_streams_reach() {
    // Указание всё ещё описывает прошлую камеру (самое светлое 26,67 с·ISO), а кадр нового потока стоит на
    // 100 с·ISO — вне чужого отрезка. Тёмная цель недостижима «по чужим пределам»: ждём, а не открываем.
    let scene = uniform_scene(TARGET_GRAY / 16.0);
    let harness = harness_under(
        directive_with_reach(26.67, DARKEST_SECONDS_ISO, 26.67),
        BRIGHTEST_SECONDS_ISO,
        &scene,
    );
    assert_eq!(
        readiness_of(&harness, Some(exposure_product(BRIGHTEST_SECONDS_ISO))),
        ReadinessProbe::WAITING
    );
    // Запрос снят (applied = None, поток закончился): пределов нет, ждём полной цели.
    let mut withdrawn = Harness::new();
    withdrawn.directives.publish(ExposureDirective::DEFAULT);
    withdrawn.steps(3, Some(exposure_product(BRIGHTEST_SECONDS_ISO)), &scene);
    assert_eq!(
        readiness_of(&withdrawn, Some(exposure_product(BRIGHTEST_SECONDS_ISO))),
        ReadinessProbe::WAITING
    );
}
