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
